//! #13007: a move or a motion for an item, a falling block, TNT or an orb
//! looks only in that kind's list (`OwnList`).
use super::{MovePacket, Path};
use crate::game::{
    Audio, Game, Options, Progress, Settings, execution_role::ExecutionRole, load_resources,
};
use horizon_core::geometry::Vec3;
use horizon_protocol::packets::play::{self, Clientbound as C};

/// Each reaches its own entity and no other, and an id tracked as a falling
/// block that no list holds any more moves nothing.
#[test]
#[ignore = "daily"]
fn moves_and_motion_reach_their_own_list() {
    let root = std::path::PathBuf::from(std::env::var_os("HORIZON_TEST_RESOURCES").unwrap());
    let settings = Settings::default();
    let loaded = load_resources(
        horizon_assets::resource_source::ResourceRoot::directory(root.clone()),
        Vec::new(),
        None,
        &root,
        &settings,
        std::time::Instant::now(),
        false,
        ExecutionRole::Client,
        &Progress::new(),
        &mut |_, _| unreachable!(),
    )
    .unwrap();
    let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
    options.resources = root.clone();
    options.execution_role = ExecutionRole::Client;
    let mut game = Game::new(
        &loaded.resources,
        loaded.pack,
        loaded.gui.unwrap(),
        options,
        settings,
        Audio::silent(&horizon_assets::resource_source::ResourceSource::directory(
            root.clone(),
        )),
        None,
    )
    .unwrap();
    let (client, _server) = horizon_protocol::connection::Pipe::pair();
    game.connect_local(client, "LookupTest").unwrap();
    let start = [0.0, 64.0, 0.0];
    for (id, name) in [
        (40, "falling_block"),
        (41, "falling_block"),
        (43, "tnt"),
        (45, "experience_orb"),
        (47, "zombie"),
        (49, "item"),
    ] {
        let kind = game
            .remote
            .as_ref()
            .unwrap()
            .builtin
            .get("minecraft:entity_type")
            .unwrap()
            .id(&format!("minecraft:{name}"))
            .unwrap();
        game.handle_packet(C::AddEntity(play::AddEntity {
            id,
            uuid: id as u128,
            kind,
            position: start,
            velocity: [0.0; 3],
            pitch: 0,
            yaw: 0,
            head_yaw: 0,
            data: 0,
        }))
        .unwrap();
    }
    assert_eq!(game.mobs.iter().map(|m| m.id).collect::<Vec<_>>(), [47]);
    // The item's stack arrives in its entity data: it leaves `pending_items`
    // for `dropped`.
    game.remote.as_mut().unwrap().pending_items.remove(&49);
    game.dropped.push(horizon_client::entities::ItemEntity::new(
        49,
        horizon_client::items::ItemStack::new("minecraft:sand", 1),
        Vec3::new(start[0], start[1], start[2]),
        Vec3::ZERO,
        0.0,
    ));
    // Still tracked as a falling block, but in no list.
    game.falling.retain(|f| f.id != 41);
    // A target and a motion of its own for each: one that lands on another
    // entity shows.
    let to = |id: i32| [f64::from(id), 70.0, 1.0];
    let v = |id: i32| [f64::from(id) / 100.0, 0.5, -0.25];
    for id in [40, 41, 43, 45, 49] {
        game.move_entity(
            id,
            Some(Path::Linear(to(id))),
            None,
            false,
            MovePacket::Move,
        );
        game.remote_motion(id, v(id));
    }
    let at = |p: Vec3| [p.x, p.y, p.z];
    let moved = |id: u32, position: Vec3, velocity: Vec3| (id, at(position), at(velocity));
    assert_eq!(
        game.falling
            .iter()
            .map(|f| moved(f.id, f.position, f.velocity))
            .collect::<Vec<_>>(),
        [(40, to(40), v(40))]
    );
    assert_eq!(
        game.tnts
            .iter()
            .map(|t| moved(t.id, t.position, t.velocity))
            .collect::<Vec<_>>(),
        [(43, to(43), v(43))]
    );
    assert_eq!(
        game.dropped
            .iter()
            .map(|d| moved(d.id, d.position, d.velocity))
            .collect::<Vec<_>>(),
        [(49, to(49), v(49))]
    );
    // An orb eases to a move over three ticks.
    let lerps = &game.remote.as_ref().unwrap().lerps;
    assert_eq!(lerps.keys().copied().collect::<Vec<_>>(), [45]);
    assert_eq!(lerps[&45].target, to(45));
    assert_eq!(
        game.orbs
            .iter()
            .map(|o| (o.id, at(o.velocity)))
            .collect::<Vec<_>>(),
        [(45, v(45))]
    );
    assert_eq!(
        game.mobs
            .iter()
            .map(|m| moved(m.id, m.position, m.velocity))
            .collect::<Vec<_>>(),
        [(47, start, [0.0; 3])]
    );
}
