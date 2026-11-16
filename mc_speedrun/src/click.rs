//! Right-clicks on a block face, done by the bot itself the way a player does them: the
//! look, the line of sight, the checks and the packet.
//!
//! A click goes only to a face the eyes see: our own ray from the eyes through the point
//! must reach that block, entering by that face, with nothing in between, and no farther
//! than the player's reach. What the server checks on top (loaded, the point on the block,
//! no entity where a block goes) is checked here first, because a click the server
//! refuses gives no answer at all.
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

/// The player's `block_interaction_range`, from the eyes to the point clicked: the client's
/// own pick ray is this long.
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

/// Where a ray from the eyes first meets a block: the block, the face it enters by, the
/// point, and how far.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub pos: BlockPos,
    pub face: Direction,
    pub point: Vec3,
    pub dist: f64,
}

/// Where a ray from `from` in direction `dir` (unit) first meets a block outline, within
/// `max`. Blocks are walked one by one along the ray; in each, the outline boxes (a slab is
/// half a block, a fence a post) are tested, so a ray passes over a slab and through grass
/// the way the game's own pick does. Unloaded blocks stop the ray.
pub fn raycast(bot: &Client, from: Vec3, dir: Vec3, max: f64) -> Option<Hit> {
    let world = crate::world::world_lock(bot);
    let world = world.read();
    let mut cell = BlockPos::from(from);
    let step = |d: f64| if d > 0.0 { 1 } else if d < 0.0 { -1 } else { 0 };
    let (sx, sy, sz) = (step(dir.x), step(dir.y), step(dir.z));
    let next = |p: f64, c: i32, s: i32, d: f64| {
        if s == 0 { f64::INFINITY } else { ((c + (s > 0) as i32) as f64 - p) / d }
    };
    let mut t_max = Vec3::new(next(from.x, cell.x, sx, dir.x), next(from.y, cell.y, sy, dir.y), next(from.z, cell.z, sz, dir.z));
    let delta = Vec3::new(if sx != 0 { 1.0 / dir.x.abs() } else { f64::INFINITY }, if sy != 0 { 1.0 / dir.y.abs() } else { f64::INFINITY }, if sz != 0 { 1.0 / dir.z.abs() } else { f64::INFINITY });
    let mut t = 0.0;
    while t <= max {
        let state = world.get_block_state(cell)?;
        if !state.is_air() {
            let off = Vec3::new(cell.x as f64, cell.y as f64, cell.z as f64);
            let mut best: Option<(f64, Direction)> = None;
            for b in state.outline_shape(cell).to_aabbs() {
                let b = b.move_relative(off);
                if let Some((te, face)) = ray_box(from, dir, &b)
                    && te <= max
                    && best.is_none_or(|(bt, _)| te < bt)
                {
                    best = Some((te, face));
                }
            }
            if let Some((te, face)) = best {
                return Some(Hit { pos: cell, face, point: from + dir * te, dist: te });
            }
        }
        // on to the next block the ray enters
        if t_max.x <= t_max.y && t_max.x <= t_max.z {
            t = t_max.x;
            t_max.x += delta.x;
            cell = BlockPos::new(cell.x + sx, cell.y, cell.z);
        } else if t_max.y <= t_max.z {
            t = t_max.y;
            t_max.y += delta.y;
            cell = BlockPos::new(cell.x, cell.y + sy, cell.z);
        } else {
            t = t_max.z;
            t_max.z += delta.z;
            cell = BlockPos::new(cell.x, cell.y, cell.z + sz);
        }
    }
    None
}

/// Where the ray enters box `b`, and by which face; None when it misses, or starts inside.
fn ray_box(from: Vec3, dir: Vec3, b: &Aabb) -> Option<(f64, Direction)> {
    let mut t_enter = f64::NEG_INFINITY;
    let mut t_exit = f64::INFINITY;
    let mut face = Direction::Up;
    for (o, d, lo, hi, neg, pos) in [
        (from.x, dir.x, b.min.x, b.max.x, Direction::West, Direction::East),
        (from.y, dir.y, b.min.y, b.max.y, Direction::Down, Direction::Up),
        (from.z, dir.z, b.min.z, b.max.z, Direction::North, Direction::South),
    ] {
        if d.abs() < 1e-12 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let (t1, t2) = ((lo - o) / d, (hi - o) / d);
        // entering through the min side when moving +, the max side when moving -
        let (near, far, f) = if t1 < t2 { (t1, t2, neg) } else { (t2, t1, pos) };
        if near > t_enter {
            t_enter = near;
            face = f;
        }
        t_exit = t_exit.min(far);
    }
    (t_enter <= t_exit && t_enter >= 0.0).then_some((t_enter, face))
}

/// Whether the eyes see `point` on `face` of `pos`: the ray towards it, in reach, first
/// meets that block through that face.
pub fn sees(bot: &Client, pos: BlockPos, face: Direction, point: Vec3) -> bool {
    let e = eye(bot);
    let d = point - e;
    let len = d.length();
    if len > REACH || len < 1e-6 {
        return false;
    }
    // a little past the point, so a ray grazing the face still counts as meeting it
    raycast(bot, e, d * (1.0 / len), len + 0.05).is_some_and(|h| h.pos == pos && h.face == face)
}

/// Points to aim at on `face` of `pos`'s outline: the middle first, then towards the
/// corners, so an edge peeking past another block is found too.
fn face_points(bot: &Client, pos: BlockPos, face: Direction) -> Vec<Vec3> {
    let state = crate::world::block_at(bot, pos);
    let off = Vec3::new(pos.x as f64, pos.y as f64, pos.z as f64);
    let n = face.normal();
    let mut out = Vec::new();
    for b in state.outline_shape(pos).to_aabbs() {
        let b = b.move_relative(off);
        let at = |u: f64, v: f64| {
            let lerp = |lo: f64, hi: f64, f: f64| lo + (hi - lo) * f;
            match (n.x, n.y, n.z) {
                (1, _, _) => Vec3::new(b.max.x, lerp(b.min.y, b.max.y, u), lerp(b.min.z, b.max.z, v)),
                (-1, _, _) => Vec3::new(b.min.x, lerp(b.min.y, b.max.y, u), lerp(b.min.z, b.max.z, v)),
                (_, 1, _) => Vec3::new(lerp(b.min.x, b.max.x, u), b.max.y, lerp(b.min.z, b.max.z, v)),
                (_, -1, _) => Vec3::new(lerp(b.min.x, b.max.x, u), b.min.y, lerp(b.min.z, b.max.z, v)),
                (_, _, 1) => Vec3::new(lerp(b.min.x, b.max.x, u), lerp(b.min.y, b.max.y, v), b.max.z),
                _ => Vec3::new(lerp(b.min.x, b.max.x, u), lerp(b.min.y, b.max.y, v), b.min.z),
            }
        };
        for (u, v) in [(0.5, 0.5), (0.2, 0.2), (0.8, 0.2), (0.2, 0.8), (0.8, 0.8), (0.5, 0.15), (0.5, 0.85), (0.15, 0.5), (0.85, 0.5)] {
            out.push(at(u, v));
        }
    }
    out
}

/// A point the eyes see on `face` of `pos`, in reach, if there is one.
pub fn visible_point(bot: &Client, pos: BlockPos, face: Direction) -> Option<Vec3> {
    face_points(bot, pos, face).into_iter().find(|&p| sees(bot, pos, face, p))
}

pub const FACES: [Direction; 6] = [Direction::Down, Direction::Up, Direction::North, Direction::South, Direction::West, Direction::East];

/// The nearest face of `pos` the eyes see in reach, and the point on it.
pub fn sight(bot: &Client, pos: BlockPos) -> Option<(Direction, Vec3)> {
    let e = eye(bot);
    let mut found: Vec<(Direction, Vec3)> = FACES.iter().filter_map(|&f| visible_point(bot, pos, f).map(|p| (f, p))).collect();
    found.sort_by(|a, b| a.1.distance_squared_to(e).total_cmp(&b.1.distance_squared_to(e)));
    found.into_iter().next()
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
    // the point must be on the block (the server's check: within 1.0000001 of the centre
    // on every axis; a slab's top is inside the cube, not on its top face)
    let c = against.center();
    if [point.x - c.x, point.y - c.y, point.z - c.z].iter().any(|d| d.abs() > 1.0) {
        bail!("click point {point} is not on {against}");
    }
    if !sees(bot, against, face, point) {
        bail!("{face:?} face of {against} is not in sight at {point}");
    }
    look(bot, point).await;
    // looked from where we are now: a step during the look can have hidden it
    if !sees(bot, against, face, point) {
        bail!("{face:?} face of {against} went out of sight");
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
