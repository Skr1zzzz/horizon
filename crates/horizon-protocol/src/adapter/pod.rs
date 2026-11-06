//! Experimental connection-owned name mappings; never replace protocol layout quirks.
use crate::{Error, Result, registry::Builtin, rsnap::Snapshot};
use horizon_world::{chunk::BlockState, registry::BlockRegistry};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    sync::Arc,
};

fn resolve(blocks: &BlockRegistry, name: &str, properties: &str) -> Option<i32> {
    let mut descriptor = BlockState {
        name: name.into(),
        properties: Default::default(),
    };
    for pair in properties.split(',').filter(|s| !s.is_empty()) {
        let (key, value) = pair.split_once('=')?;
        if descriptor
            .properties
            .insert(key.into(), value.into())
            .is_some()
        {
            return None;
        }
    }
    blocks.id(&descriptor).map(|id| id.0 as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reverse_items_only_include_resolved_names() {
        let blocks = BlockRegistry::from_facts_json(&br#"{"states":[[0,true,false,0,{"id":"minecraft:air"}]],"defaults":{"minecraft:air":0}}"#[..]).unwrap();
        let builtin = Builtin::parse(r#"{"minecraft:item":{"entries":{"minecraft:air":{"protocol_id":0},"minecraft:stone":{"protocol_id":1},"pod:lamp":{"protocol_id":2}}}}"#).unwrap();
        let snapshot = Snapshot {
            protocol: 774,
            blocks: vec![],
            states: vec![],
            registries: vec![(
                "minecraft:item".into(),
                vec![
                    "missing:item".into(),
                    "pod:lamp".into(),
                    "minecraft:air".into(),
                    "minecraft:stone".into(),
                ],
            )],
            shapes: vec![],
            boxes: vec![],
        };
        let profile = Profile::build(&snapshot, &blocks, &builtin);
        assert_eq!(profile.items, [0, 2, 0, 1]);
        assert_eq!(profile.server_items, [(0, 2), (1, 3), (2, 1)].into());
    }

    #[test]
    fn pod_names_require_exact_properties_in_any_order() {
        let registry = BlockRegistry::from_facts_json(&br#"{"states":[[0,true,false,0,{"id":"minecraft:air"}],[0,false,false,0,{"id":"pod:lamp","properties":{"axis":"x","lit":"true"}}]],"defaults":{"minecraft:air":0,"pod:lamp":1}}"#[..]);
        let registry = registry.unwrap();
        assert_eq!(resolve(&registry, "pod:lamp", "lit=true,axis=x"), Some(1));
        assert_eq!(resolve(&registry, "pod:lamp", "axis=x,lit=true"), Some(1));
        assert_eq!(resolve(&registry, "pod:lamp", "lit=true"), None);
        assert_eq!(resolve(&registry, "missing:block", ""), None);
    }
}

#[derive(Debug)]
pub struct Profile {
    pub states: Vec<i32>,
    pub items: Vec<i32>,
    pub local_states: u32,
    pub server_items: BTreeMap<i32, i32>,
    pub server_states: BTreeMap<i32, i32>,
}
impl Profile {
    pub fn load(path: &Path, resources: &Path) -> Result<Arc<Self>> {
        // Bound the read itself, not only the parser's allocation.
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take((64 << 20) + 1)
            .read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes, resources)
    }
    pub fn from_bytes(bytes: &[u8], resources: &Path) -> Result<Arc<Self>> {
        let snapshot = Snapshot::parse(bytes)?;
        if snapshot.protocol != 774 {
            return Err(Error::new("PoD snapshot protocol is not 774"));
        }
        let blocks =
            BlockRegistry::from_facts_json(std::fs::File::open(resources.join("facts.json"))?)?;
        let report =
            Builtin::find(resources).ok_or_else(|| Error::new("PoD: no local item registry"))?;
        let builtin = Builtin::load(&report)?;
        Ok(Arc::new(Self::build(&snapshot, &blocks, &builtin)))
    }
    pub fn build(snapshot: &Snapshot, blocks: &BlockRegistry, builtin: &Builtin) -> Self {
        let air = blocks
            .default_state("minecraft:air")
            .map_or(0, |id| id.0 as i32);
        let mut unknown = 0;
        let mut namespaces = BTreeSet::new();
        let mut server_states = BTreeMap::new();
        let states = snapshot
            .states
            .iter()
            .enumerate()
            .map(|(raw, state)| {
                let block = snapshot.blocks.get(state.block as usize);
                let mapped = block.and_then(|block| {
                    if state.flags & 128 != 0 {
                        return None;
                    }
                    resolve(blocks, &block.name, &state.properties)
                });
                // Unknown entries become air only clientbound; they must not alias real air on writes.
                if let Some(local) = mapped {
                    server_states.entry(local).or_insert(raw as i32);
                }
                mapped.unwrap_or_else(|| {
                    unknown += 1;
                    if let Some(block) = block {
                        namespaces.insert(block.name.split(':').next().unwrap_or("").to_owned());
                    }
                    air
                })
            })
            .collect::<Vec<_>>();
        log::info!(
            "PoD: mapped {} of {} server states, {unknown} unknown (namespaces {namespaces:?})",
            states.len() - unknown,
            states.len()
        );
        let local_items = builtin.get("minecraft:item");
        let mut server_items = BTreeMap::new();
        let items = snapshot
            .registries
            .iter()
            .find(|(name, _)| name == "minecraft:item")
            .map(|(_, names)| {
                names
                    .iter()
                    .enumerate()
                    .map(|(raw, name)| {
                        let local = local_items.and_then(|ids| ids.id(name));
                        if let Some(local) = local {
                            server_items.entry(local).or_insert(raw as i32);
                        }
                        local.unwrap_or(0)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            states,
            items,
            local_states: blocks.len() as u32,
            server_items,
            server_states,
        }
    }
    pub fn state(&self, id: i32) -> i32 {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.states.get(i))
            .copied()
            .unwrap_or(0)
    }
    pub fn bits(&self) -> (u8, u8) {
        let bits = |n: u32| (32 - n.saturating_sub(1).leading_zeros()) as u8;
        (bits(self.states.len() as u32), bits(self.local_states))
    }
}
