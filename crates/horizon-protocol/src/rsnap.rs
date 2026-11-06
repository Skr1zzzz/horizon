//! Bounded, owned PODRSNAP v1 decoding; no filesystem or transport access.
use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::ops::Range;

const MAX_FILE: usize = 64 << 20;
const MAX_ENTRIES: usize = 1 << 22;
const MISSING: u32 = 1 << 7;

#[derive(Debug)]
pub struct Block {
    pub name: String,
    pub raw_id: u32,
    pub default_state: u32,
    states: Range<usize>,
}

#[derive(Debug)]
pub struct State {
    /// BLOK index, not the block's registry raw id; invalid only for MISSING states.
    pub block: u32,
    pub properties: String,
    /// Index into Snapshot::shapes; None for dynamic, failed or missing states.
    pub shape: Option<u32>,
    pub luminance: u8,
    pub opacity: u8,
    pub flags: u32,
}

#[derive(Debug)]
pub struct Snapshot {
    pub protocol: u32,
    pub blocks: Vec<Block>,
    pub states: Vec<State>,
    pub registries: Vec<(String, Vec<String>)>,
    /// Shared ranges avoid multiplying large collision shapes by the state count.
    pub shapes: Vec<Range<usize>>,
    pub boxes: Vec<[f64; 6]>,
}

fn bad() -> Error {
    Error::new("malformed or over-budget PODRSNAP v1")
}
fn bytes(data: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    data.get(start..start.checked_add(len).ok_or_else(bad)?)
        .ok_or_else(bad)
}
fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes(data, at, 4)?.try_into().map_err(|_| bad())?,
    ))
}
fn u64_at(data: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(
        bytes(data, at, 8)?.try_into().map_err(|_| bad())?,
    ))
}
fn span(start: u32, count: u32, limit: usize) -> Result<Range<usize>> {
    let start = usize::try_from(start).map_err(|_| bad())?;
    let end = start
        .checked_add(usize::try_from(count).map_err(|_| bad())?)
        .ok_or_else(bad)?;
    if end > limit {
        return Err(bad());
    }
    Ok(start..end)
}
fn text(strings: &[u8], row: &[u8], at: usize, budget: &mut usize) -> Result<String> {
    let range = span(u32_at(row, at)?, u32_at(row, at + 4)?, strings.len())?;
    *budget = budget.checked_sub(range.len()).ok_or_else(bad)?;
    Ok(std::str::from_utf8(&strings[range])
        .map_err(|_| bad())?
        .to_owned())
}

impl Snapshot {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() > MAX_FILE
            || bytes(data, 0, 8)? != b"PODRSNAP"
            || u32_at(data, 8)? != 1
            || u32_at(data, 12)? != 0
        {
            return Err(bad());
        }
        let protocol = u32_at(data, 16)?;
        let count = u32_at(data, 20)? as usize;
        if count != 7 {
            return Err(bad());
        }
        let table_end = 64 + count * 24;
        bytes(data, 0, table_end)?;
        if Sha256::digest(&data[64..])[..] != data[32..64] {
            return Err(Error::new("PODRSNAP checksum mismatch"));
        }
        let tags = [
            *b"STAT", *b"SHAP", *b"BOXS", *b"BLOK", *b"REGS", *b"RENT", *b"STRS",
        ];
        let sizes = [24, 8, 48, 16, 16, 8, 1];
        let mut sections: [Option<&[u8]>; 7] = [None; 7];
        let mut ranges = Vec::new();
        for row in data[64..table_end].chunks_exact(24) {
            let index = tags
                .iter()
                .position(|tag| tag[..] == row[..4])
                .ok_or_else(bad)?;
            let start = usize::try_from(u64_at(row, 8)?).map_err(|_| bad())?;
            let len = usize::try_from(u64_at(row, 16)?).map_err(|_| bad())?;
            let section = bytes(data, start, len)?;
            let end = start.checked_add(len).ok_or_else(bad)?;
            if u32_at(row, 4)? != 0
                || start < table_end
                || start % 8 != 0
                || sections[index].is_some()
                || len % sizes[index] != 0
                || (index != 6 && len / sizes[index] > MAX_ENTRIES)
            {
                return Err(bad());
            }
            sections[index] = Some(section);
            ranges.push(start..end);
        }
        ranges.sort_by_key(|r| r.start);
        let mut end = table_end;
        for range in ranges {
            let aligned = end.checked_add(7).ok_or_else(bad)? & !7;
            if range.start != aligned || data[end..range.start].iter().any(|&b| b != 0) {
                return Err(bad());
            }
            end = range.end;
        }
        // Require final padding too: every strict prefix of a complete file is invalid.
        if data.len() != (end.checked_add(7).ok_or_else(bad)? & !7)
            || data[end..].iter().any(|&b| b != 0)
        {
            return Err(bad());
        }
        let [stat, shap, boxs, blok, regs, rent, strings] = sections.map(|s| s.ok_or_else(bad));
        let (stat, shap, boxs, blok, regs, rent, strings) =
            (stat?, shap?, boxs?, blok?, regs?, rent?, strings?);
        std::str::from_utf8(strings).map_err(|_| bad())?;
        // Deduplicated STRS references can otherwise amplify a small input quadratically.
        let mut budget = MAX_FILE;
        let mut boxes = Vec::new();
        for row in boxs.chunks_exact(48) {
            let mut bounds = [0.0; 6];
            for (i, value) in bounds.iter_mut().enumerate() {
                *value = f64::from_bits(u64_at(row, i * 8)?);
                if !value.is_finite() {
                    return Err(bad());
                }
            }
            if (0..3).any(|i| bounds[i] > bounds[i + 3]) {
                return Err(bad());
            }
            boxes.push(bounds);
        }
        let mut shapes = Vec::new();
        for row in shap.chunks_exact(8) {
            shapes.push(span(u32_at(row, 0)?, u32_at(row, 4)?, boxes.len())?);
        }
        let mut blocks = Vec::new();
        for row in blok.chunks_exact(16) {
            blocks.push(Block {
                name: text(strings, row, 0, &mut budget)?,
                raw_id: u32_at(row, 8)?,
                default_state: u32_at(row, 12)?,
                states: 0..0,
            });
        }
        let mut raw_ids: Vec<_> = blocks.iter().map(|b| b.raw_id).collect();
        raw_ids.sort_unstable();
        if raw_ids.windows(2).any(|w| w[0] == w[1]) {
            return Err(bad());
        }
        let mut states = Vec::new();
        for row in stat.chunks_exact(24) {
            let block = u32_at(row, 0)?;
            let shape = u32_at(row, 4)?;
            let flags = u32_at(row, 8)?;
            if (flags & MISSING == 0 && block as usize >= blocks.len())
                || (shape != u32::MAX && shape as usize >= shapes.len())
                || row[22..24] != [0, 0]
                || row[20] > 15
                || row[21] > 15
            {
                return Err(bad());
            }
            if flags & MISSING == 0 {
                let range = &mut blocks[block as usize].states;
                if range.start == range.end {
                    *range = states.len()..states.len();
                }
                if range.end != states.len() {
                    return Err(bad());
                }
                range.end += 1;
            }
            states.push(State {
                block,
                properties: text(strings, row, 12, &mut budget)?,
                shape: (shape != u32::MAX && flags & (3 | MISSING) == 0).then_some(shape),
                luminance: row[20],
                opacity: row[21],
                flags,
            });
        }
        for (i, block) in blocks.iter().enumerate() {
            let default = states.get(block.default_state as usize).ok_or_else(bad)?;
            if default.flags & MISSING != 0 || default.block as usize != i {
                return Err(bad());
            }
        }
        // Validate even unreferenced RENT entries, without allocating their strings twice.
        for row in rent.chunks_exact(8) {
            let range = span(u32_at(row, 0)?, u32_at(row, 4)?, strings.len())?;
            std::str::from_utf8(&strings[range]).map_err(|_| bad())?;
        }
        let mut registries = Vec::new();
        let mut entries_left = MAX_ENTRIES;
        for row in regs.chunks_exact(16) {
            let range = span(u32_at(row, 8)?, u32_at(row, 12)?, rent.len() / 8)?;
            entries_left = entries_left.checked_sub(range.len()).ok_or_else(bad)?;
            let mut names = Vec::new();
            for i in range {
                names.push(text(strings, &rent[i * 8..i * 8 + 8], 0, &mut budget)?);
            }
            registries.push((text(strings, row, 0, &mut budget)?, names));
        }
        Ok(Self {
            protocol,
            blocks,
            states,
            registries,
            shapes,
            boxes,
        })
    }

    pub fn state_name(&self, id: u32) -> Option<String> {
        let state = self.states.get(id as usize)?;
        if state.flags & MISSING != 0 {
            return None;
        }
        let name = &self.blocks.get(state.block as usize)?.name;
        Some(if state.properties.is_empty() {
            name.clone()
        } else {
            format!("{name}[{}]", state.properties)
        })
    }

    pub fn block_by_name(&self, name: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.name == name)
    }

    pub fn states_of_block(&self, raw: u32) -> Option<Range<usize>> {
        self.blocks
            .iter()
            .find(|b| b.raw_id == raw)
            .map(|b| b.states.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn seal(data: &mut [u8]) {
        let hash = Sha256::digest(&data[64..]);
        data[32..64].copy_from_slice(&hash);
    }
    fn fixture() -> Vec<u8> {
        let mut stat = words(&[0, 0, 8, 15, 8]);
        stat.extend_from_slice(&[7, 15, 0, 0]);
        let sections = [
            (*b"STAT", stat),
            (*b"SHAP", words(&[0, 1])),
            (
                *b"BOXS",
                [0.0_f64, 0.0, 0.0, 1.0, 1.0, 1.0]
                    .into_iter()
                    .flat_map(f64::to_le_bytes)
                    .collect(),
            ),
            (*b"BLOK", words(&[0, 15, 9, 0])),
            (*b"REGS", words(&[23, 15, 0, 1])),
            (*b"RENT", words(&[0, 15])),
            (*b"STRS", b"test:tiny_blocklit=trueminecraft:block".to_vec()),
        ];
        let mut out = vec![0; 64 + sections.len() * 24];
        out[..8].copy_from_slice(b"PODRSNAP");
        out[8..12].copy_from_slice(&1_u32.to_le_bytes());
        out[16..20].copy_from_slice(&774_u32.to_le_bytes());
        out[20..24].copy_from_slice(&7_u32.to_le_bytes());
        for (i, (tag, data)) in sections.iter().enumerate() {
            let at = 64 + i * 24;
            let start = out.len() as u64;
            out[at..at + 4].copy_from_slice(tag);
            out[at + 8..at + 16].copy_from_slice(&start.to_le_bytes());
            out[at + 16..at + 24].copy_from_slice(&(data.len() as u64).to_le_bytes());
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(8) * 8, 0);
        }
        seal(&mut out);
        out
    }

    #[test]
    fn round_trip_and_truncations() {
        let data = fixture();
        let snapshot = Snapshot::parse(&data).unwrap();
        assert_eq!(snapshot.protocol, 774);
        assert_eq!(
            snapshot.state_name(0).as_deref(),
            Some("test:tiny_block[lit=true]")
        );
        assert_eq!(snapshot.states_of_block(9), Some(0..1));
        assert_eq!(
            snapshot
                .block_by_name("test:tiny_block")
                .unwrap()
                .default_state,
            0
        );
        assert_eq!(snapshot.states[0].luminance, 7);
        assert_eq!(snapshot.states[0].opacity, 15);
        assert_eq!(snapshot.states[0].flags, 8);
        assert_eq!(snapshot.states[0].shape, Some(0));
        assert_eq!(snapshot.shapes, vec![0..1]);
        assert_eq!(snapshot.boxes, vec![[0., 0., 0., 1., 1., 1.]]);
        assert_eq!(
            snapshot.registries,
            vec![("minecraft:block".into(), vec!["test:tiny_block".into()])]
        );
        assert_eq!(snapshot.state_name(1), None);
        for end in 0..data.len() {
            assert!(Snapshot::parse(&data[..end]).is_err(), "prefix {end}");
        }
    }

    #[test]
    fn corrupt_ranges_and_checksum() {
        let original = fixture();
        for at in [72, 80, 64 + 6 * 24 + 8] {
            let mut data = original.clone();
            data[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
            seal(&mut data);
            assert!(Snapshot::parse(&data).is_err());
        }
        let stat = u64_at(&original, 72).unwrap() as usize;
        for at in [stat, stat + 4, stat + 12, stat + 16] {
            let mut data = original.clone();
            data[at..at + 4].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
            seal(&mut data);
            assert!(Snapshot::parse(&data).is_err());
        }
        let mut data = original;
        data[32] ^= 1;
        assert!(Snapshot::parse(&data).is_err());
    }

    #[test]
    #[ignore = "requires the local Fabric podtest snapshot"]
    fn real_podtest() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local/pod/rsnap-podtest.bin");
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("podtest snapshot absent; skipped");
                return;
            }
            Err(e) => panic!("reading podtest snapshot: {e}"),
        };
        let snapshot = Snapshot::parse(&data).unwrap();
        assert_eq!(snapshot.protocol, 774);
        for (name, expected) in [
            ("podtest:lamp", vec!["lit=false", "lit=true"]),
            ("podtest:pillar", vec!["axis=x", "axis=y", "axis=z"]),
        ] {
            let block = snapshot.block_by_name(name).unwrap();
            let range = snapshot.states_of_block(block.raw_id).unwrap();
            let mut props: Vec<_> = snapshot.states[range]
                .iter()
                .map(|s| s.properties.as_str())
                .collect();
            props.sort_unstable();
            assert_eq!(props, expected);
        }
        assert!(snapshot.block_by_name("minecraft:stone").is_some());
        assert!(snapshot.block_by_name("podtest:ruby_block").is_some());
        println!(
            "protocol={}, blocks={}, states={}, shapes={}, boxes={}, registries={}, entries={}",
            snapshot.protocol,
            snapshot.blocks.len(),
            snapshot.states.len(),
            snapshot.shapes.len(),
            snapshot.boxes.len(),
            snapshot.registries.len(),
            snapshot
                .registries
                .iter()
                .map(|(_, entries)| entries.len())
                .sum::<usize>()
        );
    }
}
