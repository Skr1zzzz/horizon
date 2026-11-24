//! The creaking heart's block entity (`CreakingHeartBlockEntity`): at night
//! an awake heart in a pale oak trunk spawns a creaking that protects it
//! (`SpawnUtil.trySpawnMob`), lets it go by day, when it strays or when a
//! player is stuck in it, and when that creaking is hurt by a player emits
//! trails and spreads resin over the logs. The game and the mob AI probe
//! both tick hearts through this module; the host applies the block changes
//! and plays what it reports.
use super::{Mob, MobEvent, MobType, MobWorld, creaking};
use crate::{collisions::SceneCollisions, containers::CreakingHeart};
use horizon_core::{
    geometry::{Aabb, Vec3},
    random::{LevelRandom, RandomSource},
};
use horizon_world::registry::BlockStateId;
use std::sync::Arc;

/// `Creaking.CREAKING_ORANGE` and `CREAKING_GRAY`.
pub const ORANGE: u32 = 16_545_810;
pub const GRAY: u32 = 6_250_335;
/// `CreakingHeartBlockEntity.DISTANCE_CREAKING_TOO_FAR`.
const TOO_FAR: f64 = 34.0;
/// `Direction.values()`: down, up, north, south, west, east.
const DIRECTIONS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];
const FACE_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// What a heart's tick did, for its host to apply.
#[derive(Clone, Debug)]
pub enum HeartEvent {
    /// `Level.setBlockAndUpdate`.
    SetBlock([i32; 3], BlockStateId),
    /// `Level.playSound(null, ...)`: heard by everyone.
    Sound(&'static str, [f64; 3], f32, f32),
    /// A `TrailParticleOption` sent from `at` towards `target`.
    Trail {
        at: [f64; 3],
        target: [f64; 3],
        color: u32,
        duration: i32,
    },
    /// `updateNeighbourForOutputSignal`: comparators read the new signal.
    OutputChanged([i32; 3]),
    /// `Level.gameEvent(event, pos, Context.of(state))` from the heart's own
    /// block state (a resin clump's `BLOCK_PLACE`).
    BlockGameEvent(&'static str, [i32; 3], BlockStateId),
    /// A mob's own sounds and effects (the spawned creaking's voice, a
    /// creaking falling apart).
    Mob(MobEvent),
}

/// The mobs and players a heart sees and the creakings it spawns.
pub trait HeartHost {
    /// `Level.players()`: where each player in the heart's level stands and
    /// whether it is a spectator. A server has no player of its own: these
    /// are the players it serves.
    fn players(&self) -> &[(Vec3, bool)];
    fn mob(&self, id: u32) -> Option<&Mob>;
    fn mob_mut(&mut self, id: u32) -> Option<&mut Mob>;
    /// The loaded mob with this UUID.
    fn mob_for_uuid(&self, uuid: [i32; 4]) -> Option<u32>;
    fn uuid_for(&mut self, id: u32) -> [i32; 4];
    /// The creaking's entity type.
    fn creaking_type(&self) -> Option<Arc<MobType>>;
    /// A new mob of the type (the next entity id, its random seeded).
    fn create_mob(&mut self, kind: Arc<MobType>, at: Vec3) -> Mob;
    /// `Level.addFreshEntity`.
    fn add_mob(&mut self, m: Mob);
    /// `ServerLevel.isSpawningMonsters` (rules and difficulty).
    fn spawning_monsters(&self) -> bool;
    /// `Player.getEyePosition` of each player in the heart's level that is
    /// not a spectator (`PlayerSensor` lists none in `NEAREST_PLAYERS`).
    fn player_eyes(&self) -> &[[f64; 3]];
}

/// Block reads during a tick see the tick's own changes.
struct Blocks<'a, 'w> {
    w: &'a MobWorld<'w>,
    changed: Vec<([i32; 3], BlockStateId)>,
}
impl Blocks<'_, '_> {
    fn get(&self, p: [i32; 3]) -> BlockStateId {
        self.changed
            .iter()
            .rev()
            .find(|(q, _)| *q == p)
            .map_or_else(|| self.w.world.block(p[0], p[1], p[2]), |(_, s)| *s)
    }
    fn set(&mut self, p: [i32; 3], s: BlockStateId, out: &mut Vec<HeartEvent>) {
        self.changed.push((p, s));
        out.push(HeartEvent::SetBlock(p, s));
    }
    fn name(&self, s: BlockStateId) -> &str {
        &self.w.facts.get(s).name
    }
    fn prop(&self, s: BlockStateId, key: &str) -> Option<String> {
        self.w
            .registry
            .state(s)
            .and_then(|st| st.properties.get(key).cloned())
    }
    fn with_prop(&self, s: BlockStateId, key: &str, value: &str) -> Option<BlockStateId> {
        let mut st = self.w.registry.state(s)?.clone();
        st.properties.insert(key.to_owned(), value.to_owned());
        self.w.registry.resolve(&st).ok()
    }
    fn is_pale_oak_log(&self, p: [i32; 3]) -> bool {
        let s = self.get(p);
        self.w.has_tag("minecraft:pale_oak_logs", self.name(s))
    }
}

/// `EnvironmentAttributes.CREAKING_ACTIVE`: the overworld's night, from
/// 12600 to 23400.
pub fn creaking_active(w: &MobWorld) -> bool {
    w.dimension_type.has_timeline("minecraft:day")
        && (12600..23401).contains(&w.day_time.rem_euclid(24000))
}
/// `CreakingHeartBlock.hasRequiredLogs`: pale oak logs on both ends of the
/// heart's axis, along the same axis.
fn has_required_logs(b: &Blocks, state: BlockStateId, pos: [i32; 3]) -> bool {
    let axis = b.prop(state, "axis").unwrap_or_else(|| "y".into());
    let dirs: [usize; 2] = match axis.as_str() {
        "x" => [4, 5],
        "z" => [2, 3],
        _ => [0, 1],
    };
    dirs.iter().all(|&d| {
        let n = [
            pos[0] + DIRECTIONS[d][0],
            pos[1] + DIRECTIONS[d][1],
            pos[2] + DIRECTIONS[d][2],
        ];
        let s = b.get(n);
        b.w.has_tag("minecraft:pale_oak_logs", b.name(s))
            && b.prop(s, "axis").as_deref() == Some(axis.as_str())
    })
}

/// `Level.getNearestPlayer(x, y, z, 32, false) != null` at the heart's
/// corner: any player in the level within 32 blocks that is not a spectator
/// (`EntitySelector.NO_SPECTATORS`).
fn player_near(host: &impl HeartHost, pos: [i32; 3]) -> bool {
    let (x, y, z) = (f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2]));
    host.players().iter().any(|&(p, spectator)| {
        let (dx, dy, dz) = (p.x - x, p.y - y, p.z - z);
        !spectator && dx * dx + dy * dy + dz * dz < 32.0 * 32.0
    })
}

/// `getCreakingProtector`: the protector's entity, resolving a UUID read
/// from a save (forgotten 30 ticks after the heart came back unresolved).
pub fn protector(heart: &mut CreakingHeart, host: &impl HeartHost) -> Option<u32> {
    let (id, uuid) = heart.creaking?;
    if let Some(id) = id {
        if host.mob(id).is_some_and(|m| !m.removed) {
            return Some(id);
        }
        // setCreakingInfo(uuid)
        heart.creaking = Some((None, uuid));
        heart.ticks_existed = 0;
    }
    if let Some(id) = host.mob_for_uuid(uuid) {
        heart.creaking = Some((Some(id), uuid));
        return Some(id);
    }
    if heart.ticks_existed >= 30 {
        heart.creaking = None;
    }
    None
}
fn distance_to_creaking(heart: &mut CreakingHeart, pos: [i32; 3], host: &impl HeartHost) -> f64 {
    protector(heart, host)
        .and_then(|id| host.mob(id))
        .map_or(0.0, |m| {
            let at = Vec3::new(
                f64::from(pos[0]) + 0.5,
                f64::from(pos[1]),
                f64::from(pos[2]) + 0.5,
            );
            m.distance_sqr(at).sqrt()
        })
}
/// `Creaking.playerIsStuckInYou` of the protector `id`. The mob AI's
/// `NEAREST_PLAYERS` says only whether the creaking lists a player (the one
/// it sees: on a server, the guest it was ticked with), so once it does, the
/// eyes of every player in the level that is not a spectator are looked at:
/// a player whose eyes are inside the creaking is near it.
fn protector_stuck(host: &mut impl HeartHost, id: u32) -> bool {
    let eyes = host.player_eyes().to_vec();
    host.mob_mut(id).is_some_and(|m| {
        let listed = super::monster_brain::nearest_players(m).contains(&super::Who::Player);
        creaking::player_is_stuck(m, if listed { eyes.as_slice() } else { &[] })
    })
}
/// `computeAnalogOutputSignal`.
fn output_signal(heart: &mut CreakingHeart, pos: [i32; 3], host: &impl HeartHost) -> i32 {
    if heart.creaking.is_none() || protector(heart, host).is_none() {
        return 0;
    }
    let scaled = distance_to_creaking(heart, pos, host).clamp(0.0, 32.0) / 32.0;
    15 - (scaled * 15.0).floor() as i32
}
/// `removeProtector`: the creaking falls apart (`tearDown`), or with a
/// damage source (`by_player`) twitches to death.
pub fn remove_protector(
    heart: &mut CreakingHeart,
    host: &mut impl HeartHost,
    by_player: bool,
    out: &mut Vec<HeartEvent>,
) {
    let Some(id) = protector(heart, host) else {
        return;
    };
    if let Some(m) = host.mob_mut(id) {
        let mut events = Vec::new();
        if by_player {
            // creakingDeathEffects, setTearingDown, setHealth(0).
            creaking::set_tearing_down(m);
            m.health = 0.0;
            m.voice("entity.creaking.twitch", &mut events);
        } else {
            creaking::tear_down(m, &mut events);
        }
        out.extend(events.into_iter().map(HeartEvent::Mob));
    }
    heart.creaking = None;
}
/// `emitParticles`: trails between the protector and the heart.
fn emit_particles(
    heart: &mut CreakingHeart,
    pos: [i32; 3],
    count: i32,
    towards: bool,
    lr: &mut LevelRandom,
    host: &impl HeartHost,
    out: &mut Vec<HeartEvent>,
) {
    let Some(b) = protector(heart, host)
        .and_then(|id| host.mob(id))
        .map(Mob::bounds)
    else {
        return;
    };
    let (lo, hi) = (b.min_position(), b.max_position());
    for _ in 0..count {
        let mut source = [
            lo.x + lr.next_double() * (hi.x - lo.x),
            lo.y + lr.next_double() * (hi.y - lo.y),
            lo.z + lr.next_double() * (hi.z - lo.z),
        ];
        let mut destination = [
            f64::from(pos[0]) + lr.next_double(),
            f64::from(pos[1]) + lr.next_double(),
            f64::from(pos[2]) + lr.next_double(),
        ];
        if towards {
            std::mem::swap(&mut source, &mut destination);
        }
        let duration = lr.next_int_bound(40).unwrap_or(0) + 10;
        out.push(HeartEvent::Trail {
            at: source,
            target: destination,
            color: if towards { ORANGE } else { GRAY },
            duration,
        });
    }
}
/// `Util.shuffledCopy(Direction.values(), random)`.
fn shuffled_directions(lr: &mut impl RandomSource) -> [usize; 6] {
    let mut order = [0, 1, 2, 3, 4, 5];
    for i in (2..=6).rev() {
        let j = lr.next_int_bound(i as i32).unwrap_or(0) as usize;
        order.swap(i - 1, j);
    }
    order
}
fn offset(p: [i32; 3], d: usize) -> [i32; 3] {
    [
        p[0] + DIRECTIONS[d][0],
        p[1] + DIRECTIONS[d][1],
        p[2] + DIRECTIONS[d][2],
    ]
}
/// `spreadResin`: a resin clump on the first log face found within two steps
/// of pale oak logs (`BlockPos.breadthFirstTraversal(pos, 2, 64, ...)`).
fn spread_resin(
    b: &mut Blocks,
    pos: [i32; 3],
    lr: &mut LevelRandom,
    out: &mut Vec<HeartEvent>,
) -> Option<[i32; 3]> {
    let mut visited: Vec<[i32; 3]> = Vec::new();
    let mut queue: std::collections::VecDeque<([i32; 3], i32)> = Default::default();
    queue.push_back((pos, 0));
    let mut count = 0;
    while let Some((current, depth)) = queue.pop_front() {
        if visited.contains(&current) {
            continue;
        }
        visited.push(current);
        // The node visitor.
        if b.is_pale_oak_log(current) {
            for d in shuffled_directions(lr) {
                let n = offset(current, d);
                let mut neighbour = b.get(n);
                let opposite = d ^ 1;
                let facts = b.w.facts.get(neighbour);
                if b.w.facts.is_air(neighbour) {
                    neighbour = b.w.registry.default_state("minecraft:resin_clump")?;
                } else if &*facts.name == "minecraft:water"
                    && facts.fluid_level == 8
                    && !facts.fluid_falling
                {
                    let clump = b.w.registry.default_state("minecraft:resin_clump")?;
                    neighbour = b.with_prop(clump, "waterlogged", "true")?;
                }
                if b.name(neighbour) == "minecraft:resin_clump"
                    && b.prop(neighbour, FACE_NAMES[opposite]).as_deref() != Some("true")
                {
                    let placed = b.with_prop(neighbour, FACE_NAMES[opposite], "true")?;
                    b.set(n, placed, out);
                    return Some(n);
                }
            }
        }
        count += 1;
        if count >= 64 {
            return None;
        }
        if depth < 2 {
            // The neighbour provider: adjacent pale oak logs.
            for d in shuffled_directions(lr) {
                let n = offset(current, d);
                if b.is_pale_oak_log(n) {
                    queue.push_back((n, depth + 1));
                }
            }
        }
    }
    None
}
/// `creakingHurt`: a player hurt the protector.
pub fn creaking_hurt(
    heart: &mut CreakingHeart,
    pos: [i32; 3],
    w: &MobWorld,
    lr: &mut LevelRandom,
    host: &mut impl HeartHost,
    out: &mut Vec<HeartEvent>,
) {
    let Some(id) = protector(heart, host) else {
        return;
    };
    if heart.emitter > 0 {
        return;
    }
    emit_particles(heart, pos, 20, false, lr, host, out);
    let mut b = Blocks {
        w,
        changed: Vec::new(),
    };
    let state = b.get(pos);
    if b.prop(state, "creaking_heart_state").as_deref() == Some("awake") {
        let clumps = lr.next_int_between_inclusive(2, 3).unwrap_or(2);
        for _ in 0..clumps {
            if let Some(at) = spread_resin(&mut b, pos, lr, out) {
                out.push(HeartEvent::Sound("block.resin.place", centre(at), 1.0, 1.0));
                out.push(HeartEvent::BlockGameEvent(
                    "minecraft:block_place",
                    at,
                    state,
                ));
            }
        }
    }
    heart.emitter = 100;
    heart.emitter_target = host.mob(id).map(Mob::center);
}
fn centre(p: [i32; 3]) -> [f64; 3] {
    [
        f64::from(p[0]) + 0.5,
        f64::from(p[1]) + 0.5,
        f64::from(p[2]) + 0.5,
    ]
}

/// `SpawnUtil.Strategy.ON_TOP_OF_COLLIDER_NO_LEAVES`.
fn can_spawn_on(b: &Blocks, pos: [i32; 3], state: BlockStateId, above: BlockStateId) -> bool {
    let mut scene = SceneCollisions {
        world: b.w.world,
        binding: b.w.collision,
        subject: None,
    };
    let above_pos = [pos[0], pos[1] + 1, pos[2]];
    let above_empty =
        b.w.collision
            .collision_shape(&mut scene, above, above_pos)
            .is_ok_and(|s| s.is_empty());
    above_empty
        && !b.w.has_tag("minecraft:leaves", b.name(state))
        && b.w.facts.get(state).sturdy_full & (1 << 1) != 0
}
/// `SpawnUtil.trySpawnMob(CREAKING, SPAWNER, level, pos, 5, 16, 8,
/// ON_TOP_OF_COLLIDER_NO_LEAVES, true)` and `spawnProtector`.
fn spawn_protector(
    pos: [i32; 3],
    w: &MobWorld,
    lr: &mut LevelRandom,
    host: &mut impl HeartHost,
    out: &mut Vec<HeartEvent>,
) -> Option<u32> {
    let kind = host.creaking_type()?;
    let b = Blocks {
        w,
        changed: Vec::new(),
    };
    for _ in 0..5 {
        let dx = lr.next_int_between_inclusive(-16, 16).unwrap_or(0);
        let dz = lr.next_int_between_inclusive(-16, 16).unwrap_or(0);
        let mut at = [pos[0] + dx, pos[1] + 8, pos[2] + dz];
        if !w.world.loaded(at[0], at[2]) {
            continue;
        }
        // moveToPossibleSpawnPosition
        let mut above = b.get(at);
        let mut found = false;
        for _ in (-8..=8).rev() {
            at[1] -= 1;
            let current = b.get(at);
            if can_spawn_on(&b, at, current, above) {
                at[1] += 1;
                found = true;
                break;
            }
            above = current;
        }
        if !found {
            continue;
        }
        // noCollision(getSpawnAABB)
        let half = f64::from(kind.width as f32 / 2.0);
        let height = f64::from(kind.height as f32);
        let (x, y, z) = (
            f64::from(at[0]) + 0.5,
            f64::from(at[1]),
            f64::from(at[2]) + 0.5,
        );
        let bounds = Aabb::new(
            Vec3::new(x - half, y, z - half),
            Vec3::new(x + half, y + height, z + half),
        );
        let mut scene = SceneCollisions {
            world: w.world,
            binding: w.collision,
            subject: None,
        };
        if !scene.unobstructed(bounds).unwrap_or(false) {
            continue;
        }
        // EntityType.create: a random yaw, then Mob.finalizeSpawn.
        let mut m = host.create_mob(kind.clone(), Vec3::new(x, y, z));
        let yaw = wrap_degrees(lr.next_float() * 360.0);
        m.yaw = yaw;
        m.previous_yaw = yaw;
        m.head_yaw = yaw;
        m.body_yaw = yaw;
        finalize_spawn(&mut m, lr);
        // Mob.checkSpawnRules (always true for a creaking) and
        // checkSpawnObstruction (no liquid in the box).
        if liquid_in(w, m.bounds()) {
            continue;
        }
        let id = m.id;
        // SpawnUtil: addFreshEntityWithPassengers, then playAmbientSound.
        let mut events = Vec::new();
        if !creaking::is_active(&m) {
            m.voice("entity.creaking.ambient", &mut events);
        }
        // spawnProtector: ENTITY_PLACE at the creaking and entity event 60
        // (the poof cloud), then setTransient.
        let spawned_at = [m.position.x, m.position.y, m.position.z];
        events.push(MobEvent::GameEvent {
            event: "minecraft:entity_place",
            at: spawned_at,
            source: id,
            state: BlockStateId(0),
        });
        events.push(MobEvent::EntityEvent {
            mob: id,
            event: 60,
            at: spawned_at,
        });
        creaking::set_transient(&mut m, pos);
        // spawnProtector's caller: the creaking's spawn sound.
        m.voice("entity.creaking.spawn", &mut events);
        host.add_mob(m);
        out.extend(events.into_iter().map(HeartEvent::Mob));
        out.push(HeartEvent::Sound(
            "block.creaking_heart.spawn",
            centre(pos),
            1.0,
            1.0,
        ));
        return Some(id);
    }
    None
}
/// `Mob.finalizeSpawn`: the follow range's random spawn bonus and the
/// handedness, from the level random.
pub fn finalize_spawn(m: &mut Mob, lr: &mut LevelRandom) {
    let bonus = lr.triangle(0.0, 0.114_850_000_000_000_01);
    // ADD_MULTIPLIED_BASE on the base value.
    m.attrs.follow_range += m.kind.follow_range * bonus;
    m.update_max_visited_nodes();
    let _left_handed = lr.next_float() < 0.05;
}
fn wrap_degrees(angle: f32) -> f32 {
    let mut w = angle % 360.0;
    if w >= 180.0 {
        w -= 360.0;
    }
    if w < -180.0 {
        w += 360.0;
    }
    w
}
fn liquid_in(w: &MobWorld, b: Aabb) -> bool {
    let (lo, hi) = (b.min_position(), b.max_position());
    for x in lo.x.floor() as i32..=(hi.x.ceil() as i32 - 1) {
        for y in lo.y.floor() as i32..=(hi.y.ceil() as i32 - 1) {
            for z in lo.z.floor() as i32..=(hi.z.ceil() as i32 - 1) {
                if w.fluid(x, y, z) != crate::blocks::Fluid::None {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether this tick of the heart reaches its update (`updateCreakingState`
/// and the protector checks), the only part of [`server_tick`] that reads the
/// level's other mobs: about once a second, so a host may build them only then.
pub fn tick_updates(heart: &CreakingHeart) -> bool {
    heart.ticker < 0
}

/// `CreakingHeartBlockEntity.serverTick` (hearts that are not uprooted).
pub fn server_tick(
    heart: &mut CreakingHeart,
    pos: [i32; 3],
    w: &MobWorld,
    lr: &mut LevelRandom,
    host: &mut impl HeartHost,
    out: &mut Vec<HeartEvent>,
) {
    if server_tick_start(heart, pos, lr, host, out) {
        server_tick_update(heart, pos, w, lr, host, out);
    }
}

/// [`server_tick`] up to its update, which reads no mob world: whether the
/// update follows (as [`tick_updates`] said before it).
pub fn server_tick_start(
    heart: &mut CreakingHeart,
    pos: [i32; 3],
    lr: &mut LevelRandom,
    host: &mut impl HeartHost,
    out: &mut Vec<HeartEvent>,
) -> bool {
    heart.ticks_existed += 1;
    let signal = output_signal(heart, pos, host);
    if heart.output_signal != signal {
        heart.output_signal = signal;
        out.push(HeartEvent::OutputChanged(pos));
    }
    if heart.emitter > 0 {
        if heart.emitter > 50 {
            emit_particles(heart, pos, 1, true, lr, host, out);
            emit_particles(heart, pos, 1, false, lr, host, out);
        }
        if heart.emitter % 10 == 0 && heart.emitter_target.is_some() {
            if let Some(m) = protector(heart, host).and_then(|id| host.mob(id)) {
                heart.emitter_target = Some(m.center());
            }
            if let Some(target) = heart.emitter_target {
                let c = centre(pos);
                let progress = 0.2 + 0.8 * f64::from(100 - heart.emitter) / 100.0;
                let sound = [
                    (c[0] - target[0]) * progress + target[0],
                    (c[1] - target[1]) * progress + target[1],
                    (c[2] - target[2]) * progress + target[2],
                ];
                // playSound(null, BlockPos.containing(soundLocation), ...)
                let at = centre([
                    sound[0].floor() as i32,
                    sound[1].floor() as i32,
                    sound[2].floor() as i32,
                ]);
                let volume = heart.emitter as f32 / 2.0 / 100.0 + 0.5;
                out.push(HeartEvent::Sound(
                    "block.creaking_heart.hurt",
                    at,
                    volume,
                    1.0,
                ));
            }
        }
        heart.emitter -= 1;
    }
    let ticker = heart.ticker;
    heart.ticker -= 1;
    ticker < 0
}

/// The rest of [`server_tick`], once [`server_tick_start`] says so:
/// `updateCreakingState` and the protector checks.
pub fn server_tick_update(
    heart: &mut CreakingHeart,
    pos: [i32; 3],
    w: &MobWorld,
    lr: &mut LevelRandom,
    host: &mut impl HeartHost,
    out: &mut Vec<HeartEvent>,
) {
    heart.ticker = lr.next_int_bound(5).unwrap_or(0) + 20;
    let mut b = Blocks {
        w,
        changed: Vec::new(),
    };
    let state = b.get(pos);
    // updateCreakingState
    let active = creaking_active(w);
    let wanted = if !has_required_logs(&b, state, pos) && heart.creaking.is_none() {
        "uprooted"
    } else if active {
        "awake"
    } else {
        "dormant"
    };
    let mut updated = state;
    if b.prop(state, "creaking_heart_state").as_deref() != Some(wanted)
        && let Some(next) = b.with_prop(state, "creaking_heart_state", wanted)
    {
        updated = next;
        b.set(pos, next, out);
        if wanted == "uprooted" {
            return;
        }
    }
    if heart.creaking.is_none() {
        let awake = b.prop(updated, "creaking_heart_state").as_deref() == Some("awake");
        if awake
            && host.spawning_monsters()
            && player_near(host, pos)
            && let Some(id) = spawn_protector(pos, w, lr, host, out)
        {
            let uuid = host.uuid_for(id);
            heart.creaking = Some((Some(id), uuid));
        }
    } else if let Some(id) = protector(heart, host) {
        let persistent = host.mob(id).is_some_and(|m| m.persistent);
        let stuck = protector_stuck(host, id);
        if !active && !persistent || distance_to_creaking(heart, pos, host) > TOO_FAR || stuck {
            remove_protector(heart, host, false, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::{
        geometry::{Axis, VoxelShape},
        nbt::Tag,
    };

    /// A level as a heart's spawn check sees it: its players, no mobs.
    struct Level(Vec<(Vec3, bool)>);
    impl HeartHost for Level {
        fn players(&self) -> &[(Vec3, bool)] {
            &self.0
        }
        fn mob(&self, _: u32) -> Option<&Mob> {
            None
        }
        fn mob_mut(&mut self, _: u32) -> Option<&mut Mob> {
            None
        }
        fn mob_for_uuid(&self, _: [i32; 4]) -> Option<u32> {
            None
        }
        fn uuid_for(&mut self, _: u32) -> [i32; 4] {
            unreachable!("no creaking is spawned here")
        }
        fn creaking_type(&self) -> Option<Arc<MobType>> {
            None
        }
        fn create_mob(&mut self, _: Arc<MobType>, _: Vec3) -> Mob {
            unreachable!("no creaking is spawned here")
        }
        fn add_mob(&mut self, _: Mob) {
            unreachable!("no creaking is spawned here")
        }
        fn spawning_monsters(&self) -> bool {
            true
        }
        fn player_eyes(&self) -> &[[f64; 3]] {
            &[]
        }
    }

    /// `getNearestPlayer(pos, 32, false)` asks every player in the level. A
    /// server has no player of its own, so a heart there must see the player
    /// it serves standing 10 blocks away, or it never spawns its creaking
    /// (#15127). A spectator never counts, and 32 blocks is already too far.
    #[test]
    fn a_heart_sees_the_players_a_server_serves() {
        let pos = [0, 64, 0];
        let near = |players: &[(Vec3, bool)]| player_near(&Level(players.to_vec()), pos);
        assert!(near(&[(Vec3::new(10.0, 64.0, 0.0), false)]));
        assert!(near(&[
            (Vec3::new(5.0, 64.0, 0.0), true),
            (Vec3::new(0.0, 70.0, 31.0), false),
        ]));
        assert!(!near(&[]));
        assert!(!near(&[(Vec3::new(10.0, 64.0, 0.0), true)]));
        assert!(!near(&[(Vec3::new(32.0, 64.0, 0.0), false)]));
    }

    /// A heart's protector and the eyes of the players in its level.
    struct Protector {
        creaking: Mob,
        eyes: Vec<[f64; 3]>,
    }
    impl HeartHost for Protector {
        fn players(&self) -> &[(Vec3, bool)] {
            &[]
        }
        fn player_eyes(&self) -> &[[f64; 3]] {
            &self.eyes
        }
        fn mob(&self, id: u32) -> Option<&Mob> {
            Some(&self.creaking).filter(|m| m.id == id)
        }
        fn mob_mut(&mut self, id: u32) -> Option<&mut Mob> {
            Some(&mut self.creaking).filter(|m| m.id == id)
        }
        fn mob_for_uuid(&self, _: [i32; 4]) -> Option<u32> {
            None
        }
        fn uuid_for(&mut self, _: u32) -> [i32; 4] {
            unreachable!("no creaking is spawned here")
        }
        fn creaking_type(&self) -> Option<Arc<MobType>> {
            None
        }
        fn create_mob(&mut self, _: Arc<MobType>, _: Vec3) -> Mob {
            unreachable!("no creaking is spawned here")
        }
        fn add_mob(&mut self, _: Mob) {
            unreachable!("no creaking is spawned here")
        }
        fn spawning_monsters(&self) -> bool {
            true
        }
    }

    /// `playerIsStuckInYou` looks at the eyes of the players the creaking's
    /// `NEAREST_PLAYERS` lists. A server has no player of its own, so they
    /// are the eyes of the players it serves: one whose eyes stay inside the
    /// creaking frees it on the fifth check in a row (#15127), and none count
    /// while the creaking lists no player.
    #[test]
    fn a_player_a_server_serves_gets_stuck_in_the_protector() {
        let mut kind = (*crate::mobs::sensor_scan_tests::kind()).clone();
        kind.species = crate::mobs::Species::Creaking;
        // 0.9 wide and 1.4 tall: x and z from 0.05 to 0.95, y from 64 to 65.4.
        let mut creaking = Mob::new(7, Arc::new(kind), Vec3::new(0.5, 64.0, 0.5), 0.0);
        super::super::monster_brain::init(&mut creaking, 0);
        let mut level = Protector {
            creaking,
            eyes: vec![[9.5, 65.62, 0.5], [0.5, 65.0, 0.5]],
        };
        for _ in 0..5 {
            assert!(!protector_stuck(&mut level, 7), "no player listed");
        }
        super::super::monster_brain::mem(&mut level.creaking)
            .nearest_players
            .set_list(vec![crate::mobs::Who::Player]);
        for check in 1..=4 {
            assert!(!protector_stuck(&mut level, 7), "check {check}");
        }
        assert!(protector_stuck(&mut level, 7), "the fifth check in a row");
    }

    /// A server's level as a heart sees it: the players it serves, and the
    /// creakings the heart spawns into it.
    struct Server {
        players: Vec<(Vec3, bool)>,
        mobs: Vec<Mob>,
    }
    /// The UUID the server gives a creaking it adds.
    const UUID: [i32; 4] = [0, 0, 0, 15127];
    impl HeartHost for Server {
        fn players(&self) -> &[(Vec3, bool)] {
            &self.players
        }
        fn mob(&self, id: u32) -> Option<&Mob> {
            self.mobs.iter().find(|m| m.id == id)
        }
        fn mob_mut(&mut self, id: u32) -> Option<&mut Mob> {
            self.mobs.iter_mut().find(|m| m.id == id)
        }
        fn mob_for_uuid(&self, _: [i32; 4]) -> Option<u32> {
            None
        }
        fn uuid_for(&mut self, _: u32) -> [i32; 4] {
            UUID
        }
        fn creaking_type(&self) -> Option<Arc<MobType>> {
            let mut kind = (*crate::mobs::sensor_scan_tests::kind()).clone();
            kind.species = crate::mobs::Species::Creaking;
            Some(Arc::new(kind))
        }
        fn create_mob(&mut self, kind: Arc<MobType>, at: Vec3) -> Mob {
            Mob::new(100, kind, at, 0.0)
        }
        fn add_mob(&mut self, m: Mob) {
            self.mobs.push(m);
        }
        fn spawning_monsters(&self) -> bool {
            true
        }
        fn player_eyes(&self) -> &[[f64; 3]] {
            &[]
        }
    }

    /// An awake heart in its trunk at midnight, ticked as a server ticks it.
    /// The server's mob world has no player of its own (`selected_player`
    /// is none, as `settle_spawn` leaves it); the level's one player, the
    /// one it serves, stands 10 blocks away. `getNearestPlayer` finds that
    /// player, so the heart spawns its creaking and keeps it as its
    /// protector (#15127). A spectator there, or nobody, spawns nothing.
    #[test]
    fn a_heart_on_a_server_spawns_its_creaking_for_the_player_it_serves() {
        with_trunk(|w| {
            assert!(
                w.selected_player().is_none(),
                "a server has no player of its own"
            );
            let tick = |players: Vec<(Vec3, bool)>| {
                let mut host = Server {
                    players,
                    mobs: Vec::new(),
                };
                // Its update is due this tick.
                let mut heart = CreakingHeart {
                    ticker: -1,
                    ..Default::default()
                };
                let mut out = Vec::new();
                let mut lr = crate::mobs::world::level_random(15127);
                server_tick(&mut heart, HEART, w, &mut lr, &mut host, &mut out);
                (heart.creaking, host.mobs, out)
            };
            let near = Vec3::new(10.0, 65.0, 0.0);
            let (protector, mobs, out) = tick(vec![(near, false)]);
            let [creaking] = mobs.as_slice() else {
                panic!("one creaking spawned, got {}", mobs.len());
            };
            assert_eq!(
                protector,
                Some((Some(creaking.id), UUID)),
                "the heart's protector"
            );
            assert_eq!(
                creaking.extra.home,
                Some(HEART),
                "the creaking is bound to the heart"
            );
            assert!(
                out.iter()
                    .any(|e| matches!(e, HeartEvent::Sound("block.creaking_heart.spawn", ..))),
                "the heart plays its spawn sound"
            );
            assert!(
                !out.iter().any(|e| matches!(e, HeartEvent::SetBlock(..))),
                "the heart stays awake in its trunk"
            );
            for (who, players) in [("a spectator", vec![(near, true)]), ("nobody", Vec::new())] {
                let (protector, mobs, _) = tick(players);
                assert!(protector.is_none() && mobs.is_empty(), "{who}: no creaking");
            }
        });
    }

    /// Where the heart sits: a pale oak log below and above it, over a stone
    /// floor at y 63 from x and z -16 to 16, where `spawn_protector` looks.
    const HEART: [i32; 3] = [0, 65, 0];
    /// The level's block states, in registry order: the block, its
    /// properties, and whether it is a full, sturdy cube.
    const BLOCKS: [(&str, &[(&str, &str)], bool); 6] = [
        ("minecraft:air", &[], false),
        ("minecraft:stone", &[], true),
        ("minecraft:pale_oak_log", &[("axis", "y")], true),
        (
            "minecraft:creaking_heart",
            &[("axis", "y"), ("creaking_heart_state", "uprooted")],
            true,
        ),
        (
            "minecraft:creaking_heart",
            &[("axis", "y"), ("creaking_heart_state", "dormant")],
            true,
        ),
        (
            "minecraft:creaking_heart",
            &[("axis", "y"), ("creaking_heart_state", "awake")],
            true,
        ),
    ];
    const STONE: BlockStateId = BlockStateId(1);
    const LOG: BlockStateId = BlockStateId(2);
    const AWAKE: BlockStateId = BlockStateId(5);

    /// Runs `f` over the heart's trunk as a server's mob world sees it at
    /// midnight in the overworld, with no player of its own. Built from
    /// [`BLOCKS`] alone: no prepared vanilla data.
    fn with_trunk(f: impl FnOnce(&MobWorld<'_>)) {
        use horizon_world::{
            block_collision::BlockCollisionRegistry,
            block_landing::{BlockLandingRegistry, FallDefinition, LandingDefinition},
            collision::CachedCollisionRegistry,
            movement_properties::MovementProperties,
            registry::BlockRegistry,
        };
        let text = |s: &str| Tag::String(s.into());
        let states: Vec<Tag> = BLOCKS
            .iter()
            .map(|&(name, properties, _)| {
                let mut fields = vec![("id", text(name))];
                if !properties.is_empty() {
                    fields.push((
                        "properties",
                        Tag::compound(properties.iter().map(|&(key, value)| (key, text(value)))),
                    ));
                }
                Tag::compound(fields)
            })
            .collect();
        let mut names: Vec<&str> = BLOCKS.iter().map(|&(name, ..)| name).collect();
        names.dedup();
        let registry = BlockRegistry::from_nbt(&Tag::compound([
            ("DataVersion", Tag::Int(horizon_world::DATA_VERSION)),
            ("states", Tag::List(states.clone())),
            (
                "defaults",
                Tag::compound([
                    ("minecraft:air", Tag::Int(0)),
                    ("minecraft:stone", Tag::Int(1)),
                    ("minecraft:pale_oak_log", Tag::Int(2)),
                    ("minecraft:creaking_heart", Tag::Int(3)),
                ]),
            ),
        ]))
        .unwrap();
        let facts = crate::blocks::BlockFacts {
            states: BLOCKS
                .iter()
                .map(|&(name, _, full)| state_facts(name, full))
                .collect(),
            air: BlockStateId(0),
            woods: Default::default(),
            sign_texts: Default::default(),
        };
        let collision = Arc::new(
            BlockCollisionRegistry::new(Arc::new(
                CachedCollisionRegistry::from_nbt(
                    &Tag::compound([
                        ("DataVersion", Tag::Int(horizon_world::DATA_VERSION)),
                        ("states", Tag::List(states.clone())),
                        (
                            "shapes",
                            Tag::List(vec![
                                shape_tag(&VoxelShape::empty()),
                                shape_tag(&VoxelShape::block()),
                            ]),
                        ),
                        (
                            "cached_shapes",
                            Tag::IntArray(
                                BLOCKS.iter().map(|&(_, _, full)| i32::from(full)).collect(),
                            ),
                        ),
                    ]),
                    &registry,
                )
                .unwrap(),
            ))
            .unwrap(),
        );
        let suffocation =
            crate::mobs::suffocation::bind(collision.clone(), &std::collections::BTreeSet::new())
                .unwrap();
        let fluids = crate::motion::FluidResources::from_registry(
            &registry,
            &facts,
            &std::collections::BTreeSet::new(),
        )
        .unwrap();
        let attributes = Arc::new(
            horizon_world::attributes::entity::Catalog::from_json(&serde_json::json!({
                "data_version": horizon_world::DATA_VERSION,
                "attributes": [],
                "entities": {},
            }))
            .unwrap(),
        );
        let float = |v: f32| Tag::Float(v.to_bits());
        let properties = MovementProperties::from_nbt(
            &Tag::compound([
                ("DataVersion", Tag::Int(horizon_world::DATA_VERSION)),
                ("states", Tag::List(states)),
                (
                    "blocks",
                    Tag::compound(names.iter().map(|&name| {
                        (
                            name,
                            Tag::compound([
                                ("friction", float(0.6)),
                                ("speedFactor", float(1.0)),
                                ("jumpFactor", float(1.0)),
                                ("bounceRestitution", float(0.0)),
                                ("fallDistanceReduction", float(0.0)),
                                ("fenceGate", Tag::Byte(0)),
                            ]),
                        )
                    })),
                ),
            ]),
            &registry,
        )
        .unwrap();
        let movement = properties.bind(&Default::default()).unwrap();
        let landing = BlockLandingRegistry::new(
            &properties,
            &names
                .iter()
                .map(|&name| {
                    let definition = LandingDefinition {
                        fall: FallDefinition::Default,
                        shelf_mushroom_bounce: false,
                    };
                    (name.to_owned(), definition)
                })
                .collect::<std::collections::BTreeMap<_, _>>(),
        )
        .unwrap();
        let data = crate::mobs::MobData::fallback(&facts);
        // No biome files: a folder with an empty biome list.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "horizon-creaking-heart-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("data/minecraft/worldgen/biome")).unwrap();
        let biomes = crate::biome::Biomes::load_for_server(
            &horizon_assets::resource_source::ResourceRoot::directory(root.clone()),
        );
        std::fs::remove_dir_all(&root).unwrap();
        let biomes = biomes.unwrap();
        let block_tags = std::collections::HashMap::from([(
            "minecraft:pale_oak_logs".to_owned(),
            std::collections::HashSet::from(["minecraft:pale_oak_log".to_owned()]),
        )]);
        let mut world = crate::world::World::new(BlockStateId(0), 0);
        for x in -1..=1 {
            for z in -1..=1 {
                world
                    .chunks
                    .insert((x, z), Arc::new(crate::chunk::Chunk::empty(x, z)));
            }
        }
        for x in -16..=16 {
            for z in -16..=16 {
                world.set(x, 63, z, STONE).unwrap();
            }
        }
        let [x, y, z] = HEART;
        world.set(x, y - 1, z, LOG).unwrap();
        world.set(x, y, z, AWAKE).unwrap();
        world.set(x, y + 1, z, LOG).unwrap();
        let lid_openers = std::collections::HashMap::new();
        let w = MobWorld {
            mob_maps: horizon_client::neoforge_mob_maps::NeoForgeMobMaps::vanilla(),
            goal_host: None,
            teams: Default::default(),
            professions: crate::mobs::villager::profession_table(),
            loop_context: crate::test_resources::server_loop_context(),
            world: &world,
            registry: &registry,
            facts: &facts,
            fluid_tables: &fluids,
            attributes: &attributes,
            movement: &movement,
            collision: &collision,
            suffocation: &suffocation,
            data: &data,
            // A server has no player of its own (`settle_spawn`).
            player: None,
            player_identity: None,
            player_selection: None,
            controlling_player_movement: None,
            nearest_player: None,
            others: &[],
            live_mobs: None,
            dragon: None,
            leash_boats: &[],
            who_lookup: None,
            foreign: None,
            others_grid: None,
            others_index: None,
            passenger_objects: &[],
            others_villagers: None,
            village: None,
            species_slots: None,
            others_solid: None,
            daylight: false,
            monsters_burn: false,
            game_time: 1000,
            // Midnight: creakings are active.
            day_time: 18000,
            raining: false,
            thundering: false,
            level: horizon_core::LevelKey::OVERWORLD,
            block_tags: &block_tags,
            landing: &landing,
            biomes: &biomes,
            mob_griefing: true,
            universal_anger: false,
            mob_drops: true,
            max_entity_cramming: 24,
            difficulty: crate::mobs::spawning::LocalDifficulty::new(2, 18000, 0, 0.0),
            dimension_type: horizon_world::dimension_type::DimensionFacts::overworld(),
            world_seed: 0,
            sea_level: 63,
            items: None,
            lid_openers: &lid_openers,
            mod_effects: None,
            mob_hooks: None,
            asks_animal_tame: false,
            asks_living_destroy_block: false,
            asks_baby_spawn: false,
            border: None,
            section_writer: None,
        };
        f(&w);
    }

    /// A block's facts: a full, sturdy cube when `full`, else empty space.
    fn state_facts(name: &str, full: bool) -> crate::blocks::StateFacts {
        let sound: Arc<str> = Arc::from("");
        crate::blocks::StateFacts {
            name: Arc::from(name),
            emission: 0,
            dynamic_light: false,
            dampening: 0,
            skylight_down: !full,
            shape_occlusion: false,
            can_occlude: full,
            solid_render: full,
            hardness: 1.0,
            requires_tool: false,
            replaceable: !full,
            random_ticks: false,
            block_entity: false,
            render_shape: crate::blocks::RenderShape::Model,
            destroy_overlay: true,
            full_collision: full,
            friction: 0.6,
            explosion_resistance: 1.0,
            item: Arc::from(name),
            pick_item: Arc::from(name),
            sounds: crate::blocks::Sounds {
                volume: 1.0,
                pitch: 1.0,
                break_sound: sound.clone(),
                step: sound.clone(),
                place: sound.clone(),
                hit: sound.clone(),
                fall: sound,
            },
            fluid: crate::blocks::Fluid::None,
            fluid_level: 0,
            fluid_falling: false,
            waterlogged: false,
            random_offset: false,
            outline_offset: false,
            legacy_solid: full,
            // isFaceSturdy(FULL) on all six faces.
            sturdy_full: if full { 0b11_1111 } else { 0 },
            classes: Arc::from([]),
            outline: Arc::from([]),
            outline_edges: Arc::from([]),
            map_color: 0,
        }
    }

    /// A collision shape as `cached-collision.nbt` stores it.
    fn shape_tag(shape: &VoxelShape) -> Tag {
        let sizes = shape.sizes();
        let mut bits = Vec::new();
        for z in 0..sizes[2] {
            for y in 0..sizes[1] {
                for x in 0..sizes[0] {
                    bits.push(u8::from(shape.is_full(x as i32, y as i32, z as i32)));
                }
            }
        }
        let (min, max) = shape.discrete_bounds();
        let mut fields = vec![
            ("occupied", Tag::ByteArray(bits)),
            ("cube", Tag::Byte(i8::from(shape.is_cube()))),
            (
                "cube_axes",
                Tag::Byte(
                    shape
                        .cube_coordinate_axes()
                        .iter()
                        .enumerate()
                        .fold(0, |mask, (i, on)| mask | (i8::from(*on) << i)),
                ),
            ),
            ("discrete_min", Tag::IntArray(min.to_vec())),
            ("discrete_max", Tag::IntArray(max.to_vec())),
        ];
        for (axis, name) in [(Axis::X, "x"), (Axis::Y, "y"), (Axis::Z, "z")] {
            fields.push((
                name,
                Tag::LongArray(
                    shape
                        .coordinates(axis)
                        .iter()
                        .map(|v| v.to_bits() as i64)
                        .collect(),
                ),
            ));
        }
        Tag::compound(fields)
    }
}
