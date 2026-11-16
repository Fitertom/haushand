//! Right-clicks on a block face, done by the bot itself: the look, the checks and the packet.
//!
//! Azalea's `block_interact` clicks the face under the crosshair, or, when the crosshair is
//! elsewhere, makes up the top face at the block's centre. Neither is what a placement
//! needs: putting a block on top of one above our eyes means clicking a top face we cannot
//! see. The vanilla server checks reach and that the point is on the block, not the line of
//! sight, so any face in reach can be clicked, as long as everything it does check holds:
//! that is checked here first, because a click the server refuses gives no answer at all.
//!
//! The look: our own yaw and pitch towards the point, written to the look component, so the
//! movement packet Azalea sends every tick carries them before the click. A rotation packet
//! of our own would make two movement packets in one tick, which 26.3 kicks for.

use azalea::{
    BlockPos, Client, Vec3,
    block::BlockState,
    core::{aabb::Aabb, direction::Direction},
    ecs::query::Without,
    entity::{LookDirection, Position, dimensions::EntityDimensions, metadata},
    interact::BlockStatePredictionHandler,
    physics::collision::BlockWithShape,
    protocol::packets::game::{
        s_interact::InteractionHand,
        s_use_item_on::{BlockHit, ServerboundUseItemOn},
    },
    registry::builtin::BlockKind,
};
use eyre::{Result, bail};

use crate::world::me;

/// The player's `block_interaction_range`. The server allows one more block; the margin
/// keeps a click just at the edge from being refused after a small move.
pub const REACH: f64 = 4.5;

/// Eye position.
pub fn eye(bot: &Client) -> Vec3 {
    let h = bot.component::<EntityDimensions>().map(|d| d.eye_height as f64).unwrap_or(1.62);
    me(bot) + Vec3::new(0.0, h, 0.0)
}

/// Yaw and pitch (degrees, Minecraft's convention) to look from `from` at `to`.
pub fn angles(from: Vec3, to: Vec3) -> (f32, f32) {
    let d = to - from;
    let horizontal = (d.x * d.x + d.z * d.z).sqrt();
    let yaw = (-d.x).atan2(d.z).to_degrees();
    let pitch = (-d.y).atan2(horizontal).to_degrees();
    (yaw as f32, pitch.clamp(-90.0, 90.0) as f32)
}

/// Turns the head to `point` and waits a tick, so the server has our look before a click.
pub async fn look(bot: &Client, point: Vec3) {
    let (yaw, pitch) = angles(eye(bot), point);
    {
        let mut ecs = bot.ecs.write();
        if let Some(mut l) = ecs.get_mut::<LookDirection>(bot.entity) {
            *l = LookDirection::new(yaw, pitch);
        }
    }
    bot.wait_ticks(1).await;
}

/// Blocks a right-click opens or toggles instead of placing against: clicking one with a
/// block in hand would open a table, a furnace or a chest, or swing a door.
pub fn uses_click(kind: BlockKind) -> bool {
    use BlockKind as B;
    let name = format!("{kind:?}");
    matches!(
        kind,
        B::CraftingTable | B::Furnace | B::BlastFurnace | B::Smoker | B::Chest | B::TrappedChest | B::EnderChest
            | B::Barrel | B::Hopper | B::Dispenser | B::Dropper | B::Anvil | B::ChippedAnvil | B::DamagedAnvil
            | B::EnchantingTable | B::BrewingStand | B::Lever | B::Repeater | B::Comparator | B::DaylightDetector
            | B::NoteBlock | B::Jukebox | B::Bell | B::Lectern | B::Loom | B::Grindstone | B::Stonecutter
            | B::CartographyTable | B::SmithingTable | B::Beacon | B::RespawnAnchor | B::Cake | B::Composter
            | B::Crafter | B::DecoratedPot | B::ChiseledBookshelf | B::Campfire | B::SoulCampfire
    ) || ["Door", "Trapdoor", "FenceGate", "Button", "Bed", "ShulkerBox", "Sign", "CopperGolemStatue", "Shelf"]
        .iter()
        .any(|s| name.ends_with(s))
}

/// A block one can click on to put another beside it: something solid with an outline,
/// loaded, not a fluid, not opened by the click.
pub fn clickable(state: Option<BlockState>) -> bool {
    let Some(state) = state else { return false };
    let kind = BlockKind::from(state);
    !state.is_air()
        && !matches!(kind, BlockKind::Water | BlockKind::Lava | BlockKind::BubbleColumn)
        && !state.outline_shape(BlockPos::new(0, 0, 0)).to_aabbs().is_empty()
        && !uses_click(kind)
}

/// The world's build height in the bot's dimension, from the dimension type the server sent.
pub fn height_ok(bot: &Client, y: i32) -> bool {
    let world = crate::world::world_lock(bot);
    let w = world.read();
    let min = w.chunks.min_y();
    let max = min + w.chunks.height() as i32;
    y >= min && y < max
}

/// Distance from our eyes to the nearest point of the unit cube at `pos`, as the server's
/// `isWithinBlockInteractionRange` measures.
pub fn reach_to(bot: &Client, pos: BlockPos) -> f64 {
    let e = eye(bot);
    let c = |v: f64, lo: i32| v.clamp(lo as f64, lo as f64 + 1.0);
    let nearest = Vec3::new(c(e.x, pos.x), c(e.y, pos.y), c(e.z, pos.z));
    nearest.distance_to(e)
}

/// Whether an entity that blocks building (us, a mob, a player; not dropped items or
/// orbs) stands in `cube`.
pub fn entity_in(bot: &Client, cube: &Aabb) -> bool {
    let Some(world_name) = bot.component::<azalea::world::WorldName>().ok().map(|w| w.clone()) else { return true };
    let mut ecs = bot.ecs.write();
    let mut q = ecs.query_filtered::<(&Position, &EntityDimensions, &azalea::world::WorldName), (Without<metadata::Item>, Without<metadata::ExperienceOrb>)>();
    q.iter(&ecs).filter(|(_, _, w)| **w == world_name).any(|(p, d, _)| d.make_bounding_box(**p).intersects_aabb(cube))
}

/// Right-clicks `face` of `against` at `point` with the main hand, after checking what the
/// server will check. The item must already be held. Returns as soon as the click is sent;
/// what it did is for the caller to watch (a block update, a container).
pub async fn use_item_on(bot: &Client, against: BlockPos, face: Direction, point: Vec3) -> Result<()> {
    let state = crate::world::world_lock(bot).read().get_block_state(against);
    if !clickable(state) {
        bail!("{against} cannot be clicked: {:?}", state.map(BlockKind::from));
    }
    if !height_ok(bot, against.y) {
        bail!("{against} is outside the world's height");
    }
    // the point must lie on that face of the block's unit cube (the server's check is
    // within 1.0000001 of the centre on every axis)
    let n = face.normal();
    let local = point - Vec3::new(against.x as f64, against.y as f64, against.z as f64);
    let on_face = [(local.x, n.x), (local.y, n.y), (local.z, n.z)].iter().all(|&(v, n)| match n {
        1 => (v - 1.0).abs() < 1e-3,
        -1 => v.abs() < 1e-3,
        _ => (-1e-6..=1.0 + 1e-6).contains(&v),
    });
    if !on_face {
        bail!("click point {point} is not on the {face:?} face of {against}");
    }
    look(bot, point).await;
    if reach_to(bot, against) > REACH {
        bail!("{against} is out of reach ({:.2})", reach_to(bot, against));
    }
    // the sequence number the server echoes when it confirms the block change: taken from
    // azalea's counter so its own clicks and ours never reuse one
    let seq = {
        let mut ecs = bot.ecs.write();
        ecs.get_mut::<BlockStatePredictionHandler>(bot.entity).map(|mut h| h.start_predicting()).unwrap_or(1)
    };
    bot.write_packet(ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        block_hit: BlockHit { block_pos: against, direction: face, location: point, inside: false, world_border: false },
        seq,
    });
    Ok(())
}
