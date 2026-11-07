//! Startup-only PoD content generation. Foreign archives never extract onto the filesystem.
use horizon_protocol::rsnap::{Snapshot, State};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
    path::Path,
};

/// Live menu order is authoritative, including interleaved player slots.
#[derive(Debug)]
pub struct Menu {
    pub sync_id: i32,
    pub menu: String,
    pub title: String,
    pub slots: Vec<(i32, i32, u8)>,
    pub data_slots: usize,
}
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub slots: Vec<Option<[f32; 2]>>,
}
impl Menu {
    pub fn accepts(&self, pending: Option<i32>, open: Option<(i32, &str)>) -> bool {
        if let Some(id) = pending {
            return id == self.sync_id;
        }
        open.is_none_or(|(id, name)| id == self.sync_id && !name.starts_with("minecraft:"))
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = horizon_protocol::buf::Reader::new(bytes);
        let sync_id = r.varint()?;
        if sync_id <= 0 {
            return Err("PoD: invalid sync id".into());
        }
        let menu = r.string(32767)?;
        let title = r.string(32767)?;
        let count = r.count(512)?;
        let mut slots = Vec::with_capacity(count);
        for _ in 0..count {
            let (x, y, kind) = (r.i32()?, r.i32()?, r.u8()?);
            if kind > 1 {
                return Err("PoD: invalid slot kind".into());
            }
            slots.push((x, y, kind));
        }
        let data_slots = r.count(256)?;
        if r.remaining() != 0 {
            return Err("PoD: trailing menu bytes".into());
        }
        Ok(Self {
            sync_id,
            menu,
            title,
            slots,
            data_slots,
        })
    }
    pub fn internal(&self, slot: usize) -> Option<usize> {
        let kind = self.slots.get(slot)?.2;
        if kind == 0 {
            return Some(10000 + slot);
        }
        let n = self.slots[..slot].iter().filter(|s| s.2 == 1).count();
        match n {
            0..=26 => Some(9 + n),
            27..=35 => Some(n - 27),
            _ => None,
        }
    }
    pub fn index(&self, internal: usize) -> Option<usize> {
        (0..self.slots.len()).find(|&s| self.internal(s) == Some(internal))
    }
    pub fn layout(&self) -> Layout {
        let slots: Vec<_> = self
            .slots
            .iter()
            .map(|&(x, y, _)| {
                ((0..=495).contains(&x) && (0..=495).contains(&y)).then_some([x as f32, y as f32])
            })
            .collect();
        let (mut width, mut height) = (176f32, 32f32);
        for p in slots.iter().flatten() {
            width = width.max(p[0] + 17.0);
            height = height.max(p[1] + 17.0);
        }
        Layout {
            width,
            height,
            slots,
        }
    }
}

type Result<T> = crate::Result<T>;
type Files = BTreeMap<String, Vec<u8>>;
const FILE: usize = 8 << 20;
// pod-bridge sends at most 19999 assets plus its manifest; Create alone ships ~5200.
const ENTRIES: usize = 20_000;
const OCCLUDES: u32 = 1 << 8;
const FULL_CUBE: u32 = 1 << 3;

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
    if domain(
        "level",
        &[
            "0", "1", "10", "11", "12", "13", "14", "15", "2", "3", "4", "5", "6", "7", "8", "9",
        ],
    ) && rows.iter().all(|s| s.flags & (1 << 5) != 0)
    {
        "minecraft:water"
    } else if domains.is_empty() && cube {
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
#[cfg(test)]
fn declaration(
    snap: &Snapshot,
    block: &horizon_protocol::rsnap::Block,
    item: bool,
) -> Result<Value> {
    declaration_with_entity(snap, block, item, &Value::Null)
}
fn declaration_with_entity(
    snap: &Snapshot,
    block: &horizon_protocol::rsnap::Block,
    item: bool,
    entity: &Value,
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
        cube &= s.flags & FULL_CUBE != 0 && boxes == [[0., 0., 0., 1., 1., 1.]];
        collisions.insert(p.values().cloned().collect::<Vec<_>>().join(","), boxes);
    }
    let count = domains
        .values()
        .try_fold(1usize, |n, d| n.checked_mul(d.len()))
        .ok_or("PoD: state overflow")?;
    if count != rows.len() || actual.iter().any(|p| !p.keys().eq(domains.keys())) {
        return Err("PoD: non-Cartesian states".into());
    }
    let chosen = entity_template(entity, &block.name, &domains)
        .unwrap_or_else(|| template(&domains, &rows, cube));
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
    let inherited = [
        "minecraft:oak_log",
        "minecraft:redstone_lamp",
        "minecraft:water",
    ]
    .contains(&chosen)
        || entity_template(entity, &block.name, &domains).is_some();
    if inherited
        && refs
            .iter()
            .map(|s| props(&s.properties))
            .collect::<Result<BTreeSet<_>>>()?
            != actual
    {
        return Err("PoD: template state mismatch".into());
    }
    let mut lights: BTreeSet<_> = rows.iter().map(|s| s.luminance).collect();
    if lights.len() > 1
        && rows.iter().any(|s| {
            !refs.iter().any(|r| {
                (!inherited || props(&r.properties).ok() == props(&s.properties).ok())
                    && r.luminance == s.luminance
            })
        })
    {
        // A definition carries one light level; the server's light data stays authoritative.
        let default = snap.states[block.default_state as usize].luminance;
        log::warn!(
            "PoD: {}: per-state light {lights:?} unrepresentable, using the default state's {default}",
            block.name
        );
        lights = BTreeSet::from([default]);
    }
    let mut out = json!({"template": chosen, "item": item && chosen != "minecraft:water"});
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
    // Vanilla culls a neighbour's face by this block's occlusion shape, which a template can't
    // carry: only a full cube that occludes hides whole faces. Per-state occlusion cannot be
    // expressed either; extra neighbour faces are safer than holes.
    let hides = OCCLUDES | FULL_CUBE;
    if rows.iter().any(|s| s.flags & hides != hides) {
        out["properties"]["occludes"] = json!(false);
    }
    Ok(out)
}
fn assets(bytes: &[u8]) -> Result<(Files, Value)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
    if zip.len() > ENTRIES {
        return Err("PoD: too many asset entries".into());
    }
    let mut files = Files::new();
    let mut total = 0;
    let mut invalid = 0;
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
        // Models may be OBJ (Create's tracks, wheels); sounds and equipment layers load from content mods.
        let allowed = parts[0] == "assets"
            && (parts.len() == 3 && parts[2] == "sounds.json"
                || parts.len() >= 4
                    && [
                        "blockstates",
                        "models",
                        "items",
                        "textures",
                        "lang",
                        "equipment",
                        "sounds",
                    ]
                    .contains(&parts[2])
                    && ["json", "png", "mcmeta", "lang", "obj", "mtl", "ogg"]
                        .iter()
                        .any(|ext| name.ends_with(&format!(".{ext}"))));
        if name != "pod-assets.json" && !allowed {
            continue;
        }
        // Vanilla ignores files that are not resource locations (Create ships lang/lt_LT.json);
        // packaged, they would make the whole generated mod fail to load.
        if allowed && horizon_mods::content::check_path(&name).is_err() {
            invalid += 1;
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
    if invalid > 0 {
        log::warn!("PoD: skipped {invalid} assets whose paths are not resource locations");
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
    // Assets plus a generated block and item definition per content entry.
    if files.len() > 2 * ENTRIES || files.values().map(Vec::len).sum::<usize>() > 64 << 20 {
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
const GENERATOR_REVISION: u32 = 13;

fn entity_template(
    meta: &Value,
    id: &str,
    domains: &BTreeMap<String, BTreeSet<String>>,
) -> Option<&'static str> {
    let wall = domains.contains_key("facing") || id.contains("_wall_");
    match meta["type"].as_str()? {
        "minecraft:sign" | "minecraft:hanging_sign" => {
            let hanging = domains.contains_key("attached")
                || id.ends_with("hanging_sign")
                || meta["type"] == "minecraft:hanging_sign";
            Some(match (hanging, wall) {
                (false, false) => "minecraft:oak_sign",
                (false, true) => "minecraft:oak_wall_sign",
                (true, false) => "minecraft:oak_hanging_sign",
                (true, true) => "minecraft:oak_wall_hanging_sign",
            })
        }
        _ => None,
    }
}

fn entity_texture(files: &Files, meta: &Value, template: &str) -> Option<String> {
    if !template.ends_with("_sign") {
        return None;
    }
    let wood = meta["wood_type"].as_str()?;
    let (ns, name) = identifier(wood).ok()?;
    let folder = if template.ends_with("hanging_sign") {
        "signs/hanging"
    } else {
        "signs"
    };
    files
        .contains_key(&format!("assets/{ns}/textures/entity/{folder}/{name}.png"))
        .then(|| format!("{ns}:entity/{folder}/{name}"))
}

fn fluid_textures(files: &Files, id: &str) -> Option<[String; 2]> {
    let (ns, name) = id.split_once(':')?;
    ["fluid", "block"].into_iter().find_map(|folder| {
        let pair = [
            format!("{ns}:{folder}/{name}_still"),
            format!("{ns}:{folder}/{name}_flow"),
        ];
        pair.iter()
            .all(|id| {
                let (ns, path) = id.split_once(':').unwrap();
                files.contains_key(&format!("assets/{ns}/textures/{path}.png"))
            })
            .then_some(pair)
    })
}

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

/// Mod item model types (`create:model/wrench`) are client code; swap each for the static model the mod
/// ships beside it, or a flat icon of the item texture, as `minecraft:model`. New model files go to `extra`.
fn adapt_foreign_models(
    node: &mut Value,
    files: &Files,
    ns: &str,
    name: &str,
    extra: &mut Files,
) -> bool {
    let Some(kind) = node["type"].as_str().map(str::to_owned) else {
        return false;
    };
    let mut changed = false;
    let mut visit = |child: Option<&mut Value>| {
        if let Some(child) = child {
            changed |= adapt_foreign_models(child, files, ns, name, extra);
        }
    };
    match kind.as_str() {
        "minecraft:composite" => {
            for child in node
                .get_mut("models")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                visit(Some(child));
            }
        }
        "minecraft:condition" => {
            visit(node.get_mut("on_true"));
            visit(node.get_mut("on_false"));
        }
        "minecraft:select" | "minecraft:range_dispatch" => {
            let key = if kind == "minecraft:select" {
                "cases"
            } else {
                "entries"
            };
            for case in node
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                visit(case.get_mut("model"));
            }
            visit(node.get_mut("fallback"));
        }
        t if t.starts_with("minecraft:") || !t.contains(':') => {}
        _ => {
            let model = |id: &str| {
                let (owner, path) = id.split_once(':').unwrap_or(("minecraft", id));
                files.contains_key(&format!("assets/{owner}/models/{path}.json"))
            };
            let id = node["model"]
                .as_str()
                .filter(|id| model(id))
                .map(str::to_owned)
                .or_else(|| {
                    // Mod renderers can name their static model in another field (backpacks use `base`).
                    node.as_object()?.iter().find_map(|(key, value)| {
                        if key == "type" || key == "model" {
                            return None;
                        }
                        value.as_str().filter(|id| model(id)).map(str::to_owned)
                    })
                })
                .or_else(|| {
                    [
                        format!("{ns}:item/{name}/item"),
                        format!("{ns}:item/{name}"),
                    ]
                    .into_iter()
                    .find(|id| model(id))
                })
                .or_else(|| {
                    files
                        .contains_key(&format!("assets/{ns}/textures/item/{name}.png"))
                        .then(|| {
                            extra.insert(
                                format!("assets/{ns}/models/item/{name}.json"),
                                serde_json::to_vec(&json!({"parent":"minecraft:item/generated",
                                    "textures":{"layer0":format!("{ns}:item/{name}")}}))
                                .expect("JSON value serializes"),
                            );
                            format!("{ns}:item/{name}")
                        })
                });
            if let Some(id) = id {
                log::info!("PoD: {ns}:{name}: item model type {kind} drawn as static {id}");
                *node = json!({"type":"minecraft:model","model":id});
                return true;
            }
        }
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
            // Vanilla's block parents have faces (its root only transforms); item/generated is a flat
            // sprite that no block model can use (mod signs' item icons would draw as missing models).
            return ns == "minecraft" && path.starts_with("block/") && path != "block/block";
        };
        let Ok(model) = serde_json::from_slice::<Value>(bytes) else {
            return false;
        };
        // OBJ models (`"loader": "neoforge:obj"`) carry their faces in the .obj file.
        if model.get("elements").is_some()
            || model["loader"]
                .as_str()
                .is_some_and(|l| l.ends_with(":obj") && model["model"].is_string())
        {
            return true;
        }
        let Some(parent) = model["parent"].as_str() else {
            return false;
        };
        id = parent.to_owned();
    }
}

fn tinted_model(files: &Files, id: &str) -> bool {
    let mut id = id.to_owned();
    let mut seen = BTreeSet::new();
    loop {
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", &id));
        if !seen.insert(format!("{ns}:{path}")) {
            return false;
        }
        let Some(model) = files
            .get(&format!("assets/{ns}/models/{path}.json"))
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        else {
            return false;
        };
        // A child's elements replace its parent's, including the parent's tint faces.
        if let Some(elements) = model.get("elements") {
            return elements.as_array().into_iter().flatten().any(|element| {
                element["faces"]
                    .as_object()
                    .into_iter()
                    .flat_map(|faces| faces.values())
                    .any(|face| face["tintindex"].as_i64().is_some_and(|index| index >= 0))
            });
        }
        let Some(parent) = model["parent"].as_str() else {
            return false;
        };
        id = parent.to_owned();
    }
}

fn biome_tint(files: &Files, id: &str, declaration: &mut Value) {
    let Some((ns, name)) = id.split_once(':') else {
        return;
    };
    let Some(state) = files
        .get(&format!("assets/{ns}/blockstates/{name}.json"))
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
    else {
        return;
    };
    let mut models = Vec::new();
    blockstate_models(&state, &mut models);
    if !models.iter().any(|model| tinted_model(files, model)) {
        return;
    }
    // Java colour providers are code, so infer only recognisable biome-coloured families.
    let tint = if name.ends_with("leaves")
        || declaration["template"]
            .as_str()
            .is_some_and(|t| t.ends_with("leaves"))
    {
        "foliage"
    } else if [
        "grass",
        "fern",
        "bush",
        "vine",
        "sprout",
        "clover",
        "sedge",
        "reed",
        "sugar_cane",
    ]
    .iter()
    .any(|word| name.contains(word))
    {
        "grass"
    } else if name.contains("water") {
        "water"
    } else if ["distance", "persistent"]
        .iter()
        .all(|key| declaration["state_properties"].get(key).is_some())
    {
        "foliage"
    } else {
        return;
    };
    declaration["properties"]["biome_tint"] = json!(tint);
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

fn adapt_blockstate(node: &mut Value) -> bool {
    match node {
        Value::Array(entries) => entries
            .iter_mut()
            .fold(false, |changed, entry| adapt_blockstate(entry) | changed),
        Value::Object(entry) if entry.get("model").is_some_and(Value::is_string) => {
            let before = entry.len();
            entry.retain(|key, _| {
                !key.split_once(':')
                    .is_some_and(|(ns, key)| ns != "minecraft" && !ns.is_empty() && key == "type")
            });
            before != entry.len()
        }
        _ => false,
    }
}

fn adapt(files: &mut Files, snap: &Snapshot, ns: &str) {
    let prefix = format!("assets/{ns}/items/");
    let items: Vec<String> = files
        .keys()
        .filter(|p| p.starts_with(&prefix) && p.ends_with(".json"))
        .cloned()
        .collect();
    let mut extra = Files::new();
    for path in items {
        let Ok(mut item) = serde_json::from_slice::<Value>(&files[&path]) else {
            continue;
        };
        let name = &path[prefix.len()..path.len() - ".json".len()];
        // Foreign types first, so a select's copied fallback is already drawable.
        let foreign = item
            .get_mut("model")
            .is_some_and(|model| adapt_foreign_models(model, files, ns, name, &mut extra));
        if foreign | adapt_selects(&mut item) {
            files.insert(
                path.clone(),
                serde_json::to_vec(&item).expect("JSON value serializes"),
            );
        }
    }
    files.extend(extra);
    for block in &snap.blocks {
        let Some(name) = block.name.strip_prefix(&format!("{ns}:")) else {
            continue;
        };
        let path = format!("assets/{ns}/blockstates/{name}.json");
        let Some(mut state) = files
            .get(&path)
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
        else {
            continue;
        };
        let mut changed = false;
        if let Some(variants) = state["variants"].as_object_mut() {
            for entry in variants.values_mut() {
                changed |= adapt_blockstate(entry);
            }
        }
        if let Some(parts) = state["multipart"].as_array_mut() {
            for part in parts {
                changed |= adapt_blockstate(&mut part["apply"]);
            }
        }
        if changed {
            files.insert(
                path.clone(),
                serde_json::to_vec(&state).expect("JSON value serializes"),
            );
        }
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
        let mut skipped = 0;
        let mut standalone = 0;
        for block in &snap.blocks {
            let (owner, name) = identifier(&block.name)?;
            if owner != ns {
                continue;
            }
            // One unrepresentable block must not cost the whole mod its content.
            let mut value = match declaration_with_entity(
                &snap,
                block,
                items.contains(&block.name),
                &meta["block_entities"][&block.name],
            ) {
                Ok(value) => value,
                Err(e) => {
                    log::warn!("PoD: {}: block skipped: {e}", block.name);
                    skipped += 1;
                    continue;
                }
            };
            if let Some(chosen) = value["template"].as_str().filter(|t| t.ends_with("_sign")) {
                if let Some(texture) =
                    entity_texture(&files, &meta["block_entities"][&block.name], chosen)
                {
                    value["block_entity_texture"] = json!(texture);
                }
                if let Some(wood) = meta["block_entities"][&block.name]["wood_type"].as_str()
                    && identifier(wood).is_ok()
                {
                    value["wood_type"] = json!(wood);
                }
                // Particle-only foreign models must not replace the template's sign body.
                files.remove(&format!("assets/{ns}/blockstates/{name}.json"));
            }
            biome_tint(&assets, &block.name, &mut value);
            // FLUID also marks waterlogged solids; only liquid level blocks use these conventions.
            if snap.states_of_block(block.raw_id).is_some_and(|r| {
                snap.states[r].iter().all(|s| {
                    s.flags & (1 << 5) != 0
                        && props(&s.properties).is_ok_and(|p| p.contains_key("level"))
                })
            }) {
                if let Some(pair) = fluid_textures(&files, &block.name) {
                    value["fluid_textures"] = json!(pair);
                }
            }
            files.insert(
                format!("data/{ns}/horizon/block/{name}.json"),
                serde_json::to_vec(&value)?,
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
            "PoD: generated {id}.hmod ({blocks} blocks, {skipped} skipped, {standalone} items) — restart Horizon to see them"
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
    fn sign_templates_and_packaged_textures() {
        for (properties, expected, folder) in [
            ("rotation=0,waterlogged=false", "oak_sign", "signs"),
            ("facing=north,waterlogged=false", "oak_wall_sign", "signs"),
            (
                "attached=false,rotation=0,waterlogged=false",
                "oak_hanging_sign",
                "signs/hanging",
            ),
            (
                "facing=north,waterlogged=false",
                "oak_wall_hanging_sign",
                "signs/hanging",
            ),
        ] {
            let hanging = folder == "signs/hanging";
            let meta = json!({"type":if hanging {"minecraft:hanging_sign"} else {"minecraft:sign"}, "wood_type":"bop:fir"});
            let domains = props(properties)
                .unwrap()
                .into_iter()
                .map(|(k, v)| (k, BTreeSet::from([v])))
                .collect();
            let chosen = entity_template(&meta, "bop:unusual", &domains).unwrap();
            assert_eq!(chosen, format!("minecraft:{expected}"));
            let mut files = Files::new();
            assert_eq!(entity_texture(&files, &meta, chosen), None);
            files.insert(
                format!("assets/bop/textures/entity/{folder}/fir.png"),
                vec![],
            );
            assert_eq!(
                entity_texture(&files, &meta, chosen),
                Some(format!("bop:entity/{folder}/fir"))
            );
        }
    }

    #[test]
    fn tinted_faces_select_biome_provider_through_parents() {
        let mut files = Files::from([
            (
                "assets/test/blockstates/mahogany_leaves.json".into(),
                br#"{"variants":{"":{"model":"test:block/child"}}}"#.to_vec(),
            ),
            (
                "assets/test/models/block/child.json".into(),
                br#"{"parent":"test:block/base"}"#.to_vec(),
            ),
            (
                "assets/test/models/block/base.json".into(),
                br#"{"elements":[{"faces":{"north":{"tintindex":0}}}]}"#.to_vec(),
            ),
        ]);
        let mut value = declaration(
            &declaration_snapshot(),
            &declaration_snapshot().blocks[0],
            true,
        )
        .unwrap();
        biome_tint(&files, "test:mahogany_leaves", &mut value);
        assert_eq!(value["properties"]["biome_tint"], "foliage");
        files.insert(
            "assets/test/models/block/child.json".into(),
            br#"{"parent":"test:block/base","elements":[{"faces":{"north":{"tintindex":-1}}}]}"#
                .to_vec(),
        );
        let mut value = json!({"properties":{}});
        biome_tint(&files, "test:mahogany_leaves", &mut value);
        assert!(value["properties"].get("biome_tint").is_none());
    }

    #[test]
    fn fabric_blockstate_loader_keeps_plain_model_and_rotation() {
        let path = "assets/test/blockstates/a.json";
        let entry = json!({"fabric:type":"test:loader","minecraft:type":"model",
            "model":"minecraft:block/cube_all","y":270,"uvlock":true,"weight":2});
        let mut files = Files::from([(
            path.into(),
            serde_json::to_vec(&json!({
                "variants":{"":entry}, "multipart":[{"when":{"facing":"east"},"apply":[entry]}]
            }))
            .unwrap(),
        )]);
        adapt(&mut files, &adaptation_snapshot(), "test");
        let state: Value = serde_json::from_slice(&files[path]).unwrap();
        let mut expected = entry;
        expected.as_object_mut().unwrap().remove("fabric:type");
        assert_eq!(state["variants"][""], expected);
        assert_eq!(state["multipart"][0]["apply"][0], expected);
        assert_eq!(state["multipart"][0]["when"], json!({"facing":"east"}));
        let mut no_model = json!({"fabric:type":"test:loader"});
        assert!(!adapt_blockstate(&mut no_model));
    }

    #[test]
    fn fluid_sprite_conventions_and_missing_pair() {
        for folder in ["fluid", "block"] {
            let mut files = Files::new();
            files.insert(
                format!("assets/create/textures/{folder}/honey_still.png"),
                vec![],
            );
            assert_eq!(fluid_textures(&files, "create:honey"), None);
            files.insert(
                format!("assets/create/textures/{folder}/honey_flow.png"),
                vec![],
            );
            assert_eq!(
                fluid_textures(&files, "create:honey"),
                Some([
                    format!("create:{folder}/honey_still"),
                    format!("create:{folder}/honey_flow")
                ])
            );
            assert_eq!(fluid_textures(&files, "create:chocolate"), None);
        }
    }

    #[test]
    fn pod_menu_decode_and_window_model() {
        use horizon_protocol::buf::Writer;
        let mut w = Writer::new();
        w.varint(7);
        w.string("mod:menu");
        w.string("{\"text\":\"Title\"}");
        w.varint(3);
        for (x, y, k) in [(8, 20, 0), (240, 180, 1), (-2000, 0, 0)] {
            w.i32(x);
            w.i32(y);
            w.u8(k);
        }
        w.varint(2);
        let bytes = w.data;
        let m = Menu::decode(&bytes).unwrap();
        assert!(m.accepts(Some(7), None), "open_screen then payload");
        assert!(m.accepts(None, None), "payload only");
        assert!(!m.accepts(Some(8), None));
        assert!(!m.accepts(None, Some((7, "minecraft:hopper"))));
        assert_eq!(m.sync_id, 7);
        assert_eq!(m.data_slots, 2);
        let l = m.layout();
        assert_eq!(l.slots.len(), 3);
        assert_eq!((l.width, l.height), (257.0, 197.0));
        assert_eq!(l.slots[2], None);
        assert_eq!(m.internal(1), Some(9));
        assert_eq!(m.index(10002), Some(2));
        for n in 0..bytes.len() {
            assert!(Menu::decode(&bytes[..n]).is_err());
        }
        let mut w = Writer::new();
        w.varint(7);
        w.string("m:a");
        w.string("");
        w.varint(513);
        assert!(Menu::decode(&w.data).is_err());
        let mut w = Writer::new();
        w.varint(7);
        w.string(&"a".repeat(32768));
        assert!(Menu::decode(&w.data).is_err());
    }
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

    #[test]
    fn declaration_preserves_light_and_conservatively_disables_occlusion() {
        for (flags, expected) in [
            ([0, 0], Some(json!(false))),
            ([OCCLUDES | FULL_CUBE, OCCLUDES | FULL_CUBE], None),
            ([OCCLUDES, OCCLUDES], Some(json!(false))),
            ([OCCLUDES, 0], Some(json!(false))),
        ] {
            let mut snap = declaration_snapshot();
            for (state, flags) in snap.states.iter_mut().zip(flags) {
                state.flags = flags;
            }
            let value = declaration(&snap, &snap.blocks[0], true).unwrap();
            assert_eq!(value["template"], "minecraft:stone");
            assert_eq!(value["properties"].get("occludes"), expected.as_ref());
            assert_eq!(value["properties"]["light"], 7);
        }
    }

    #[test]
    fn block_geometry_excludes_flat_item_sprites_and_accepts_obj() {
        let files = Files::from([
            (
                "assets/m/models/item/s.json".into(),
                br#"{"parent":"minecraft:item/generated"}"#.to_vec(),
            ),
            (
                "assets/m/models/block/o.json".into(),
                br#"{"loader":"neoforge:obj","model":"m:models/block/o.obj"}"#.to_vec(),
            ),
            (
                "assets/m/models/block/c.json".into(),
                br#"{"parent":"minecraft:block/cube_all"}"#.to_vec(),
            ),
        ]);
        assert!(!geometry(&files, "m:item/s"));
        assert!(geometry(&files, "m:block/o"));
        assert!(geometry(&files, "m:block/c"));
        assert!(!geometry(&files, "minecraft:block/block"));
    }

    #[test]
    fn foreign_item_model_types_become_static_models() {
        let files = Files::from([
            ("assets/m/models/block/d/item.json".into(), b"{}".to_vec()),
            ("assets/m/models/item/w/item.json".into(), b"{}".to_vec()),
            ("assets/m/models/item/g.json".into(), b"{}".to_vec()),
            ("assets/m/textures/item/t.png".into(), vec![]),
            (
                "assets/travelersbackpack/models/block/standard.json".into(),
                b"{}".to_vec(),
            ),
        ]);
        let mut extra = Files::new();
        let mut run = |node: Value, name: &str| {
            let mut node = node;
            let changed = adapt_foreign_models(&mut node, &files, "m", name, &mut extra);
            (changed, node)
        };
        let static_model = |id: &str| json!({"type":"minecraft:model","model":id});
        let oversized = json!({"type":"m:model/oversized","model":"m:block/d/item","min":[0]});
        assert_eq!(run(oversized, "d"), (true, static_model("m:block/d/item")));
        let backpack = json!({"type":"travelersbackpack:backpack",
            "base":"travelersbackpack:block/standard","tintSources":[]});
        assert_eq!(
            run(backpack, "w"),
            (true, static_model("travelersbackpack:block/standard"))
        );
        // An unrelated string must not prevent the name-based fallback.
        assert_eq!(
            run(json!({"type":"m:model/w","base":"m:block/missing"}), "w"),
            (true, static_model("m:item/w/item"))
        );
        assert_eq!(
            run(json!({"type":"m:model/w"}), "w"),
            (true, static_model("m:item/w/item"))
        );
        assert_eq!(
            run(json!({"type":"m:model/g"}), "g"),
            (true, static_model("m:item/g"))
        );
        let nested = json!({"type":"minecraft:condition","property":"m:p",
            "on_true":{"type":"m:model/t"},"on_false":static_model("m:item/g")});
        let (changed, node) = run(nested, "t");
        assert!(changed);
        assert_eq!(node["on_true"], static_model("m:item/t"));
        assert_eq!(node["on_false"], static_model("m:item/g"));
        let unknown = json!({"type":"m:model/none"});
        assert_eq!(run(unknown.clone(), "none"), (false, unknown));
        let vanilla = static_model("m:item/missing");
        assert_eq!(run(vanilla.clone(), "missing"), (false, vanilla));
        assert!(extra.contains_key("assets/m/models/item/t.json"));
        assert_eq!(extra.len(), 1);
    }

    #[test]
    fn declaration_keeps_default_light_when_per_state_light_is_unrepresentable() {
        let mut snap = declaration_snapshot();
        snap.states[1].luminance = 3;
        let value = declaration(&snap, &snap.blocks[0], true).unwrap();
        assert_eq!(value["properties"]["light"], 7);
    }

    fn adaptation_snapshot() -> Snapshot {
        snapshot_fixture(false)
    }

    fn declaration_snapshot() -> Snapshot {
        snapshot_fixture(true)
    }

    fn snapshot_fixture(declaration: bool) -> Snapshot {
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
        let mut sections = sections;
        if declaration {
            sections[0].1 = [
                words(&[0, 0, 0, 0, 6]),
                vec![7, 0, 0, 0],
                words(&[0, 0, 0, 6, 6]),
                vec![7, 0, 0, 0],
                words(&[1, 0, OCCLUDES, 0, 0]),
                vec![0; 4],
            ]
            .concat();
            sections[1].1 = words(&[0, 0]);
            sections[3].1 = words(&[12, 6, 0, 0, 18, 15, 1, 2]);
            sections[4].1 = words(&[33, 15, 0, 2]);
            sections[5].1 = words(&[12, 6, 18, 15]);
            sections[6].1 = b"mode=amode=btest:aminecraft:stoneminecraft:block".to_vec();
        }
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
            ("assets/test/sounds.json".into(), b"{}".to_vec()),
            ("assets/test/models/block/a.obj".into(), vec![4]),
            ("assets/test/lang/lt_LT.json".into(), b"{}".to_vec()),
            ("assets/test/shaders/a.vsh".into(), vec![5]),
            ("assets/test/ponder/a.nbt".into(), vec![6]),
        ]);
        let bytes = package(&files).unwrap();
        let (allowed, _) = assets(&bytes).unwrap();
        assert_eq!(
            allowed.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "assets/test/models/block/a.obj",
                "assets/test/sounds.json",
                "assets/test/sounds/a.ogg",
                "assets/test/textures/a.png",
            ]
        );
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
