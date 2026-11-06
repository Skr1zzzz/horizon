//! Startup-only PoD content generation. Foreign archives never extract onto the filesystem.
use horizon_protocol::rsnap::{Snapshot, State};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
    path::Path,
};

type Result<T> = crate::Result<T>;
type Files = BTreeMap<String, Vec<u8>>;
const FILE: usize = 8 << 20;

fn safe(path: &str) -> bool {
    !path.contains(['\\', ':', '\0']) && path.split('/').all(|p| !matches!(p, "" | "." | ".."))
}
fn identifier(id: &str) -> Result<(&str, &str)> {
    let (ns, name) = id.split_once(':').ok_or("PoD: invalid identifier")?;
    if ns.is_empty()
        || ns.len() > 64
        || ns.contains("..")
        || !safe(ns)
        || !ns
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
        || name.contains("..")
        || !safe(name)
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_./-".contains(&b))
    {
        return Err("PoD: unsafe identifier".into());
    }
    Ok((ns, name))
}
fn props(text: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for pair in text.split(',').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').ok_or("PoD: invalid properties")?;
        if [k, v].iter().any(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        }) || out.insert(k.into(), v.into()).is_some()
        {
            return Err("PoD: invalid properties".into());
        }
    }
    Ok(out)
}
fn template(
    domains: &BTreeMap<String, BTreeSet<String>>,
    rows: &[&State],
    cube: bool,
) -> &'static str {
    let domain = |key: &str, values: &[&str]| {
        domains.len() == 1
            && domains
                .get(key)
                .is_some_and(|d| d.iter().map(String::as_str).eq(values.iter().copied()))
    };
    if domains.is_empty() && cube {
        "minecraft:iron_block"
    } else if domain("lit", &["false", "true"])
        && rows
            .iter()
            .all(|s| s.luminance == if s.properties == "lit=true" { 15 } else { 0 })
    {
        "minecraft:redstone_lamp"
    } else if domain("axis", &["x", "y", "z"]) {
        "minecraft:oak_log"
    } else {
        "minecraft:stone"
    }
}
fn declaration(
    snap: &Snapshot,
    block: &horizon_protocol::rsnap::Block,
    item: bool,
) -> Result<Value> {
    let range = snap
        .states_of_block(block.raw_id)
        .ok_or("PoD: missing states")?;
    let rows: Vec<_> = snap.states[range].iter().collect();
    if rows.is_empty() {
        return Err("PoD: empty block".into());
    }
    let mut domains: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut actual = BTreeSet::new();
    let mut collisions = BTreeMap::new();
    let mut cube = true;
    for s in &rows {
        let p = props(&s.properties)?;
        for (k, v) in &p {
            domains.entry(k.clone()).or_default().insert(v.clone());
        }
        if !actual.insert(p.clone()) {
            return Err("PoD: duplicate state".into());
        }
        let shape = s
            .shape
            .ok_or("PoD: dynamic/unknown collision unsupported")?;
        let boxes = &snap.boxes[snap.shapes[shape as usize].clone()];
        if s.flags & 3 != 0 || boxes.len() > 16 {
            return Err("PoD: unsupported collision".into());
        }
        cube &= s.flags & 8 != 0 && boxes == [[0., 0., 0., 1., 1., 1.]];
        collisions.insert(p.values().cloned().collect::<Vec<_>>().join(","), boxes);
    }
    let count = domains
        .values()
        .try_fold(1usize, |n, d| n.checked_mul(d.len()))
        .ok_or("PoD: state overflow")?;
    if count != rows.len() || actual.iter().any(|p| !p.keys().eq(domains.keys())) {
        return Err("PoD: non-Cartesian states".into());
    }
    let chosen = template(&domains, &rows, cube);
    if chosen == "minecraft:stone" {
        log::warn!(
            "PoD: {}: stone fallback; only representable light is preserved",
            block.name
        );
    }
    let reference = snap.block_by_name(chosen).ok_or("PoD: missing template")?;
    let refs = snap
        .states_of_block(reference.raw_id)
        .ok_or("PoD: missing template states")?;
    let refs = &snap.states[refs];
    let inherited = chosen == "minecraft:oak_log" || chosen == "minecraft:redstone_lamp";
    if inherited
        && refs
            .iter()
            .map(|s| props(&s.properties))
            .collect::<Result<BTreeSet<_>>>()?
            != actual
    {
        return Err("PoD: template state mismatch".into());
    }
    let lights: BTreeSet<_> = rows.iter().map(|s| s.luminance).collect();
    if lights.len() > 1
        && rows.iter().any(|s| {
            !refs.iter().any(|r| {
                (!inherited || props(&r.properties).ok() == props(&s.properties).ok())
                    && r.luminance == s.luminance
            })
        })
    {
        return Err("PoD: unrepresentable per-state light".into());
    }
    let mut out = json!({"template": chosen, "item": item});
    if !domains.is_empty() {
        out["default_state"] = json!(props(
            &snap.states[block.default_state as usize].properties
        )?);
        if !inherited {
            out["state_properties"] = json!(
                domains
                    .iter()
                    .map(|(k, d)| (
                        k,
                        if d.iter().map(String::as_str).eq(["false", "true"]) {
                            json!({"type":"boolean"})
                        } else {
                            json!({"type":"enum", "values": d})
                        }
                    ))
                    .collect::<BTreeMap<_, _>>()
            );
        }
        out["collision_shapes"] =
            json!({"properties":domains.keys().collect::<Vec<_>>(), "values":collisions});
    } else {
        out["collision_shapes"] = json!({"default":collisions[""]});
    }
    if lights.len() == 1 {
        out["properties"] = json!({"light":lights.first()});
    }
    Ok(out)
}
fn assets(bytes: &[u8]) -> Result<(Files, Value)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    if zip.len() > 4096 {
        return Err("PoD: too many asset entries".into());
    }
    let mut files = Files::new();
    let mut total = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name()?.into_owned();
        if !safe(name.trim_end_matches('/'))
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err("PoD: unsafe asset path".into());
        }
        if entry.is_dir() {
            continue;
        }
        let parts: Vec<_> = name.split('/').collect();
        let allowed = parts.len() >= 4
            && parts[0] == "assets"
            && ["blockstates", "models", "items", "textures", "lang"].contains(&parts[2])
            && ["json", "png", "mcmeta", "lang"]
                .iter()
                .any(|ext| name.ends_with(&format!(".{ext}")));
        if name != "pod-assets.json" && !allowed {
            continue;
        }
        if entry.size() > FILE as u64 {
            return Err("PoD: oversized asset".into());
        }
        let mut data = Vec::new();
        entry
            .by_ref()
            .take(FILE as u64 + 1)
            .read_to_end(&mut data)?;
        total += data.len();
        if data.len() > FILE || total > 64 << 20 || files.insert(name, data).is_some() {
            return Err("PoD: oversized or duplicate assets".into());
        }
    }
    let meta = serde_json::from_slice(
        files
            .remove("pod-assets.json")
            .ok_or("PoD: missing assets metadata")?
            .as_slice(),
    )?;
    Ok((files, meta))
}
pub(super) fn validate_assets(bytes: &[u8]) -> Result<()> {
    assets(bytes).map(|_| ())
}
fn package(files: &Files) -> Result<Vec<u8>> {
    if files.len() > 4096 || files.values().map(Vec::len).sum::<usize>() > 64 << 20 {
        return Err("PoD: oversized package".into());
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in files {
        if !safe(name) || data.len() > FILE {
            return Err("PoD: unsafe package entry".into());
        }
        zip.start_file(name, zip::write::SimpleFileOptions::default())?;
        zip.write_all(data)?;
    }
    Ok(zip.finish()?.into_inner())
}
fn fingerprint(rsnap: &[u8], assets: &[u8]) -> String {
    format!(
        "PoD v1 rsnap={:x} assets={:x}",
        Sha256::digest(rsnap),
        Sha256::digest(assets)
    )
}
fn current(description: &str, fingerprint: &str, loaded: bool) -> bool {
    loaded && description == fingerprint
}

/// Consume a complete pair once, without waiting for a server finish packet.
pub(super) fn ready(
    snapshot: &mut Option<Vec<u8>>,
    assets: &mut Option<Vec<u8>>,
) -> Option<(Vec<u8>, Vec<u8>)> {
    if snapshot.is_some() && assets.is_some() {
        Some((snapshot.take().unwrap(), assets.take().unwrap()))
    } else {
        None
    }
}

fn mod_id(ns: &str) -> String {
    if ns.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        ns.into()
    } else {
        // A digest avoids conflating deco-blocks, deco.blocks and deco_blocks.
        format!("pod_{:x}", Sha256::digest(ns.as_bytes()))[..64].into()
    }
}

fn manifest(ns: &str, version: &str, fingerprint: &str) -> Result<String> {
    let id = mod_id(ns);
    let namespaces = if id == ns { vec![] } else { vec![ns] };
    // JSON quoting keeps untrusted metadata from injecting TOML fields.
    let version = serde_json::to_string(version)?;
    Ok(format!(
        "id = {id:?}\nname = {ns:?}\nversion = {version}\napi = \"0.2\"\nside = \"client\"\ncapabilities = [\"registries\"]\nnamespaces = {namespaces:?}\ndescription = {fingerprint:?}\n"
    ))
}

pub(super) fn install(rsnap: &[u8], archive: &[u8], dir: &Path) -> Result<Vec<String>> {
    let snap = Snapshot::parse(rsnap)?;
    let (assets, meta) = assets(archive)?;
    let fingerprint = fingerprint(rsnap, archive);
    let items: BTreeSet<_> = snap
        .registries
        .iter()
        .find(|(id, _)| id == "minecraft:item")
        .ok_or("PoD: missing item registry")?
        .1
        .iter()
        .collect();
    let mut namespaces = BTreeSet::new();
    for id in snap
        .blocks
        .iter()
        .map(|b| &b.name)
        .chain(items.iter().copied())
    {
        let (ns, _) = identifier(id)?;
        if ns != "minecraft" {
            namespaces.insert(ns.to_owned());
        }
    }
    let loaded = horizon_protocol::mod_list::local();
    let folder = dir.join(crate::mod_list::DIR);
    if std::fs::symlink_metadata(&folder).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("PoD: symlink content folder refused".into());
    }
    let mut messages = Vec::new();
    let mut enabled = crate::mod_list::ModList::load(dir);
    for ns in namespaces {
        let id = mod_id(&ns);
        let fingerprint = format!("{fingerprint} namespace={ns} mod={id}");
        let path = folder.join(format!("{id}.hmod"));
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("PoD: symlink package refused".into());
        }
        let existing = if path.exists() {
            Some(horizon_mods::Package::read(&path)?)
        } else {
            None
        };
        let local = loaded.iter().find(|m| m.id == id);
        if let Some(p) = &existing {
            let same_file = local.is_some_and(|m| {
                m.sha256 == std::fs::read(&path).ok().map(|b| Sha256::digest(b).into())
            });
            if current(&p.manifest.description, &fingerprint, same_file) {
                continue;
            }
            if !p.manifest.description.starts_with("PoD v1 rsnap=") {
                if local.is_some() {
                    continue;
                }
                return Err("PoD: refusing to overwrite an installed mod".into());
            }
        } else if local.is_some() {
            continue;
        }
        if !meta["namespaces"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(&ns)))
        {
            return Err("PoD: namespace missing from assets".into());
        }
        let mut files: Files = assets
            .iter()
            .filter(|(p, _)| p.starts_with(&format!("assets/{ns}/")))
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect();
        let mut blocks = 0;
        let mut standalone = 0;
        for block in &snap.blocks {
            let (owner, name) = identifier(&block.name)?;
            if owner != ns {
                continue;
            }
            files.insert(
                format!("data/{ns}/horizon/block/{name}.json"),
                serde_json::to_vec(&declaration(&snap, block, items.contains(&block.name))?)?,
            );
            blocks += 1;
        }
        for id in &items {
            let (owner, name) = identifier(id)?;
            if owner == ns && snap.block_by_name(id).is_none() {
                log::warn!(
                    "PoD: {id}: standalone item uses approximate minecraft:diamond defaults"
                );
                files.insert(
                    format!("data/{ns}/horizon/item/{name}.json"),
                    br#"{"template":"minecraft:diamond"}"#.to_vec(),
                );
                standalone += 1;
            }
        }
        let version = meta["mods"]
            .as_array()
            .and_then(|mods| {
                mods.iter().find(|m| {
                    m["namespaces"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(&ns)))
                })
            })
            .and_then(|m| m["version"].as_str())
            .unwrap_or("0.0.0");
        files.insert(
            "horizon-mod.toml".into(),
            manifest(&ns, version, &fingerprint)?.into_bytes(),
        );
        let bytes = package(&files)?;
        std::fs::create_dir_all(&folder)?;
        std::fs::write(&path, bytes)?;
        enabled.enabled.insert(id.clone());
        enabled.disabled.remove(&id);
        let message = format!(
            "PoD: generated {id}.hmod ({blocks} blocks, {standalone} items) — restart Horizon to see them"
        );
        log::info!("{message}");
        messages.push(message);
    }
    if !messages.is_empty() {
        enabled.save(dir);
        let saved = crate::mod_list::ModList::load(dir);
        if saved.enabled != enabled.enabled {
            return Err("PoD: could not enable generated content".into());
        }
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vanilla_identifiers_and_foreign_namespace_package() {
        assert!(identifier("deco-blocks:deco_end_portal_frame").is_ok());
        assert!(identifier("laser-eyes:folder/a.b-c_d").is_ok());
        for id in [
            "a..b:c", "A:b", ":b", "a:b..c", "a:b:c", "a:", "a:/b", "a:b//c",
        ] {
            assert!(identifier(id).is_err(), "{id}");
        }
        let ns = "deco-blocks";
        let text = manifest(ns, "1.0", "PoD v1 rsnap=test namespace=deco-blocks").unwrap();
        let manifest = horizon_mods::Manifest::parse(&text).unwrap();
        assert_eq!(manifest.namespaces, [ns]);
        assert_ne!(mod_id(ns), mod_id("deco.blocks"));
        assert_ne!(mod_id(ns), mod_id("deco_blocks"));
        let files = BTreeMap::from([
            ("horizon-mod.toml".into(), text.into_bytes()),
            ("assets/deco-blocks/textures/block/a.png".into(), vec![1, 2]),
            (
                "data/deco-blocks/horizon/block/deco_bedrock.json".into(),
                br#"{"template":"minecraft:stone"}"#.to_vec(),
            ),
        ]);
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/pod-namespace-test");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("deco.hmod");
        std::fs::write(&path, package(&files).unwrap()).unwrap();
        let content = horizon_mods::Content::read(&path, &manifest).unwrap();
        assert_eq!(content.blocks[0].id, "deco-blocks:deco_bedrock");
        assert!(
            content
                .files
                .iter()
                .any(|p| p == "assets/deco-blocks/textures/block/a.png")
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn receipt_is_ready_before_pack_or_finish_and_only_once() {
        for outcome in ["no pack", "accepted", "declined", "failed"] {
            let (mut snapshot, mut assets) = (Some(vec![1]), None);
            assert!(ready(&mut snapshot, &mut assets).is_none());
            assets = Some(vec![2]);
            // The configuration loop checks this before receiving the next
            // packet, even if that packet opens an unanswered pack prompt.
            assert_eq!(
                ready(&mut snapshot, &mut assets),
                Some((vec![1], vec![2])),
                "{outcome}"
            );
            assert!(ready(&mut snapshot, &mut assets).is_none());
        }
    }

    #[test]
    fn template_table() {
        let state = |p: &str, light| State {
            block: 0,
            properties: p.into(),
            shape: Some(0),
            luminance: light,
            opacity: 15,
            flags: 8,
        };
        for (props, cube, expected) in [
            (vec![("", 0)], true, "minecraft:iron_block"),
            (
                vec![("lit=false", 0), ("lit=true", 15)],
                true,
                "minecraft:redstone_lamp",
            ),
            (
                vec![("axis=x", 0), ("axis=y", 0), ("axis=z", 0)],
                true,
                "minecraft:oak_log",
            ),
            (
                vec![("lit=false", 0), ("lit=true", 7)],
                true,
                "minecraft:stone",
            ),
        ] {
            let rows: Vec<_> = props.iter().map(|(p, l)| state(p, *l)).collect();
            let mut domains: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for row in &rows {
                for (k, v) in super::props(&row.properties).unwrap() {
                    domains.entry(k).or_default().insert(v);
                }
            }
            assert_eq!(
                template(&domains, &rows.iter().collect::<Vec<_>>(), cube),
                expected
            );
        }
    }
    #[test]
    fn package_layout_allowlist_and_fingerprint() {
        let files = BTreeMap::from([
            (
                "pod-assets.json".into(),
                br#"{"namespaces":["test"],"mods":[]}"#.to_vec(),
            ),
            ("assets/test/textures/a.png".into(), vec![1, 2]),
            ("assets/test/sounds/a.ogg".into(), vec![3]),
        ]);
        let bytes = package(&files).unwrap();
        let (allowed, _) = assets(&bytes).unwrap();
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed["assets/test/textures/a.png"], [1, 2]);
        assert!(package(&BTreeMap::from([("assets/test/../a.png".into(), vec![])])).is_err());
        assert!(identifier("../test:a").is_err());
        assert!(identifier("test:../a").is_err());
        assert!(current("same", "same", true));
        assert!(!current("same", "different", true));
        assert!(!current("same", "same", false));
    }
}
