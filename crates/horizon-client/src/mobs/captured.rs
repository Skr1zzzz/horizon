//! PoD captures are already in feet-relative block coordinates, not ModelPart pixels.
use super::{Mob, render::Atlas};
use crate::mesher::Vertex;
use serde_json::Value;
use std::{collections::HashMap, path::Path};

pub struct Batch {
    texture: String,
    translucent: bool,
    quads: Vec<[f32; 20]>,
}
#[derive(Default)]
pub struct Captures(pub HashMap<String, HashMap<String, Vec<Batch>>>);
impl Captures {
    pub fn parse(bytes: &[u8]) -> Self {
        let mut out = Self::default();
        if bytes.len() > 8 << 20 {
            return out;
        }
        let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
            return out;
        };
        if v["format"] != 1 {
            return out;
        }
        for (id, entity) in v["entities"].as_object().into_iter().flatten() {
            if entity.get("error").is_some() {
                continue;
            }
            let Some(samples) = entity["samples"].as_object() else {
                continue;
            };
            let count: usize = samples
                .values()
                .flat_map(|s| s.as_array().into_iter().flatten())
                .map(|b| b["quads"].as_array().map_or(0, Vec::len))
                .sum();
            if count > 20_000 {
                continue;
            }
            let mut poses = HashMap::new();
            for (name, batches) in samples {
                if name != "idle" && !(0..8).any(|k| name == &format!("walk_{k}")) {
                    continue;
                }
                let mut pose = Vec::new();
                for b in batches.as_array().into_iter().flatten() {
                    let Some(texture) = b["texture"].as_str() else {
                        continue;
                    };
                    let Some((ns, path)) = texture.split_once(':') else {
                        continue;
                    };
                    let Some(path) = path
                        .strip_prefix("textures/")
                        .and_then(|p| p.strip_suffix(".png"))
                    else {
                        continue;
                    };
                    if path.starts_with("atlas/") || texture.contains("..") {
                        continue;
                    }
                    let translucent = match b["layer"].as_str() {
                        Some("cutout") => false,
                        Some("translucent") => true,
                        _ => continue,
                    };
                    let quads = b["quads"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|q| {
                            let a = q.as_array()?;
                            if a.len() != 20 {
                                return None;
                            }
                            let mut quad = [0.0; 20];
                            for (dst, src) in quad.iter_mut().zip(a) {
                                *dst = src.as_f64()? as f32;
                                if !dst.is_finite() || dst.abs() > 4096.0 {
                                    return None;
                                }
                            }
                            Some(quad)
                        })
                        .collect::<Vec<_>>();
                    if !quads.is_empty() {
                        let texture = if ns == "minecraft" {
                            path.into()
                        } else {
                            format!("{ns}:{path}")
                        };
                        pose.push(Batch {
                            texture,
                            translucent,
                            quads,
                        });
                    }
                }
                if !pose.is_empty() {
                    poses.insert(name.clone(), pose);
                }
            }
            if poses.contains_key("idle") {
                out.0.insert(id.clone(), poses);
            }
        }
        out
    }
    pub fn load(root: &Path) -> Self {
        let mut out = Self::default();
        for ns in std::fs::read_dir(root.join("assets"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = ns.path().join("pod/entities.json");
            if std::fs::metadata(&path).is_ok_and(|m| m.len() <= 8 << 20)
                && let Ok(bytes) = std::fs::read(path)
            {
                out.0.extend(Self::parse(&bytes).0);
            }
        }
        out
    }
    pub fn draw(
        &self,
        id: &str,
        m: &Mob,
        alpha: f32,
        camera: glam::DVec3,
        atlas: Atlas,
        light: u32,
        out: &mut Vec<Vertex>,
    ) -> bool {
        let Some(samples) = self.0.get(id) else {
            return false;
        };
        if m.invisible {
            return true;
        }
        let speed = m.previous_limb_speed + (m.limb_speed - m.previous_limb_speed) * alpha;
        let phase = m.limb_position - m.limb_speed * (1.0 - alpha);
        let key = sample(speed, phase);
        let batches = samples.get(&key).unwrap_or(&samples["idle"]);
        let a = f64::from(alpha);
        let position = glam::DVec3::new(
            m.previous.x + (m.position.x - m.previous.x) * a,
            m.previous.y + (m.position.y - m.previous.y) * a,
            m.previous.z + (m.position.z - m.previous.z) * a,
        ) - camera;
        let yaw =
            m.previous_body_yaw + super::living::wrap(m.body_yaw - m.previous_body_yaw) * alpha;
        let matrix = glam::Mat4::from_translation(position.as_vec3())
            * glam::Mat4::from_rotation_y(-yaw.to_radians());
        for batch in batches {
            let Some(uv) = atlas(&batch.texture) else {
                continue;
            };
            let layer = super::render::body_overlay(m, alpha)
                | if batch.translucent {
                    crate::gfx::OIT_ENTITY
                } else {
                    0
                };
            for q in &batch.quads {
                let point = |i| matrix.transform_point3(glam::Vec3::new(q[i], q[i + 1], q[i + 2]));
                let n = (point(5) - point(0))
                    .cross(point(10) - point(0))
                    .normalize_or_zero();
                let normal = [
                    (n.x * 127.0) as i8,
                    (n.y * 127.0) as i8,
                    (n.z * 127.0) as i8,
                    0,
                ];
                for i in [0, 5, 10, 15] {
                    out.push(Vertex {
                        pos: point(i).to_array(),
                        uv: [
                            uv[0] + (uv[2] - uv[0]) * q[i + 3],
                            uv[1] + (uv[3] - uv[1]) * q[i + 4],
                        ],
                        normal,
                        layer,
                        color: 0xffff_ffff,
                        light,
                        glint_uv: [0.0; 2],
                    });
                }
            }
        }
        true
    }
}
fn sample(speed: f32, phase: f32) -> String {
    if speed > 0.05 && phase.is_finite() {
        format!(
            "walk_{}",
            ((phase.rem_euclid(std::f32::consts::TAU) * 8.0 / std::f32::consts::TAU).round()
                as usize)
                % 8
        )
    } else {
        "idle".into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pod_captured_validation() {
        let batch = serde_json::json!({"texture":"m:textures/entity/a.png","layer":"cutout","quads":vec![[0;20]]});
        let mut atlas = batch.clone();
        atlas["texture"] = "minecraft:textures/atlas/blocks.png".into();
        let mut large = batch.clone();
        large["quads"] = serde_json::json!(vec![[0; 20]; 20001]);
        let bytes = serde_json::to_vec(&serde_json::json!({"format":1,"entities":{
            "m:a":{"samples":{"idle":[batch,atlas]}},"m:error":{"error":"failed"},
            "m:large":{"samples":{"idle":[large]}}}}))
        .unwrap();
        let c = Captures::parse(&bytes);
        assert_eq!(c.0.len(), 1);
        assert_eq!(c.0["m:a"]["idle"].len(), 1);
        assert_eq!(c.0["m:a"]["idle"][0].quads.len(), 1);
        assert!(Captures::parse(br#"{"format":2}"#).0.is_empty());
    }
    #[test]
    fn pod_captured_phase() {
        assert_eq!(sample(0.0, 1.0), "idle");
        for k in 0..8 {
            assert_eq!(
                sample(1.0, k as f32 * std::f32::consts::TAU / 8.0),
                format!("walk_{k}")
            );
        }
        assert_eq!(sample(1.0, -std::f32::consts::TAU / 8.0), "walk_7");
        assert_eq!(sample(1.0, std::f32::consts::TAU - 0.01), "walk_0");
    }
}
