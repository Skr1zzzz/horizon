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
// Revisioned independently of the wire format so cached packages get new adaptations.
const GENERATOR_REVISION: u32 = 3;

fn menus(files: &mut Files, meta: &Value, ns: &str) -> Result<()> {
    for menu in meta["menus"].as_array().into_iter().flatten() {
        let id = menu["id"].as_str().ok_or("PoD: menu missing id")?;
        let (owner, name) = identifier(id)?;
        if owner != ns {
            continue;
        }
        let slots = menu["slots"].as_array();
        if menu["order"] != "container_first"
            || !slots.is_some_and(|s| (1..=256).contains(&s.len()))
            || menu.get("inventory").is_none()
        {
            log::info!(
                "PoD: {id}: skipping unsupported menu order, slot count or missing inventory"
            );
            continue;
        }
        let point = |v: &Value| -> Option<[u32; 2]> {
            let a = v.as_array()?;
            if a.len() != 2 {
                return None;
            }
            let x = a[0].as_u64()?;
            let y = a[1].as_u64()?;
            (x <= 512 && y <= 512).then_some([x as u32, y as u32])
        };
        let Some([x, y]) = point(&menu["inventory"]) else {
            log::warn!("PoD: {id}: invalid inventory coordinates");
            continue;
        };
        let Some(data) = menu["data_slots"].as_u64().filter(|&n| n <= 256) else {
            log::warn!("PoD: {id}: invalid or oversized data slot count");
            continue;
        };
        // Off-panel slots are parked by mods, not clickable screen positions.
        let slots: Vec<_> = slots.unwrap().iter().map(point).collect();
        let (mut w, mut h) = (176.max(x + 162), (y + 82).min(512).max(y + 76));
        for &[x, y] in slots.iter().flatten() {
            w = w.max(x + 16);
            h = h.max(y + 16);
        }
        if w > 512 || h > 512 {
            log::warn!("PoD: {id}: menu does not fit a 512 pixel panel");
            continue;
        }
        files.insert(
            format!("data/{ns}/horizon/menu/{name}.json"),
            serde_json::to_vec(&json!({"slots":slots.len(),"data_slots":data,
                "screen":{"size":[w,h],"slots":slots,"inventory":[x,y]}}))?,
        );
    }
    Ok(())
}

fn adapt_selects(node: &mut Value) -> bool {
    let mut changed = false;
    match node {
        Value::Object(map) => {
            for child in map.values_mut() {
                changed |= adapt_selects(child);
            }
            if map.get("type").and_then(Value::as_str) == Some("minecraft:select")
                && map
                    .get("property")
                    .and_then(Value::as_str)
                    .is_some_and(|p| p.contains(':') && !p.starts_with("minecraft:"))
                && !map.contains_key("fallback")
                && let Some(model) = map
                    .get("cases")
                    .and_then(Value::as_array)
                    .and_then(|cases| cases.first())
                    .and_then(|case| case.get("model"))
                    .cloned()
            {
                map.insert("fallback".into(), model);
                changed = true;
            }
        }
        Value::Array(children) => {
            for child in children {
                changed |= adapt_selects(child);
            }
        }
        _ => {}
    }
    changed
}

fn geometry(files: &Files, id: &str) -> bool {
    let mut id = id.to_owned();
    let mut seen = BTreeSet::new();
    loop {
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", &id));
        let canonical = format!("{ns}:{path}");
        if !seen.insert(canonical) {
            return false;
        }
        let Some(bytes) = files.get(&format!("assets/{ns}/models/{path}.json")) else {
            // Vanilla's root supplies transforms, not any visible faces.
            return ns == "minecraft" && path != "block/block";
        };
        let Ok(model) = serde_json::from_slice::<Value>(bytes) else {
            return false;
        };
        if model.get("elements").is_some() {
            return true;
        }
        let Some(parent) = model["parent"].as_str() else {
            return false;
        };
        id = parent.to_owned();
    }
}

fn item_geometry<'a>(files: &Files, node: &'a Value) -> Option<&'a str> {
    match node["type"].as_str()? {
        "minecraft:model" => node["model"].as_str().filter(|id| geometry(files, id)),
        "minecraft:composite" => node["models"]
            .as_array()?
            .iter()
            .find_map(|child| item_geometry(files, child)),
        "minecraft:condition" => item_geometry(files, &node["on_false"]),
        "minecraft:select" | "minecraft:range_dispatch" => {
            let key = if node["type"] == "minecraft:select" {
                "cases"
            } else {
                "entries"
            };
            let child = node
                .get("fallback")
                .or_else(|| node[key].as_array()?.first()?.get("model"))?;
            item_geometry(files, child)
        }
        _ => None,
    }
}

fn blockstate_models<'a>(node: &'a Value, models: &mut Vec<&'a str>) {
    match node {
        Value::Object(map) => {
            if let Some(id) = map.get("model").and_then(Value::as_str) {
                models.push(id);
            }
            for child in map.values() {
                blockstate_models(child, models);
            }
        }
        Value::Array(children) => {
            for child in children {
                blockstate_models(child, models);
            }
        }
        _ => {}
    }
}

fn adapt(files: &mut Files, snap: &Snapshot, ns: &str) {
    for (path, bytes) in files.iter_mut() {
        if path.starts_with(&format!("assets/{ns}/items/"))
            && path.ends_with(".json")
            && let Ok(mut item) = serde_json::from_slice::<Value>(bytes)
            && adapt_selects(&mut item)
        {
            *bytes = serde_json::to_vec(&item).expect("JSON value serializes");
        }
    }
    for block in &snap.blocks {
        let Some(name) = block.name.strip_prefix(&format!("{ns}:")) else {
            continue;
        };
        let path = format!("assets/{ns}/blockstates/{name}.json");
        let Some(state) = files
            .get(&path)
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
        else {
            continue;
        };
        let mut models = Vec::new();
        blockstate_models(&state, &mut models);
        if models.is_empty() || models.iter().any(|id| geometry(files, id)) {
            continue;
        }
        let Some(item) = files
            .get(&format!("assets/{ns}/items/{name}.json"))
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
        else {
            continue;
        };
        let Some(model) = item_geometry(files, &item["model"]) else {
            continue;
        };
        let facing = snap.states_of_block(block.raw_id).is_some_and(|range| {
            snap.states[range].iter().any(|s| {
                props(&s.properties)
                    .ok()
                    .and_then(|p| p.get("facing").cloned())
                    .is_some_and(|v| ["north", "east", "south", "west"].contains(&v.as_str()))
            })
        });
        let variants = if facing {
            ["north", "east", "south", "west"]
                .into_iter()
                .enumerate()
                .map(|(i, direction)| {
                    (
                        format!("facing={direction}"),
                        json!({"model":model,"y":i * 90}),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        } else {
            BTreeMap::from([("".into(), json!({"model":model}))])
        };
        files.insert(
            path,
            serde_json::to_vec(&json!({"variants":variants})).expect("JSON value serializes"),
        );
        log::info!(
            "PoD: {}: using item geometry for block entity rendering",
            block.name
        );
    }
}

fn fingerprint(rsnap: &[u8], assets: &[u8]) -> String {
    format!(
        "PoD v1 rsnap={:x} assets={:x} gen={GENERATOR_REVISION}",
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
        .map(|b| b.name.as_str())
        .chain(items.iter().map(|id| id.as_str()))
        .chain(
            meta["menus"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["id"].as_str()),
        )
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
        adapt(&mut files, &snap, &ns);
        menus(&mut files, &meta, &ns)?;
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
    fn generated_menu_loads_and_unsupported_menus_are_skipped() {
        let printer = json!({"id":"computercraft:printer","slots":[[8,19],[30,19],[-1,10],[513,0]],
            "inventory":[8,84],"data_slots":3,"order":"container_first"});
        let mut files = Files::new();
        menus(
            &mut files,
            &json!({"menus":[printer.clone()]}),
            "computercraft",
        )
        .unwrap();
        let value: Value =
            serde_json::from_slice(&files["data/computercraft/horizon/menu/printer.json"]).unwrap();
        assert_eq!(
            value,
            json!({"slots":4,"data_slots":3,"screen":{"size":[176,166],
            "slots":[[8,19],[30,19],null,null],"inventory":[8,84]}})
        );
        files.insert(
            "horizon-mod.toml".into(),
            manifest("computercraft", "1.0", "test")
                .unwrap()
                .into_bytes(),
        );
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/pod-menu-test");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("menu.hmod");
        std::fs::write(&path, package(&files).unwrap()).unwrap();
        let manifest = horizon_mods::Package::read(&path).unwrap().manifest;
        let content = horizon_mods::Content::read(&path, &manifest).unwrap();
        assert_eq!(content.menu_types[0].slots, 4);
        assert_eq!(
            content.menu_types[0].screen.as_ref().unwrap().size,
            [176, 166]
        );
        std::fs::remove_file(path).unwrap();
        for (key, replacement) in [
            ("inventory", Value::Null),
            ("order", json!("mixed")),
            ("order", json!("player_first")),
            ("slots", json!([])),
            ("slots", json!(vec![[0, 0]; 257])),
            ("data_slots", json!(257)),
            ("slots", json!([[512, 0]])),
            ("inventory", json!([400, 84])),
        ] {
            let mut bad = printer.clone();
            if replacement.is_null() {
                bad.as_object_mut().unwrap().remove(key);
            } else {
                bad[key] = replacement;
            }
            let mut files = Files::new();
            menus(&mut files, &json!({"menus":[bad]}), "computercraft").unwrap();
            assert!(files.is_empty(), "{key}");
        }
        let mut grown = printer;
        grown["slots"] = json!([[220, 200]]);
        let mut files = Files::new();
        menus(&mut files, &json!({"menus":[grown]}), "computercraft").unwrap();
        let value: Value = serde_json::from_slice(files.values().next().unwrap()).unwrap();
        assert_eq!(value["screen"]["size"], json!([236, 216]));
    }

    fn adaptation_snapshot() -> Snapshot {
        let words = |v: &[u32]| v.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
        let sections = [
            (
                *b"STAT",
                [words(&[0, u32::MAX, 0, 0, 11]), vec![0; 4]].concat(),
            ),
            (*b"SHAP", vec![]),
            (*b"BOXS", vec![]),
            (*b"BLOK", words(&[11, 6, 0, 0])),
            (*b"REGS", words(&[17, 15, 0, 1])),
            (*b"RENT", words(&[11, 6])),
            (*b"STRS", b"facing=easttest:aminecraft:block".to_vec()),
        ];
        let mut bytes = vec![0; 64 + sections.len() * 24];
        bytes[..8].copy_from_slice(b"PODRSNAP");
        bytes[8..12].copy_from_slice(&1_u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&774_u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&7_u32.to_le_bytes());
        for (i, (tag, data)) in sections.iter().enumerate() {
            let at = 64 + i * 24;
            bytes[at..at + 4].copy_from_slice(tag);
            let start = bytes.len() as u64;
            bytes[at + 8..at + 16].copy_from_slice(&start.to_le_bytes());
            bytes[at + 16..at + 24].copy_from_slice(&(data.len() as u64).to_le_bytes());
            bytes.extend_from_slice(data);
            bytes.resize(bytes.len().div_ceil(8) * 8, 0);
        }
        let digest = Sha256::digest(&bytes[64..]);
        bytes[32..64].copy_from_slice(&digest);
        Snapshot::parse(&bytes).unwrap()
    }

    #[test]
    fn foreign_select_adaptation_preserves_other_nodes() {
        let leaf = json!({"type":"minecraft:model","model":"test:block/base"});
        let select = json!({"type":"minecraft:select","property":"mod:prop",
            "cases":[{"when":"off","model":leaf}]});
        let mut vanilla = select.clone();
        vanilla["property"] = json!("minecraft:local_time");
        let mut fallback = select.clone();
        fallback["fallback"] = json!({"type":"minecraft:empty"});
        let mut tree = json!({"type":"minecraft:condition","on_false":select,
            "on_true":{"type":"minecraft:composite","models":[vanilla,fallback]}});
        let original = tree.clone();
        assert!(adapt_selects(&mut tree));
        assert_eq!(tree["on_false"]["fallback"], leaf);
        assert_eq!(tree["on_true"], original["on_true"]);
        assert!(!adapt_selects(&mut tree));
        let mut files = Files::from([("assets/test/items/bad.json".into(), b"not json".to_vec())]);
        let original = files.clone();
        adapt(&mut files, &adaptation_snapshot(), "test");
        assert_eq!(files, original);
    }

    #[test]
    fn block_entity_geometry_adaptation() {
        let state_path = "assets/test/blockstates/a.json";
        let item_path = "assets/test/items/a.json";
        let mut files = Files::from([
            (state_path.into(), serde_json::to_vec(&json!({"variants":{"":{"model":"test:block/particle"}}})).unwrap()),
            ("assets/test/models/block/particle.json".into(), br#"{"parent":"minecraft:block/block","textures":{"particle":"test:a"}}"#.to_vec()),
            ("assets/test/models/block/base.json".into(), br#"{"elements":[]}"#.to_vec()),
            ("assets/test/models/block/body.json".into(), br#"{"parent":"test:block/base"}"#.to_vec()),
            (item_path.into(), serde_json::to_vec(&json!({"model":{"type":"minecraft:composite","models":[
                {"type":"mod:ignored"}, {"type":"minecraft:model","model":"test:block/particle"},
                {"type":"minecraft:condition","on_false":{"type":"minecraft:select","fallback":{
                    "type":"minecraft:range_dispatch","entries":[{"model":{"type":"minecraft:model","model":"test:block/body"}}]
                }}}]}})).unwrap()),
        ]);
        let original = files.clone();
        let mut snap = adaptation_snapshot();
        adapt(&mut files, &snap, "test");
        let state: Value = serde_json::from_slice(&files[state_path]).unwrap();
        assert_eq!(
            state,
            json!({"variants":{
            "facing=north":{"model":"test:block/body","y":0},
            "facing=east":{"model":"test:block/body","y":90},
            "facing=south":{"model":"test:block/body","y":180},
            "facing=west":{"model":"test:block/body","y":270}}})
        );
        snap.states[0].properties = "waterlogged=false".into();
        files = original.clone();
        adapt(&mut files, &snap, "test");
        assert_eq!(
            serde_json::from_slice::<Value>(&files[state_path]).unwrap(),
            json!({"variants":{"":{"model":"test:block/body"}}})
        );
        for model in [
            "test:block/base",
            "test:block/body",
            "minecraft:block/cube_all",
            "block/cube_all",
        ] {
            files = original.clone();
            files.insert(
                state_path.into(),
                serde_json::to_vec(&json!({"multipart":[{"apply":[{"model":model}]}]})).unwrap(),
            );
            let before = files.clone();
            adapt(&mut files, &snap, "test");
            assert_eq!(files, before, "{model}");
        }
        files = original;
        files.remove(item_path);
        let before = files.clone();
        adapt(&mut files, &snap, "test");
        assert_eq!(files, before);
    }

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
        let revised = fingerprint(b"snapshot", b"assets");
        let old = revised.split(" gen=").next().unwrap();
        assert!(old.starts_with("PoD v1 rsnap="));
        assert!(revised.starts_with("PoD v1 rsnap="));
        assert!(!current(old, &revised, true));
        assert!(current(&revised, &revised, true));
        assert!(current("same", "same", true));
        assert!(!current("same", "different", true));
        assert!(!current("same", "same", false));
    }
}
