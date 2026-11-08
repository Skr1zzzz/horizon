//! Server-advertised Liminal Worlds client atmosphere.
// ported from Liminal Worlds (MIT, ours)
use crate::{
    environment::Sky,
    particles::{ParticleEngine, ParticleOptions, ParticleType},
};
use horizon_core::random::RandomSource;
use horizon_protocol::registry::Dynamic;

pub const VOID: &str = "liminalworlds:void";

pub fn detected(registries: &Dynamic) -> bool {
    ["minecraft:dimension_type", "minecraft:dimension"]
        .iter()
        .any(|registry| registries.id(registry, VOID).is_some())
}

pub fn factors(x: i32, z: i32) -> (f32, f32) {
    let distance = i64::from(x).abs().max(i64::from(z).abs());
    if distance >= 100_000 {
        return (1.0, 1.0);
    }
    let step = ((distance - 25_000).max(0) / 1000) as f32;
    (
        (step * 0.011).min(1.0),
        ((step - 35.0) / 40.0).clamp(0.0, 1.0),
    )
}

pub fn sky(sky: &mut Sky, darkness: f32) {
    if darkness <= 0.0 {
        return;
    }
    let dim = 1.0 - darkness * 0.92;
    let blend = (darkness * 0.55).min(0.75);
    for (i, (base, scale)) in [(18.0, 10.0), (20.0, 12.0), (32.0, 20.0)]
        .into_iter()
        .enumerate()
    {
        // The mixin operates on packed RGB: preserve both truncation steps.
        let source = ((sky.sky_color[i] * 255.0) as i32 as f32 * dim) as i32;
        let target = (base + darkness * scale) as i32;
        sky.sky_color[i] = ((source as f32 * (1.0 - blend) + target as f32 * blend) as i32)
            .clamp(0, 255) as f32
            / 255.0;
        sky.sunrise_color[i] =
            ((sky.sunrise_color[i] * 255.0) as i32 as f32 * dim * 0.85) as i32 as f32 / 255.0;
    }
    sky.star_brightness *= dim.max(0.08);
}

pub fn fog(color: &mut [f32; 4], factor: f32) {
    for (i, value) in color[..3].iter_mut().enumerate() {
        *value = *value * (1.0 - factor * 0.35) + factor * 0.08 * if i == 2 { 0.9 } else { 1.0 };
    }
}

pub fn portal(level: &str, x: i32, z: i32) -> bool {
    level == VOID
        || (level == "minecraft:the_nether" && i64::from(x).abs().max(i64::from(z).abs()) >= 3125)
}

pub fn portal_ash(engine: &mut ParticleEngine, pos: [i32; 3]) {
    let count = engine.random().next_int_bound(2).expect("positive bound") + 2;
    for _ in 0..count {
        let r = engine.random();
        let p = pos.map(|v| f64::from(v) + r.next_double());
        let v = std::array::from_fn(|_| (r.next_double() - 0.5) * 0.4);
        engine.add(&ParticleOptions::simple(ParticleType::Ash), p, v);
    }
}

pub fn ambient_ash(engine: &mut ParticleEngine, origin: [i32; 3], in_void: bool) {
    for _ in 0..if in_void { 8 } else { 4 } {
        let r = engine.random();
        let p = [
            origin[0] as f64 + 0.5 + (r.next_double() - 0.5) * 16.0,
            origin[1] as f64 + (r.next_double() - 0.5) * 10.0,
            origin[2] as f64 + 0.5 + (r.next_double() - 0.5) * 16.0,
        ];
        engine.add(
            &ParticleOptions::simple(ParticleType::Ash),
            p,
            [0.0, -0.012, 0.0],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn liminal_factors() {
        // Java: steps 0, 5, 35, 55, then the >=100000 void override.
        for (x, dark, fog) in [
            (24000, 0.0, 0.0),
            (30000, 0.055, 0.0),
            (60000, 0.385, 0.0),
            (80000, 0.605, 0.5),
            (101000, 1.0, 1.0),
            (103000, 1.0, 1.0),
        ] {
            for (x, z) in [(x, 0), (0, -x), (-x, x)] {
                let actual = factors(x, z);
                assert!((actual.0 - dark).abs() < 0.000001);
                assert!((actual.1 - fog).abs() < 0.000001);
            }
        }
        assert_eq!(factors(25999, 0), (0.0, 0.0));
        assert_eq!(factors(100000, 0), (1.0, 1.0));
    }
    #[test]
    fn liminal_detection() {
        let mut r = Dynamic::default();
        assert!(!detected(&r));
        r.registries.insert(
            "minecraft:dimension_type".into(),
            vec![
                ("minecraft:overworld".into(), None),
                ("other:void".into(), None),
            ],
        );
        assert!(!detected(&r));
        for registry in ["minecraft:dimension_type", "minecraft:dimension"] {
            let mut r = r.clone();
            r.registries
                .insert(registry.into(), vec![(VOID.into(), None)]);
            assert!(detected(&r));
        }
        r.registries
            .insert("minecraft:biome".into(), vec![(VOID.into(), None)]);
        assert!(!detected(&r));
    }
}
