//! Session protocol and geometry adapted from laser-eyes (MIT).
use glam::DVec3;
use horizon_protocol::{Result, buf::Reader};

#[derive(Clone, Debug)]
pub struct Config {
    pub range: f64,
    pub damage: f64,
    pub ignite_entities: bool,
    pub ignite_seconds: i32,
    pub ignite_blocks: bool,
    pub break_blocks: bool,
    pub burn_threshold: i32,
    pub color: u32,
    pub eye_offset: i32,
    pub stable: bool,
    pub custom: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            range: 96.0,
            damage: 3.0,
            ignite_entities: true,
            ignite_seconds: 6,
            ignite_blocks: true,
            break_blocks: true,
            burn_threshold: 24,
            color: 0xff3030,
            eye_offset: 0,
            stable: true,
            custom: false,
        }
    }
}
fn end(r: &Reader<'_>) -> Result<()> {
    if r.remaining() == 0 {
        Ok(())
    } else {
        Err(horizon_protocol::Error::new("trailing laser payload bytes"))
    }
}
pub fn sync(bytes: &[u8]) -> Result<(u128, bool)> {
    let mut r = Reader::new(bytes);
    let value = (r.uuid()?, r.bool()?);
    end(&r)?;
    Ok(value)
}
impl Config {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let value = Self {
            range: r.f64()?,
            damage: r.f64()?,
            ignite_entities: r.bool()?,
            ignite_seconds: r.varint()?,
            ignite_blocks: r.bool()?,
            break_blocks: r.bool()?,
            burn_threshold: r.varint()?,
            color: r.u32()?,
            eye_offset: r.i32()?,
            stable: r.bool()?,
            custom: r.bool()?,
        };
        end(&r)?;
        Ok(value)
    }
}
pub fn eye(
    feet: DVec3,
    eye_height: f64,
    height: f64,
    width: f64,
    look: DVec3,
    offset: i32,
) -> DVec3 {
    feet + DVec3::Y
        * (eye_height.clamp(height * 0.35, height * 0.9) + f64::from(offset) / 16.0 - 0.12)
        + look.normalize_or_zero() * (width * 0.06).clamp(0.015, 0.04)
}
/// Java deliberately uses a fixed visual range, not the server's damage range.
pub fn segments(
    eye: DVec3,
    look: DVec3,
    yaw: f64,
    width: f64,
    hit: Option<DVec3>,
) -> Option<[(DVec3, DVec3); 2]> {
    let distance = hit.map_or(96.0, |p| p.distance(eye).min(96.0));
    if distance < 0.2 {
        return None;
    }
    let forward = 0.25f64.min(distance * 0.5);
    let base = eye + look * forward;
    let right = DVec3::new(yaw.to_radians().cos(), 0.0, yaw.to_radians().sin())
        * (width * 0.12).clamp(0.05, 0.1);
    let beam = look * (distance - forward);
    Some([
        (base - right, base - right + beam),
        (base + right, base + right + beam),
    ])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn laser_java_payloads() {
        let mut sync_bytes = (0x00112233445566778899aabbccddeeffu128)
            .to_be_bytes()
            .to_vec();
        sync_bytes.push(1);
        assert_eq!(
            sync(&sync_bytes).unwrap(),
            (0x00112233445566778899aabbccddeeff, true)
        );
        for n in 0..17 {
            assert!(sync(&sync_bytes[..n]).is_err());
        }
        let mut bytes = 96f64.to_be_bytes().to_vec();
        bytes.extend(3f64.to_be_bytes());
        bytes.extend([
            1, 0xac, 2, 0, 1, 24, 0, 255, 48, 48, 255, 255, 255, 254, 1, 0,
        ]);
        let c = Config::decode(&bytes).unwrap();
        assert_eq!(
            (c.range, c.damage, c.ignite_seconds, c.color, c.eye_offset),
            (96.0, 3.0, 300, 0xff3030, -2)
        );
        assert!(c.ignite_entities && !c.ignite_blocks && c.break_blocks && c.stable && !c.custom);
        assert_eq!(c.burn_threshold, 24);
        for n in 0..bytes.len() {
            assert!(Config::decode(&bytes[..n]).is_err());
        }
        bytes.push(0);
        assert!(Config::decode(&bytes).is_err());
    }
    #[test]
    fn laser_parallel_endpoints_and_eye_clamp() {
        let e = eye(DVec3::ZERO, 1.7, 1.8, 0.6, DVec3::Z, 2);
        assert!((e.y - 1.625).abs() < 1e-12);
        assert!((e.z - 0.036).abs() < 1e-12);
        let beams = segments(DVec3::ZERO, DVec3::Z, 0.0, 0.6, Some(DVec3::Z * 5.0)).unwrap();
        assert_eq!(
            beams[0],
            (DVec3::new(-0.072, 0.0, 0.25), DVec3::new(-0.072, 0.0, 5.0))
        );
        assert_eq!(beams[1].1, DVec3::new(0.072, 0.0, 5.0));
        assert_eq!(
            segments(DVec3::ZERO, DVec3::Y, 90.0, 0.6, None).unwrap()[0]
                .1
                .y,
            96.0
        );
        assert!(segments(DVec3::ZERO, DVec3::Z, 0.0, 0.6, Some(DVec3::Z * 0.1)).is_none());
    }
}
