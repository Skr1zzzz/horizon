use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct Local {
    pub volumes: BTreeMap<u128, f32>,
    pub heard: BTreeMap<u128, Instant>,
}
pub fn talking(age: Duration) -> bool {
    age < Duration::from_millis(300)
}
impl Local {
    pub fn gain(&self, id: u128) -> f32 {
        self.volumes.get(&id).copied().unwrap_or(1.0)
    }
    pub fn load(path: &Path) -> Self {
        let mut local = Self::default();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                if let Some((id, gain)) = line.split_once(' ')
                    && let (Ok(id), Ok(gain)) = (u128::from_str_radix(id, 16), gain.parse::<f32>())
                    && gain.is_finite()
                {
                    local.volumes.insert(id, gain.clamp(0.0, 2.0));
                }
            }
        }
        local
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text: String = self
            .volumes
            .iter()
            .map(|(id, g)| format!("{id:032x} {}\n", g.clamp(0.0, 2.0)))
            .collect();
        std::fs::write(path, text)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indicator_expiry() {
        assert!(talking(Duration::from_millis(299)));
        assert!(!talking(Duration::from_millis(300)));
        assert!(!talking(Duration::from_secs(1)));
    }
    #[test]
    fn player_volume_file_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "horizon-voice-volumes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut local = Local::default();
        local.volumes.insert(1, 0.0);
        local.volumes.insert(u128::MAX, 2.0);
        local.save(&path).unwrap();
        let restored = Local::load(&path);
        assert_eq!(restored.volumes, local.volumes);
        assert_eq!(restored.gain(7), 1.0);
        std::fs::remove_file(path).unwrap();
    }
}
