//! Experimental connection-owned name mappings; never replace protocol layout quirks.
use crate::{Error, Result, registry::Builtin, rsnap::Snapshot};
use horizon_world::{chunk::BlockState, registry::BlockRegistry};
use std::{collections::BTreeSet, io::Read, path::Path, sync::Arc};

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
        let states = snapshot
            .states
            .iter()
            .map(|state| {
                let block = snapshot.blocks.get(state.block as usize);
                let mapped = block.and_then(|block| {
                    if state.flags & 128 != 0 {
                        return None;
                    }
                    resolve(blocks, &block.name, &state.properties)
                });
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
        let items = snapshot
            .registries
            .iter()
            .find(|(name, _)| name == "minecraft:item")
            .map(|(_, names)| {
                names
                    .iter()
                    .map(|name| local_items.and_then(|ids| ids.id(name)).unwrap_or(0))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            states,
            items,
            local_states: blocks.len() as u32,
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
