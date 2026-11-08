//! Experimental Fabric configuration codecs, implemented from the wire specification.
//! References: Fabric API 0.141.3 (Apache-2.0), registry-sync-v0
//! `RegistrySyncPayload`, networking-api-v1 `RegistrationPayload`, `CommonVersionPayload`
//! and `ClientConfigurationNetworkAddon`. No registry remapping is performed here.
use crate::{Error, Result, buf::Reader, packets::configuration as cfg};
use std::collections::{BTreeMap, BTreeSet};

const SYNC: &str = "fabric:registry/sync";
const RECEIVERS: &[u8] = b"fabric:registry/sync\0c:version\0voicechat:secret\0voicechat:state\0voicechat:states\0voicechat:remove_state";
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 262_144;

#[derive(Debug, Clone)]
pub struct Registry {
    pub attributes: u8,
    /// Wire order, not raw-ID order: namespace grouping can move IDs backwards.
    pub entries: Vec<(String, i32)>,
}
pub type RegistryMap = BTreeMap<String, Registry>;

fn invalid() -> Error {
    Error::new("Invalid or oversized Fabric configuration payload")
}
fn part(s: &str, path: bool) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || b"_.-".contains(&b)
                || (path && b == b'/')
        })
}
fn identifier(s: &str) -> bool {
    let (ns, path) = s.split_once(':').unwrap_or(("minecraft", s));
    part(ns, false) && part(path, true)
}
fn registration(data: &[u8]) -> Result<BTreeSet<String>> {
    if data.len() > 8192 * 129 || data.split(|b| *b == 0).count() > 8192 {
        return Err(invalid());
    }
    Ok(data
        .split(|b| *b == 0)
        .filter_map(|b| std::str::from_utf8(b).ok())
        .filter(|s| s.len() <= 128 && identifier(s))
        .map(|s| {
            if s.contains(':') {
                s.into()
            } else {
                format!("minecraft:{s}")
            }
        })
        .collect())
}
fn string(r: &mut Reader<'_>) -> Result<String> {
    let n = r.count(256)?;
    std::str::from_utf8(r.bytes(n)?)
        .map(str::to_owned)
        .map_err(|_| invalid())
}
fn namespace(r: &mut Reader<'_>) -> Result<String> {
    let s = string(r)?;
    if s.is_empty() {
        return Ok("minecraft".into());
    }
    if !part(&s, false) {
        return Err(invalid());
    }
    Ok(s)
}
fn name(r: &mut Reader<'_>, ns: &str) -> Result<String> {
    let path = string(r)?;
    if !part(&path, true) {
        return Err(invalid());
    }
    Ok(format!("{ns}:{path}"))
}

/// `RegistrySyncPayload.read`: `last` persists across entry namespaces, and
/// differences are signed Minecraft VarInts, not ZigZag. Independent global
/// byte/entry/registry caps bound both allocation and work on hostile input.
pub fn decode(data: &[u8]) -> Result<RegistryMap> {
    if data.len() > MAX_BYTES {
        return Err(invalid());
    }
    let mut r = Reader::new(data);
    let mut map = RegistryMap::new();
    let mut total = 0;
    for _ in 0..r.count(4096)? {
        let ns = namespace(&mut r)?;
        for _ in 0..r.count(4096)? {
            if map.len() >= 4096 {
                return Err(invalid());
            }
            let id = name(&mut r, &ns)?;
            let attributes = r.u8()?;
            let mut entries = Vec::new();
            let mut names = BTreeSet::new();
            let mut ids = BTreeSet::new();
            let mut last = 0i32;
            for _ in 0..r.count(4096)? {
                let ns = namespace(&mut r)?;
                for _ in 0..r.count(MAX_ENTRIES)? {
                    let first = last.checked_add(r.varint()?).ok_or_else(invalid)?;
                    let len = r.count(MAX_ENTRIES - total)?;
                    if len == 0 || first < 0 {
                        return Err(invalid());
                    }
                    last = first.checked_add(len as i32 - 1).ok_or_else(invalid)?;
                    for raw in first..=last {
                        let entry = name(&mut r, &ns)?;
                        if !names.insert(entry.clone()) || !ids.insert(raw) {
                            return Err(invalid());
                        }
                        entries.push((entry, raw));
                    }
                    total += len;
                }
            }
            if map
                .insert(
                    id,
                    Registry {
                        attributes,
                        entries,
                    },
                )
                .is_some()
            {
                return Err(invalid());
            }
        }
    }
    if r.remaining() != 0 {
        return Err(invalid());
    }
    Ok(map)
}

/// Fresh per configuration, including reconfiguration; no global foreign state.
#[derive(Default)]
pub struct Handshake {
    enabled: bool,
    registered: bool,
    pod: bool,
    server_channels: BTreeSet<String>,
    map: Option<std::sync::Arc<RegistryMap>>,
}
impl Handshake {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }
    pub fn with_pod(mut self, enabled: bool) -> Self {
        self.pod = enabled;
        self
    }
    pub fn voicechat(&self) -> bool {
        self.enabled
            && self
                .server_channels
                .iter()
                .any(|c| c.starts_with("voicechat:"))
    }
    pub fn into_map(self) -> Option<std::sync::Arc<RegistryMap>> {
        self.map
    }

    /// A reply is sent synchronously before reading the next packet. In particular,
    /// `ClientConfigurationNetworkAddon.onServerReady` advertises before pong.
    pub fn receive(&mut self, packet: &cfg::Clientbound) -> Result<Option<cfg::Serverbound>> {
        if !self.enabled {
            return Ok(None);
        }
        let cfg::Clientbound::CustomPayload(p) = packet else {
            return Ok(match packet {
                cfg::Clientbound::Ping(p) => Some(cfg::Serverbound::Pong(p.clone())),
                _ => None,
            });
        };
        let (channel, data) = match p.channel.as_str() {
            "minecraft:register" | "minecraft:unregister" => {
                let channels = registration(&p.data)?;
                if p.channel == "minecraft:unregister" {
                    self.server_channels.retain(|c| !channels.contains(c));
                    return Ok(None);
                }
                self.server_channels.extend(channels);
                if self.registered {
                    return Ok(None);
                }
                self.registered = true;
                let mut receivers = RECEIVERS.to_vec();
                if self.pod {
                    receivers.extend_from_slice(b"\0pod:rsnap/part\0pod:assets/part");
                }
                ("minecraft:register", receivers)
            }
            "fabric:split" => {
                return Err(Error::new("Fabric split payloads are not supported yet"));
            }
            SYNC => {
                if !self.registered || self.map.is_some() {
                    return Err(invalid());
                }
                let map = decode(&p.data)?;
                let entries: usize = map.values().map(|r| r.entries.len()).sum();
                let namespaces: BTreeSet<_> = map
                    .values()
                    .flat_map(|r| &r.entries)
                    .filter_map(|(id, _)| id.split_once(':').map(|(ns, _)| ns))
                    .collect();
                log::info!(
                    "Fabric registry sync: {} registries, {entries} entries, namespaces {namespaces:?}",
                    map.len()
                );
                self.map = Some(std::sync::Arc::new(map));
                ("fabric:registry/sync/complete", Vec::new())
            }
            "c:version" => {
                let mut r = Reader::new(&p.data);
                let mut supported = false;
                for _ in 0..r.count(64)? {
                    supported |= r.varint()? == 1;
                }
                if r.remaining() != 0 || !supported {
                    return Err(Error::new(
                        "Unsupported Fabric c:version (requires version 1)",
                    ));
                }
                ("c:version", vec![1, 1])
            }
            _ => return Ok(None),
        };
        Ok(Some(cfg::Serverbound::CustomPayload(cfg::CustomPayload {
            channel: channel.into(),
            data,
        })))
    }
}

/// Ordered transport state, separate from container validation and profile installation.
#[derive(Default)]
pub struct PodParts {
    next: usize,
    count: usize,
    total: usize,
    bytes: Vec<u8>,
    complete: bool,
}
impl PodParts {
    pub fn pending(&self) -> bool {
        self.next != 0 && !self.complete
    }
    pub fn receive(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>> {
        self.receive_bounded(data, 64 << 20)
    }
    pub fn receive_assets(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>> {
        // pod-bridge caps the archive at 64 MiB: models, textures, then whole mods' sounds.
        self.receive_bounded(data, 64 << 20)
    }
    fn receive_bounded(&mut self, data: &[u8], limit: usize) -> Result<Option<Vec<u8>>> {
        let invalid = || Error::new("PoD: invalid, out-of-order or oversized part");
        if data.len() < 12 || self.complete {
            return Err(invalid());
        }
        let field =
            |offset| u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let (index, count, total) = (field(0), field(4), field(8));
        if total == 0
            || total > limit
            || count != total.div_ceil(32768)
            || index != self.next
            || index >= count
            || (index != 0 && (count != self.count || total != self.total))
        {
            return Err(invalid());
        }
        let expected = (total - index * 32768).min(32768);
        if data.len() - 12 != expected {
            return Err(invalid());
        }
        self.count = count;
        self.total = total;
        self.bytes.extend_from_slice(&data[12..]);
        self.next += 1;
        self.complete = self.next == count;
        Ok(self.complete.then(|| std::mem::take(&mut self.bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buf::Writer, packets::configuration::Ping};

    fn payload(channel: &str, data: &[u8]) -> cfg::Clientbound {
        cfg::Clientbound::CustomPayload(cfg::CustomPayload {
            channel: channel.into(),
            data: data.into(),
        })
    }
    fn response(state: &mut Handshake, channel: &str, data: &[u8]) -> cfg::CustomPayload {
        let Some(cfg::Serverbound::CustomPayload(p)) =
            state.receive(&payload(channel, data)).unwrap()
        else {
            panic!("missing reply")
        };
        p
    }
    fn fixture() -> Vec<u8> {
        b"\x01\x00\x01\x05block\x00\x02\x00\x01\x00\x01\x03air\x07podtest\x01\x04\x02\x04lamp\x08lamp_alt".to_vec()
    }
    fn pod_part(index: u32, count: u32, total: u32, size: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        for n in [index, count, total] {
            bytes.extend_from_slice(&n.to_be_bytes());
        }
        bytes.resize(12 + size, 42);
        bytes
    }
    #[test]
    fn pod_registration_flag() {
        for (fabric, pod, expected) in [
            (true, true, true),
            (true, false, false),
            (false, true, false),
        ] {
            let mut state = Handshake::new(fabric).with_pod(pod);
            let reply = state
                .receive(&payload("minecraft:register", b"pod:rsnap/ack"))
                .unwrap();
            let advertised = match reply {
                Some(cfg::Serverbound::CustomPayload(p)) => {
                    registration(&p.data).unwrap().contains("pod:rsnap/part")
                }
                _ => false,
            };
            assert_eq!(advertised, expected);
        }
    }
    #[test]
    fn pod_reassembly_and_single_completion() {
        let mut state = PodParts::default();
        assert!(
            state
                .receive(&pod_part(0, 2, 32769, 32768))
                .unwrap()
                .is_none()
        );
        assert!(state.pending());
        let bytes = state.receive(&pod_part(1, 2, 32769, 1)).unwrap().unwrap();
        assert_eq!(bytes, vec![42; 32769]);
        assert!(!state.pending());
        assert!(state.receive(&pod_part(1, 2, 32769, 1)).is_err());
    }
    #[test]
    fn assets_reassembly_budget_and_order() {
        let mut state = PodParts::default();
        assert!(
            state
                .receive_assets(&pod_part(0, 2, 32769, 32768))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            state
                .receive_assets(&pod_part(1, 2, 32769, 1))
                .unwrap()
                .unwrap(),
            vec![42; 32769]
        );
        assert!(state.receive_assets(&pod_part(1, 2, 32769, 1)).is_err());
        assert!(
            PodParts::default()
                .receive_assets(&pod_part(0, 2049, (64 << 20) + 1, 32768))
                .is_err()
        );
        assert!(
            PodParts::default()
                .receive_assets(&pod_part(1, 2, 32769, 1))
                .is_err()
        );
    }
    #[test]
    fn pod_rejects_invalid_parts() {
        for bytes in [
            pod_part(1, 2, 32769, 1),
            pod_part(0, 3, 32769, 32768),
            pod_part(0, 1, 1, 2),
            pod_part(0, 2049, (64 << 20) + 1, 1),
            pod_part(0, 0, 0, 0),
            vec![0; 11],
        ] {
            assert!(PodParts::default().receive(&bytes).is_err());
        }
        for bytes in [
            pod_part(1, 3, 65537, 32768),
            pod_part(0, 2, 32769, 32768),
            pod_part(1, 2, 32770, 2),
        ] {
            let mut state = PodParts::default();
            state.receive(&pod_part(0, 2, 32769, 32768)).unwrap();
            assert!(state.receive(&bytes).is_err());
        }
    }
    #[test]
    fn fabric_registration_order_and_flag() {
        let mut state = Handshake::new(true);
        let p = response(
            &mut state,
            "minecraft:register",
            b"c:version\0BAD:id\0fabric:registry/sync/complete",
        );
        assert_eq!(p.channel, "minecraft:register");
        assert_eq!(p.data, RECEIVERS);
        assert_eq!(state.server_channels.len(), 2);
        assert!(registration(&p.data).unwrap().contains(SYNC));
        let ping = cfg::Clientbound::Ping(Ping { id: 0x00FAB71C });
        assert!(
            matches!(state.receive(&ping).unwrap(), Some(cfg::Serverbound::Pong(p)) if p.id == 0x00FAB71C)
        );
        assert!(
            state
                .receive(&payload("minecraft:register", b"c:version"))
                .unwrap()
                .is_none()
        );
        let mut off = Handshake::new(false);
        for p in [
            payload("minecraft:register", b"x:y"),
            payload(SYNC, &fixture()),
            payload("fabric:split", &[]),
            ping,
        ] {
            assert!(off.receive(&p).unwrap().is_none());
        }
        assert!(off.into_map().is_none());
    }
    #[test]
    fn fabric_map_and_completion() {
        let bytes = fixture();
        let map = decode(&bytes).unwrap();
        let block = &map["minecraft:block"];
        assert_eq!(block.attributes, 0);
        assert_eq!(
            block.entries,
            [
                ("minecraft:air".into(), 0),
                ("podtest:lamp".into(), 4),
                ("podtest:lamp_alt".into(), 5)
            ]
        );
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end]).is_err());
        }
        let mut state = Handshake::new(true);
        response(&mut state, "minecraft:register", b"x:y");
        let ack = response(&mut state, SYNC, &bytes);
        assert_eq!(ack.channel, "fabric:registry/sync/complete");
        assert!(ack.data.is_empty());
        assert!(state.receive(&payload(SYNC, &bytes)).is_err());
        assert_eq!(state.into_map().unwrap().len(), 1);
    }
    #[test]
    fn fabric_negative_difference_and_duplicates() {
        let mut w = Writer::new();
        w.varint(1);
        w.string("");
        w.varint(1);
        w.string("block");
        w.u8(0x81);
        w.varint(2);
        w.string("");
        w.varint(1);
        w.varint(4);
        w.varint(1);
        w.string("stone");
        w.string("mod");
        w.varint(1);
        w.varint(-3);
        w.varint(2);
        w.string("a");
        w.string("b");
        let map = decode(&w.data).unwrap();
        assert_eq!(map["minecraft:block"].attributes, 0x81);
        assert_eq!(
            map["minecraft:block"].entries[1..],
            [("mod:a".into(), 1), ("mod:b".into(), 2)]
        );
        // Independent malformed fixtures: same raw ID, same name, negative ID, overflow.
        for (diff, path) in [(0, "other"), (1, "air"), (-1, "other"), (i32::MAX, "other")] {
            let mut w = Writer::new();
            w.varint(1);
            w.string("");
            w.varint(1);
            w.string("block");
            w.u8(0);
            w.varint(1);
            w.string("");
            w.varint(2);
            w.varint(0);
            w.varint(1);
            w.string("air");
            w.varint(diff);
            w.varint(2);
            w.string(path);
            w.string("end");
            assert!(decode(&w.data).is_err());
        }
        assert!(decode(&[0, 0]).is_err());
        assert!(decode(&[255, 255, 255, 255, 7]).is_err());
    }
    #[test]
    fn fabric_adapter_preserves_raw_body_and_identifier() {
        use crate::{
            State,
            adapter::{ServerVersion, session::Session},
        };
        let session = Session::new(ServerVersion::V1_21_11);
        let mut intention = Writer::new();
        intention.varint(0);
        intention.varint(crate::PROTOCOL_VERSION);
        intention.string("localhost");
        intention.u16(25565);
        intention.varint(2);
        session.serverbound(&intention.data).unwrap();
        let mut login = Writer::new();
        login.uuid(0);
        login.string("test");
        login.varint(0);
        session.clientbound(2, login.data).unwrap();
        assert_eq!(session.reading(), State::Configuration);
        let mut body = Writer::new();
        body.string(SYNC);
        body.data.extend(fixture());
        let (id, raw) = session.clientbound(1, body.data).unwrap().unwrap();
        let cfg::Clientbound::CustomPayload(p) = cfg::Clientbound::decode(id, &raw).unwrap() else {
            panic!("expected custom payload")
        };
        assert_eq!(p.channel, SYNC);
        assert_eq!(p.data, fixture());
        let mut duplicate = fixture();
        duplicate[2] = 2;
        duplicate.extend_from_slice(&fixture()[3..]);
        assert!(decode(&duplicate).is_err());
    }
    #[test]
    fn fabric_version_and_split() {
        let mut state = Handshake::new(true);
        assert_eq!(response(&mut state, "c:version", &[2, 2, 1]).data, [1, 1]);
        for bytes in [&[1, 2][..], &[1][..], &[1, 1, 0][..]] {
            assert!(state.receive(&payload("c:version", bytes)).is_err());
        }
        assert!(
            state
                .receive(&payload("fabric:split", &[]))
                .unwrap_err()
                .to_string()
                .contains("Fabric split payloads are not supported yet")
        );
    }
}
