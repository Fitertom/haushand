//! Underground: a staircase down to diamond level, strip mining for the ores the eyes see,
//! obsidian cast from lava with two buckets, and a room for the portal.
//!
//! Every block is dug by the bot itself (`dig::dig`: a face the eyes see, in reach), one at a
//! time and only after it was checked: nothing liquid in it or next to it (a liquid would
//! run into the gap), a floor under the next spot (put down when missing, so there is never
//! a drop). The pathfinder only walks, through what is open, never digging (its digging is
//! azalea's and checks none of this); one-block steps are our own walking.
//!
//! What counts as seen is what a player sees: a block whose face our own ray from the eyes
//! reaches with nothing in between (lava stops the ray, it is opaque), not whatever the
//! client has loaded inside the rock.

use std::time::Duration;

use azalea::{
    BlockPos, Client, Vec3,
    block::{BlockState, fluid_state::{FluidKind, FluidState}},
    core::direction::Direction,
    entity::{LookDirection, Physics},
    interact::BlockStatePredictionHandler,
    pathfinder::{
        PathfinderOpts,
        goals::{BlockPosGoal, Goal, RadiusGoal},
        moves,
    },
    physics::collision::BlockWithShape,
    prelude::*,
    protocol::packets::game::{s_interact::InteractionHand, s_use_item::ServerboundUseItem},
    registry::builtin::{BlockKind, ItemKind},
};
use eyre::{Result, bail, eyre};
use tracing::{debug, info, warn};

use crate::{
    click::{self, Fluids, REACH},
    craft::{self, items},
    inv::{self, Items},
    place,
    world::{block_at, drops_near, feet, me, stop_path, world_lock},
};

/// Feet level of the strip mine. Diamonds get more common down to -64, but below -54
/// caves are filled with lava instead of air (the lava aquifer level); at -54 the tunnel's
/// floor is the top of those lava caves (lava does not flow up, and a pool under the floor
/// is where obsidian is cast from) while walls and ceiling are seldom lava. Ore in the
/// walls, floor and ceiling spans -55 … -52.
pub const DIAMOND_Y: i32 = -54;

/// How far a seen ore may be for a detour to it (blocks).
const ORE_RADIUS: i32 = 10;

/// How far the eyes are taken to see in a dark cave (blocks): ores and lava farther are
/// not looked for.
const VIEW: f64 = 20.0;

/// Hotbar slot for blocks and buckets: the tools stay in 0..4 (`hotbar_tools`), so taking
/// cobblestone into the hand never swaps the pickaxe out of the hotbar.
const HAND_SLOT: u8 = 8;

const SIDES: [BlockPos; 4] =
    [BlockPos { x: 1, y: 0, z: 0 }, BlockPos { x: -1, y: 0, z: 0 }, BlockPos { x: 0, y: 0, z: 1 }, BlockPos { x: 0, y: 0, z: -1 }];
const UP: BlockPos = BlockPos { x: 0, y: 1, z: 0 };
const DOWN: BlockPos = BlockPos { x: 0, y: -1, z: 0 };

/// A horizontal direction as a unit offset.
type Dir = BlockPos;

fn turn_right(d: Dir) -> Dir {
    // east (+x) -> south (+z) -> west -> north
    BlockPos::new(-d.z, 0, d.x)
}

fn neighbours(p: BlockPos) -> [BlockPos; 6] {
    [p + UP, p + DOWN, p + SIDES[0], p + SIDES[1], p + SIDES[2], p + SIDES[3]]
}

fn normal(face: Direction) -> BlockPos {
    let n = face.normal();
    BlockPos::new(n.x, n.y, n.z)
}

fn face_towards(d: BlockPos) -> Direction {
    match (d.x, d.y, d.z) {
        (1, _, _) => Direction::East,
        (-1, _, _) => Direction::West,
        (_, 1, _) => Direction::Up,
        (_, -1, _) => Direction::Down,
        (_, _, 1) => Direction::South,
        _ => Direction::North,
    }
}

pub fn fluid(state: BlockState) -> FluidKind {
    FluidState::from(state).kind
}

fn is_source(state: BlockState, kind: FluidKind) -> bool {
    let f = FluidState::from(state);
    f.kind == kind && f.amount == 8 && matches!(BlockKind::from(state), BlockKind::Lava | BlockKind::Water)
}

/// A block one can stand on: a full collision box that is not a hazard.
fn solid(state: BlockState) -> bool {
    state.is_collision_shape_full() && fluid(state) == FluidKind::Empty && BlockKind::from(state) != BlockKind::MagmaBlock
}

/// Nothing to dig and nothing that hurts or holds us: air, cave air, a torch, lichen.
fn open(state: BlockState) -> bool {
    state.is_air() || (state.is_collision_shape_empty() && fluid(state) == FluidKind::Empty && !hazard(state))
}

fn hazard(state: BlockState) -> bool {
    matches!(BlockKind::from(state), BlockKind::Fire | BlockKind::SoulFire | BlockKind::PowderSnow | BlockKind::SweetBerryBush | BlockKind::MagmaBlock | BlockKind::PointedDripstone)
}

fn unbreakable(bot: &Client, state: BlockState) -> bool {
    let k = BlockKind::from(state);
    let diamond = inv::count_kind(bot, ItemKind::DiamondPickaxe) + inv::count_kind(bot, ItemKind::NetheritePickaxe) > 0;
    !crate::dig::breakable(state)
        || matches!(k, BlockKind::Bedrock | BlockKind::Barrier | BlockKind::ReinforcedDeepslate | BlockKind::EndPortalFrame | BlockKind::Spawner)
        || (!diamond && matches!(k, BlockKind::Obsidian | BlockKind::CryingObsidian | BlockKind::AncientDebris))
}

fn is_falling(state: BlockState) -> bool {
    matches!(BlockKind::from(state), BlockKind::Gravel | BlockKind::Sand | BlockKind::RedSand | BlockKind::SuspiciousGravel | BlockKind::SuspiciousSand)
}

/// Why a block may not be opened: a liquid next to it would run into the gap. A liquid
/// right below is no danger (liquids do not flow up) when `below_ok`.
fn liquid_next_to(bot: &Client, pos: BlockPos, below_ok: bool) -> Option<(BlockPos, FluidKind)> {
    let w = world_lock(bot);
    let w = w.read();
    for n in neighbours(pos) {
        if below_ok && n == pos + DOWN {
            continue;
        }
        let k = fluid(w.get_block_state(n).unwrap_or_default());
        if k != FluidKind::Empty {
            return Some((n, k));
        }
    }
    None
}

fn eye(bot: &Client) -> Vec3 {
    click::eye(bot)
}

fn on_ground(bot: &Client) -> bool {
    bot.query_self::<&Physics, _>(|p| p.on_ground()).unwrap_or(false)
}

async fn wait_for(bot: &Client, ticks: usize, cond: impl Fn(&Client) -> bool) -> bool {
    for _ in 0..ticks {
        if cond(bot) {
            return true;
        }
        bot.wait_ticks(1).await;
    }
    cond(bot)
}

/// Takes one of `items` into the hand, into the hand slot when it is not in the hotbar.
async fn hold(bot: &Client, items: &Items) -> bool {
    let slots = inv::player_slots(bot);
    if !inv::HOTBAR.clone().any(|i| items.contains(slots[i].kind())) {
        bot.set_selected_hotbar_slot(HAND_SLOT);
        bot.wait_ticks(1).await;
    }
    inv::hold(bot, items).await
}

// ---------------------------------------------------------------- seeing

/// Whether the eyes see `pos` within `range`: our ray reaches a face of it open to air,
/// first thing it meets (lava in between stops it).
fn in_view(bot: &Client, pos: BlockPos, range: f64) -> bool {
    let e = eye(bot);
    if e.distance_to(pos.center()) > range + 1.0 {
        return false;
    }
    let off = Vec3::new(pos.x as f64, pos.y as f64, pos.z as f64);
    for face in click::FACES {
        let n = normal(face);
        if !open(block_at(bot, pos + n)) {
            continue;
        }
        let nv = Vec3::new(n.x as f64, n.y as f64, n.z as f64);
        let centre = off + Vec3::new(0.5, 0.5, 0.5) + nv * 0.5;
        // the face must look towards the eyes
        if (e - centre).dot(nv) <= 0.0 {
            continue;
        }
        // the middle and points near the corners (spread on the two axes along the face)
        let (u, v) = match face {
            Direction::Up | Direction::Down => (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0)),
            Direction::North | Direction::South => (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            _ => (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)),
        };
        for (a, b) in [(0.0, 0.0), (0.35, 0.35), (-0.35, 0.35), (0.35, -0.35), (-0.35, -0.35)] {
            let p = centre + u * a + v * b;
            let d = p - e;
            let len = d.length();
            if len > range {
                continue;
            }
            if click::raycast_fluids(bot, e, d * (1.0 / len), len + 0.05, Fluids::Any).is_some_and(|h| h.pos == pos) {
                return true;
            }
        }
    }
    false
}

/// The fluid source `pos` as a bucket's pick meets it from the eyes: a point on its surface
/// the server's trace (sources stop it) reaches first, in reach.
fn fluid_aim(bot: &Client, pos: BlockPos) -> Option<Vec3> {
    let e = eye(bot);
    let top = {
        let f = FluidState::from(block_at(bot, pos));
        let same_above = FluidState::from(block_at(bot, pos + UP)).kind == f.kind;
        if same_above { 1.0 } else { f.amount as f64 / 9.0 }
    };
    let base = Vec3::new(pos.x as f64, pos.y as f64, pos.z as f64);
    for (x, z) in [(0.5, 0.5), (0.3, 0.3), (0.7, 0.3), (0.3, 0.7), (0.7, 0.7)] {
        for y in [top - 0.02, top * 0.5] {
            let p = base + Vec3::new(x, y, z);
            let d = p - e;
            let len = d.length();
            if len > REACH - 0.1 {
                continue;
            }
            if click::raycast_fluids(bot, e, d * (1.0 / len), REACH, Fluids::SourceOnly).is_some_and(|h| h.pos == pos) {
                return Some(p);
            }
        }
    }
    None
}

// ---------------------------------------------------------------- moving

/// Walks to stand at `pos` through what is already open: no digging on the way.
pub async fn walk_to(bot: &Client, pos: BlockPos) -> Result<()> {
    if feet(bot) == pos {
        return Ok(());
    }
    // a step to a neighbouring block: our own walk, the pathfinder for anything longer
    let d = pos - feet(bot);
    if d.x.abs() + d.z.abs() <= 1 && d.y.abs() <= 1 && step(bot, pos).await.is_ok() {
        return Ok(());
    }
    let secs = 8.0 + me(bot).distance_to(pos.center()) * 0.6;
    goto(bot, BlockPosGoal(pos), Duration::from_secs_f64(secs)).await?;
    if feet(bot) != pos {
        bail!("walked to {} instead of {pos}", feet(bot));
    }
    Ok(())
}

/// One step onto the block `target` next to us (level, one up or one down): face it, walk,
/// jump for a step up, stop on it.
async fn step(bot: &Client, target: BlockPos) -> Result<()> {
    let c = target.center();
    let up = target.y > feet(bot).y;
    for _ in 0..40 {
        let p = me(bot);
        let (dx, dz) = (c.x - p.x, c.z - p.z);
        let flat = (dx * dx + dz * dz).sqrt();
        if feet(bot) == target && (flat < 0.3 || !up) && on_ground(bot) && flat < 0.45 {
            break;
        }
        let yaw = (-dx).atan2(dz).to_degrees() as f32;
        {
            let mut ecs = bot.ecs.write();
            if let Some(mut l) = ecs.get_mut::<LookDirection>(bot.entity) {
                let pitch = l.x_rot();
                *l = LookDirection::new(yaw, pitch);
            }
        }
        if flat > 0.15 {
            bot.walk(azalea::WalkDirection::Forward);
        } else {
            bot.walk(azalea::WalkDirection::None);
        }
        if up && feet(bot).y < target.y && on_ground(bot) && flat < 1.3 {
            bot.jump();
        }
        bot.wait_ticks(1).await;
    }
    bot.walk(azalea::WalkDirection::None);
    wait_for(bot, 10, on_ground).await;
    if feet(bot) != target {
        bail!("stepped to {} instead of {target}", feet(bot));
    }
    Ok(())
}

/// The pathfinder with our underground moves (never digging down, no jumps over gaps) and
/// no digging at all, with a deadline; gives up as well after 100 game ticks without
/// moving (a goal it cannot reach).
async fn goto(bot: &Client, goal: impl Goal + 'static, limit: Duration) -> Result<()> {
    let opts = PathfinderOpts::new().allow_mining(false).successors_fn(moves::underground_move);
    let stuck = async {
        let mut last = me(bot);
        loop {
            bot.wait_ticks(100).await;
            let now = me(bot);
            if now.distance_to(last) < 0.5 {
                return;
            }
            last = now;
        }
    };
    let r = tokio::select! {
        r = tokio::time::timeout(limit, bot.goto_with_opts(goal, opts)) => r.map_err(|_| eyre!("path timed out")),
        () = stuck => Err(eyre!("path stuck: no move in 100 ticks")),
    };
    if r.is_err() {
        stop_path(bot).await;
        bot.walk(azalea::WalkDirection::None);
    }
    r
}

// ---------------------------------------------------------------- digging and sealing

/// Breaks the block at `pos` (in reach and in sight) with the best tool. Refuses when a
/// liquid is next to it; breaks again when sand or gravel falls into the gap.
pub async fn dig(bot: &Client, pos: BlockPos, below_ok: bool) -> Result<()> {
    for _ in 0..16 {
        let state = block_at(bot, pos);
        if open(state) {
            return Ok(());
        }
        if fluid(state) != FluidKind::Empty {
            bail!("{pos} is {:?}", BlockKind::from(state));
        }
        if unbreakable(bot, state) {
            bail!("{pos} is unbreakable {:?}", BlockKind::from(state));
        }
        if let Some((n, k)) = liquid_next_to(bot, pos, below_ok) {
            bail!("{k:?} at {n} next to {pos}");
        }
        // gravel over a block above our head would come down on us
        let f = feet(bot);
        if pos.x == f.x && pos.z == f.z && pos.y > f.y + 1 && is_falling(block_at(bot, pos + UP)) {
            bail!("{:?} over {pos}, right above us", BlockKind::from(block_at(bot, pos + UP)));
        }
        let falls_in = is_falling(block_at(bot, pos + UP));
        crate::dig::dig(bot, pos).await?;
        if falls_in {
            // gravel turns into a falling entity a tick or two later and lands in the gap
            bot.wait_ticks(10).await;
        }
    }
    bail!("{pos} keeps filling up")
}

/// Puts cobblestone (or another stone block) at `pos`, clicked against a face seen from
/// where we stand.
async fn put_stone(bot: &Client, pos: BlockPos) -> Result<()> {
    let stone = craft::stone_material();
    if inv::count(bot, &stone) == 0 {
        bail!("no cobblestone to put at {pos}");
    }
    // place_at walks (the pathfinder, digging) when no face is seen: not down here
    if place::clicks_for(bot, pos).is_empty() {
        bail!("no face seen to put a block at {pos}");
    }
    if !hold(bot, &stone).await {
        bail!("no cobblestone in hand");
    }
    place::place_at(bot, &stone, pos).await
}

/// After each move: lava (or water) in a cell around our body gets a block put in it, so
/// it cannot reach us. Returns an error when that failed.
async fn seal_around(bot: &Client) -> Result<()> {
    let f = feet(bot);
    let mut cells = vec![f + DOWN, f + UP + UP];
    for s in SIDES {
        cells.push(f + s);
        cells.push(f + UP + s);
    }
    for c in cells {
        let k = fluid(block_at(bot, c));
        if k == FluidKind::Lava || (k == FluidKind::Water && c != f + DOWN) {
            warn!("{k:?} at {c} next to us, sealing it");
            if let Err(e) = put_stone(bot, c).await {
                bail!("could not seal {k:?} at {c}: {e}");
            }
        }
    }
    if fluid(block_at(bot, f)) == FluidKind::Lava {
        bail!("standing in lava at {f}");
    }
    Ok(())
}

// ---------------------------------------------------------------- staircase and tunnel

/// What stopped a step.
#[derive(Debug)]
enum Blocked {
    /// A liquid or an unbreakable block in the way: try another direction.
    Way(String),
    /// Lava under the next block, nothing else wrong: a place to fill a bucket from.
    LavaBelow(BlockPos),
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocked::Way(s) => write!(f, "{s}"),
            Blocked::LavaBelow(p) => write!(f, "lava below at {p}"),
        }
    }
}

fn way(e: impl std::fmt::Display) -> Blocked {
    Blocked::Way(e.to_string())
}

/// Checks a block before a step opens it: nothing liquid in it or next to it.
fn check_open(bot: &Client, pos: BlockPos, below_ok: bool) -> Result<(), Blocked> {
    let st = block_at(bot, pos);
    if fluid(st) != FluidKind::Empty || hazard(st) {
        return Err(Blocked::Way(format!("{:?} at {pos}", BlockKind::from(st))));
    }
    if open(st) {
        // already open: whatever is next to it is already let in
        return Ok(());
    }
    if unbreakable(bot, st) {
        return Err(Blocked::Way(format!("unbreakable {:?} at {pos}", BlockKind::from(st))));
    }
    if let Some((n, k)) = liquid_next_to(bot, pos, below_ok) {
        return Err(Blocked::Way(format!("{k:?} at {n} next to {pos}")));
    }
    Ok(())
}

/// The floor of a step: solid, or made solid with cobblestone. Lava there is reported,
/// water stops the step.
fn check_floor(bot: &Client, pos: BlockPos) -> Result<bool, Blocked> {
    let st = block_at(bot, pos);
    if solid(st) {
        return Ok(true);
    }
    match fluid(st) {
        FluidKind::Lava => Err(Blocked::LavaBelow(pos)),
        FluidKind::Water => Err(Blocked::Way(format!("water floor at {pos}"))),
        FluidKind::Empty if hazard(st) => Err(Blocked::Way(format!("{:?} floor at {pos}", BlockKind::from(st)))),
        FluidKind::Empty => Ok(false),
    }
}

/// One step of a staircase from where we stand in direction `d`, one block down: the head,
/// feet and below-feet blocks in front are dug (3 high, so we walk in and drop one), a
/// floor is put under the new spot if there is none.
async fn stair_step(bot: &Client, d: Dir) -> Result<(), Blocked> {
    let s = feet(bot);
    let (a, b, c) = (s + d + UP, s + d, s + d + DOWN);
    let floor = c + DOWN;
    for p in [a, b, c] {
        check_open(bot, p, false)?;
    }
    let has_floor = check_floor(bot, floor)?;
    for p in [a, b, c] {
        dig(bot, p, false).await.map_err(way)?;
    }
    if !has_floor {
        put_stone(bot, floor).await.map_err(|e| Blocked::Way(format!("no floor: {e}")))?;
    }
    walk_to(bot, c).await.map_err(way)?;
    seal_around(bot).await.map_err(way)?;
    Ok(())
}

/// One step of a flat 1×2 tunnel. Lava as the next block's floor: with `cover_lava` a
/// block is put on it (the step goes on), else the two blocks over it are dug and
/// `LavaBelow` is returned (we stand at its edge then, which is what casting wants).
async fn tunnel_step(bot: &Client, d: Dir, cover_lava: bool) -> Result<(), Blocked> {
    let s = feet(bot);
    let (a, b) = (s + d + UP, s + d);
    let floor = b + DOWN;
    check_open(bot, a, false)?;
    let floor_state = check_floor(bot, floor);
    let lava_floor = matches!(floor_state, Err(Blocked::LavaBelow(_)));
    check_open(bot, b, lava_floor)?;
    let has_floor = match floor_state {
        Err(Blocked::LavaBelow(p)) => {
            for q in [a, b] {
                dig(bot, q, q == b).await.map_err(way)?;
            }
            if !cover_lava {
                return Err(Blocked::LavaBelow(p));
            }
            false
        }
        r => r?,
    };
    for q in [a, b] {
        dig(bot, q, false).await.map_err(way)?;
    }
    if !has_floor {
        put_stone(bot, floor).await.map_err(|e| Blocked::Way(format!("no floor: {e}")))?;
    }
    walk_to(bot, b).await.map_err(way)?;
    seal_around(bot).await.map_err(way)?;
    Ok(())
}

/// The direction we face, rounded to a side.
fn facing(bot: &Client) -> Dir {
    let yaw = bot.query_self::<&LookDirection, _>(|l| l.y_rot()).unwrap_or(0.0);
    // yaw 0 looks south (+z), 90 west (-x), 180 north, 270 east
    let q = (((yaw.rem_euclid(360.0) + 45.0) / 90.0) as i32) % 4;
    [BlockPos::new(0, 0, 1), BlockPos::new(-1, 0, 0), BlockPos::new(0, 0, -1), BlockPos::new(1, 0, 0)][q as usize]
}

/// A spot to start a staircase from with no water or lava near: started on a beach or a
/// river bank, every direction ran into water within a step.
fn dry_spot(bot: &Client, radius: i32) -> Option<BlockPos> {
    let f = feet(bot);
    let w = world_lock(bot);
    let w = w.read();
    let get = |p: BlockPos| w.get_block_state(p).unwrap_or_default();
    let wet = |p: BlockPos| {
        for dx in -4..=4 {
            for dz in -4..=4 {
                for dy in -5..=1 {
                    if fluid(get(BlockPos::new(p.x + dx, p.y + dy, p.z + dz))) != FluidKind::Empty {
                        return true;
                    }
                }
            }
        }
        false
    };
    let mut best: Option<(i32, BlockPos)> = None;
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            for dy in -3..=3 {
                let p = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                let d = dx * dx + dz * dz + dy * dy;
                if best.is_some_and(|(bd, _)| bd <= d) {
                    continue;
                }
                if solid(get(p + DOWN)) && open(get(p)) && open(get(p + UP)) && !wet(p) {
                    best = Some((d, p));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Digs a staircase down until our feet are at `target_y`.
pub async fn descend(bot: &Client, target_y: i32) -> Result<()> {
    crate::obtain::hotbar_tools(bot).await;
    if feet(bot).y > 40
        && let Some(p) = dry_spot(bot, 2).filter(|&p| p == feet(bot)).or_else(|| dry_spot(bot, 24))
        && p != feet(bot)
    {
        info!("starting the staircase away from water, at {p}");
        if let Err(e) = goto(bot, BlockPosGoal(p), Duration::from_secs(40)).await {
            info!("could not walk to {p}: {e}");
        }
    }
    let mut d = facing(bot);
    let mut fails = 0;
    let start = feet(bot);
    let t0 = bot.ticks_connected();
    info!("descending from {start} to y {target_y}");
    while feet(bot).y > target_y {
        if inv::count_kind(bot, ItemKind::WaterBucket) == 0 {
            take_seen_water(bot).await;
        }
        match stair_step(bot, d).await {
            Ok(()) => {
                fails = 0;
                debug!("stair at {}", feet(bot));
            }
            Err(e) => {
                info!("stair step {d:?} from {} blocked: {e}", feet(bot));
                fails += 1;
                d = turn_right(d);
                if fails % 4 == 0 {
                    // every side is blocked: move along level a bit and try again
                    let mut moved = false;
                    for _ in 0..4 {
                        if tunnel_step(bot, d, true).await.is_ok() {
                            moved = true;
                            break;
                        }
                        d = turn_right(d);
                    }
                    if !moved || fails > 16 {
                        bail!("stuck descending at {}", feet(bot));
                    }
                }
            }
        }
    }
    info!("at y {} after a staircase from {start}, {} ticks", feet(bot).y, bot.ticks_connected() - t0);
    Ok(())
}

// ---------------------------------------------------------------- ores

/// An ore worth a detour, and until how many of its drop.
struct Want {
    blocks: &'static [BlockKind],
    drop: ItemKind,
    until: i32,
    /// Further than this from us it is left.
    radius: i32,
}

const WANTED: &[Want] = &[
    Want { blocks: &[BlockKind::DiamondOre, BlockKind::DeepslateDiamondOre], drop: ItemKind::Diamond, until: 64, radius: ORE_RADIUS },
    // iron for a flint and steel
    Want { blocks: &[BlockKind::IronOre, BlockKind::DeepslateIronOre], drop: ItemKind::RawIron, until: 3, radius: 3 },
];

/// Ores of interest the eyes see, nearest first (diamonds before the rest).
fn seen_ores(bot: &Client, failed: &[BlockPos]) -> Vec<(BlockPos, usize)> {
    let f = feet(bot);
    let wants: Vec<(usize, &Want)> =
        WANTED.iter().enumerate().filter(|(_, w)| inv::count_kind(bot, w.drop) < w.until).collect();
    if wants.is_empty() {
        return Vec::new();
    }
    let r = wants.iter().map(|(_, w)| w.radius).max().unwrap_or(0);
    let mut found = Vec::new();
    {
        let w = world_lock(bot);
        let w = w.read();
        let get = |p: BlockPos| w.get_block_state(p).unwrap_or_default();
        for dx in -r..=r {
            for dy in -r..=r {
                for dz in -r..=r {
                    let p = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                    let kind = BlockKind::from(get(p));
                    let Some((i, want)) = wants.iter().find(|(_, w)| w.blocks.contains(&kind)) else { continue };
                    if dx.abs().max(dy.abs()).max(dz.abs()) > want.radius || failed.contains(&p) {
                        continue;
                    }
                    // a drop falling into lava or water is lost or carried off
                    if !solid(get(p + DOWN)) && !open(get(p + DOWN)) {
                        continue;
                    }
                    found.push((p, *i));
                }
            }
        }
    }
    found.retain(|&(p, _)| in_view(bot, p, VIEW));
    let here = me(bot);
    found.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.center().distance_squared_to(here).total_cmp(&b.0.center().distance_squared_to(here))));
    found
}

/// A spot to stand at to mine `pos`: open, two high, on a floor, no liquid at it, with the
/// ore's centre in reach of the eyes; nearest to us first.
fn stands_for(bot: &Client, pos: BlockPos) -> Vec<BlockPos> {
    let here = me(bot);
    let mut out = Vec::new();
    for dx in -3..=3 {
        for dy in -3..=1 {
            for dz in -3..=3 {
                let s = BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                if !(solid(block_at(bot, s + DOWN)) && open(block_at(bot, s)) && open(block_at(bot, s + UP))) {
                    continue;
                }
                if liquid_next_to(bot, s, false).is_some() || liquid_next_to(bot, s + UP, false).is_some() {
                    continue;
                }
                let e = s.center() + Vec3::new(0.0, 1.12, 0.0);
                if e.distance_to(pos.center()) <= REACH - 0.3 {
                    out.push(s);
                }
            }
        }
    }
    out.sort_by(|a, b| a.center().distance_squared_to(here).total_cmp(&b.center().distance_squared_to(here)));
    out
}

/// Digs whatever stands first on the line from the eyes to `pos`, when it is in reach and
/// safe to open: an ore one block behind the tunnel wall, the next of a vein.
async fn dig_towards(bot: &Client, pos: BlockPos) -> Result<()> {
    for _ in 0..2 {
        if click::sight(bot, pos).is_some() {
            return Ok(());
        }
        let e = eye(bot);
        let d = pos.center() - e;
        let Some(hit) = click::raycast_fluids(bot, e, d.normalize(), d.length(), Fluids::Any) else { bail!("nothing between us and {pos}?") };
        if hit.pos == pos {
            bail!("{pos} seen only past the reach");
        }
        let f = feet(bot);
        if hit.pos == f + DOWN || fluid(block_at(bot, hit.pos)) != FluidKind::Empty {
            bail!("{} is in the way of {pos}", hit.pos);
        }
        dig(bot, hit.pos, false).await?;
    }
    if click::sight(bot, pos).is_none() {
        bail!("{pos} still not in sight");
    }
    Ok(())
}

/// Mines one seen ore: from here when it is in sight, else after a walk (no digging on the
/// way) to a spot near it. Picks up what drops.
async fn mine_ore(bot: &Client, pos: BlockPos) -> Result<()> {
    if let Some((n, k)) = liquid_next_to(bot, pos, false) {
        bail!("{k:?} next to the ore at {n}");
    }
    if click::sight(bot, pos).is_none() && dig_towards(bot, pos).await.is_err() {
        let stands = stands_for(bot, pos);
        let mut ok = false;
        for s in stands.into_iter().take(3) {
            let secs = 10.0 + me(bot).distance_to(s.center());
            if goto(bot, BlockPosGoal(s), Duration::from_secs_f64(secs)).await.is_err() || feet(bot) != s {
                continue;
            }
            seal_around(bot).await?;
            if click::sight(bot, pos).is_some() || dig_towards(bot, pos).await.is_ok() {
                ok = true;
                break;
            }
        }
        if !ok {
            bail!("no spot found to mine {pos} from");
        }
    }
    dig(bot, pos, false).await?;
    collect(bot, pos.center()).await;
    Ok(())
}

/// Walks onto the items dropped near `around`, without digging.
async fn collect(bot: &Client, around: Vec3) {
    for _ in 0..6 {
        bot.wait_ticks(3).await;
        let Some(&target) = drops_near(bot, around, 5.0).first() else { return };
        if target.distance_to(me(bot)) < 1.0 {
            bot.wait_ticks(6).await;
            if drops_near(bot, around, 5.0).first().is_some_and(|p| p.distance_to(me(bot)) < 1.0) {
                return; // not picked up while standing on it: inventory full or still in the air
            }
            continue;
        }
        let cell = BlockPos::from(target);
        // never into a liquid or a hole we cannot climb out of
        if !solid(block_at(bot, cell + DOWN)) && !solid(block_at(bot, cell)) {
            let below = block_at(bot, cell + DOWN);
            if fluid(below) != FluidKind::Empty || fluid(block_at(bot, cell)) != FluidKind::Empty {
                return;
            }
        }
        if goto(bot, RadiusGoal::new(target, 0.6), Duration::from_secs(10)).await.is_err() {
            return;
        }
    }
}

/// Mines every seen ore of interest near us. Returns how many it got.
async fn mine_seen(bot: &Client, failed: &mut Vec<BlockPos>, done: &(dyn Fn(&Client) -> bool + Sync)) -> usize {
    let mut n = 0;
    while !done(bot) {
        let Some(&(pos, _)) = seen_ores(bot, failed).first() else { break };
        let kind = BlockKind::from(block_at(bot, pos));
        info!("ore {kind:?} at {pos}, {:.1} away", pos.center().distance_to(me(bot)));
        match mine_ore(bot, pos).await {
            Ok(()) => {
                n += 1;
                info!("mined {kind:?}; diamonds {} raw iron {}", inv::count_kind(bot, ItemKind::Diamond), inv::count_kind(bot, ItemKind::RawIron));
            }
            Err(e) => {
                info!("left the ore at {pos}: {e}");
                failed.push(pos);
            }
        }
    }
    n
}

/// Strip mines at `y` until `done`: a straight 1×2 tunnel, turning where liquid or
/// bedrock is ahead, mining every seen ore of interest on the way.
async fn strip_mine(bot: &Client, y: i32, done: &(dyn Fn(&Client) -> bool + Sync), max_steps: usize) -> Result<()> {
    crate::obtain::hotbar_tools(bot).await;
    let mut failed = Vec::new();
    let mut d = facing(bot);
    let mut steps = 0;
    let mut fails = 0;
    let t0 = bot.ticks_connected();
    loop {
        mine_seen(bot, &mut failed, done).await;
        if done(bot) {
            info!("strip mine done: {steps} steps, {} ticks", bot.ticks_connected() - t0);
            return Ok(());
        }
        if inv::count_kind(bot, ItemKind::WaterBucket) == 0 {
            take_seen_water(bot).await;
        }
        // a detour took us up: back down
        if feet(bot).y > y + 1 {
            descend(bot, y).await?;
        }
        if steps >= max_steps {
            bail!("strip mined {steps} steps without enough");
        }
        match tunnel_step(bot, d, true).await {
            Ok(()) => {
                steps += 1;
                fails = 0;
                if steps % 20 == 0 {
                    info!("strip mine: {steps} steps, at {}, {} ticks", feet(bot), bot.ticks_connected() - t0);
                }
            }
            Err(e) => {
                info!("tunnel {d:?} from {} blocked: {e}", feet(bot));
                fails += 1;
                // turn, and alternate the side so the tunnel snakes on instead of circling
                d = if fails % 2 == 1 { turn_right(d) } else { turn_right(turn_right(d)) };
                if fails > 8 {
                    bail!("strip mine stuck at {}", feet(bot));
                }
            }
        }
        if inv::free_slots(bot) <= 1 {
            drop_junk(bot).await;
        }
    }
}

/// Throws away stone-like blocks beyond a stack, so drops of value still fit.
async fn drop_junk(bot: &Client) {
    let junk = items!(Cobblestone, CobbledDeepslate, Dirt, Gravel, Andesite, Diorite, Granite, Tuff, Calcite);
    let mut kept = 0;
    for (slot, _, n) in inv::slots_of(bot, &junk) {
        if kept < 64 {
            kept += n;
            continue;
        }
        if let Some(h) = craft::open_own_inventory(bot).await {
            h.click(azalea::inventory::operations::ThrowClick::All { slot: slot as u16 });
        }
        bot.wait_ticks(2).await;
    }
}

/// Have `want` diamonds: down to diamond level if not there, then strip mine.
pub async fn diamonds(bot: &Client, want: i32) -> Result<()> {
    if inv::count_kind(bot, ItemKind::Diamond) >= want {
        return Ok(());
    }
    if feet(bot).y > DIAMOND_Y + 1 {
        descend(bot, DIAMOND_Y).await?;
    }
    strip_mine(bot, feet(bot).y.min(DIAMOND_Y), &|bot| inv::count_kind(bot, ItemKind::Diamond) >= want, 1500).await
}

// ---------------------------------------------------------------- buckets

fn buckets(bot: &Client) -> (i32, i32, i32) {
    (inv::count_kind(bot, ItemKind::Bucket), inv::count_kind(bot, ItemKind::WaterBucket), inv::count_kind(bot, ItemKind::LavaBucket))
}

/// The unit vector of a look, as the server computes it from yaw and pitch
/// (`Entity.calculateViewVector`), so our trace follows the same line as its.
fn view_vector(yaw: f32, pitch: f32) -> Vec3 {
    let p = pitch.to_radians();
    let y = -yaw.to_radians();
    Vec3::new((y.sin() * p.cos()) as f64, (-p.sin()) as f64, (y.cos() * p.cos()) as f64)
}

/// A bucket use: what the server's trace must hit, and the cell it changes.
#[derive(Clone, Copy, Debug)]
struct BucketUse {
    /// The bucket held: an empty one fills, a full one pours.
    hold: ItemKind,
    /// Where to look.
    aim: Vec3,
    /// The block the server's trace must stop at (a fluid source to fill from, or the
    /// block whose face the fluid is poured against).
    hit: BlockPos,
    /// For a pour: the face of `hit` that must be hit; the fluid lands in front of it.
    face: Option<Direction>,
    /// The cell that changes, and the fluid in it afterwards (Empty for a fill).
    cell: BlockPos,
    after: FluidKind,
}

/// Uses a bucket. Buckets act on `ServerboundUseItem`: the server traces from our eyes
/// along the look the packet carries (`getPlayerPOVHitResult`: block outlines, and fluid
/// sources for an empty bucket), as far as the block interaction range. So the look is set
/// first (ours, see `click::look`), the same trace is run here with our own ray and must
/// hit what was meant, and the result is checked by the block update and the inventory.
async fn use_bucket(bot: &Client, u: BucketUse) -> Result<()> {
    if !hold(bot, &Items::one(u.hold)).await {
        bail!("no {:?} to use", u.hold);
    }
    let before = inv::count_kind(bot, u.hold);
    // stand still: the server traces from where it has us
    bot.walk(azalea::WalkDirection::None);
    click::look(bot, u.aim).await;
    let (yaw, pitch) = click::angles(eye(bot), u.aim);
    let fluids = if u.hold == ItemKind::Bucket { Fluids::SourceOnly } else { Fluids::None };
    let hit = click::raycast_fluids(bot, eye(bot), view_vector(yaw, pitch), REACH, fluids);
    let Some(hit) = hit else { bail!("{:?} aimed at {} would hit nothing", u.hold, u.hit) };
    if hit.pos != u.hit || u.face.is_some_and(|f| f != hit.face) {
        bail!("{:?} aimed at {} would hit {} {:?}", u.hold, u.hit, hit.pos, hit.face);
    }
    let seq = {
        let mut ecs = bot.ecs.write();
        ecs.get_mut::<BlockStatePredictionHandler>(bot.entity).map(|mut h| h.start_predicting()).unwrap_or(1)
    };
    bot.write_packet(ServerboundUseItem { hand: InteractionHand::MainHand, seq, y_rot: yaw, x_rot: pitch });
    let done = |bot: &Client| {
        // a fill is told by the bucket alone: a source in a lake fills again from the
        // sources around it at once
        let cell_ok = u.after == FluidKind::Empty || fluid(block_at(bot, u.cell)) == u.after;
        cell_ok && inv::count_kind(bot, u.hold) < before
    };
    if wait_for(bot, 40, done).await {
        return Ok(());
    }
    bail!("{:?} at {} did not change {} to {:?} (now {:?})", u.hold, u.hit, u.cell, u.after, BlockKind::from(block_at(bot, u.cell)))
}

/// Fills a bucket from the source `pos` (in reach and in a bucket's sight).
async fn fill(bot: &Client, pos: BlockPos) -> Result<()> {
    let Some(aim) = fluid_aim(bot, pos) else { bail!("source at {pos} not in sight") };
    use_bucket(bot, BucketUse { hold: ItemKind::Bucket, aim, hit: pos, face: None, cell: pos, after: FluidKind::Empty }).await
}

/// A water source in reach and sight, filled into an empty bucket, when there is one.
async fn take_seen_water(bot: &Client) {
    if inv::count_kind(bot, ItemKind::Bucket) == 0 {
        return;
    }
    let f = feet(bot);
    for dx in -4..=4 {
        for dy in -3..=4 {
            for dz in -4..=4 {
                let p = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                if is_source(block_at(bot, p), FluidKind::Water) && fluid_aim(bot, p).is_some() {
                    match fill(bot, p).await {
                        Ok(()) => {
                            info!("water bucket filled at {p}");
                            return;
                        }
                        Err(e) => debug!("water at {p}: {e}"),
                    }
                }
            }
        }
    }
}

/// Have a water bucket: fill one from the nearest water source in view (a lake, a river,
/// the sea). Gives up when there is none within `radius`.
pub async fn water_bucket(bot: &Client, radius: f64) -> Result<()> {
    if inv::count_kind(bot, ItemKind::WaterBucket) > 0 {
        return Ok(());
    }
    if inv::count_kind(bot, ItemKind::Bucket) == 0 {
        bail!("no bucket for water");
    }
    let source: BlockState = azalea::block::blocks::Water { level: azalea::block::properties::WaterLevel::_0 }.into();
    let states = azalea::block::BlockStates { set: [source].into_iter().collect() };
    let mut failed = Vec::new();
    for _ in 0..6 {
        let here = me(bot);
        let candidates: Vec<BlockPos> = {
            let w = world_lock(bot);
            let w = w.read();
            w.find_blocks(here, &states)
                .filter(|p| !failed.contains(p))
                // the surface: open air right above, seen from the shore
                .filter(|p| w.get_block_state(*p + UP).is_some_and(|s| s.is_air()))
                .take(60)
                .collect()
        };
        let mut candidates: Vec<BlockPos> = candidates.into_iter().filter(|p| p.center().distance_to(here) <= radius).collect();
        candidates.sort_by(|a, b| a.center().distance_squared_to(here).total_cmp(&b.center().distance_squared_to(here)));
        let Some(&pos) = candidates.first() else { bail!("no water within {radius:.0}") };
        info!("water bucket: filling at {pos}");
        // a dry spot on the shore with the source in reach
        let stand = water_stand(bot, pos);
        let r = match stand {
            Some(s) => {
                let secs = 15.0 + here.distance_to(s.center()) * 0.6;
                goto(bot, BlockPosGoal(s), Duration::from_secs_f64(secs)).await
            }
            None => Err(eyre!("no dry spot next to {pos}")),
        };
        match r {
            Ok(()) => match fill(bot, pos).await {
                Ok(()) => return Ok(()),
                Err(e) => info!("water at {pos}: {e}"),
            },
            Err(e) => info!("water at {pos}: {e}"),
        }
        failed.push(pos);
    }
    bail!("could not fill a water bucket")
}

/// A dry standing spot next to the water source `pos` (its side, one up or level).
fn water_stand(bot: &Client, pos: BlockPos) -> Option<BlockPos> {
    let here = me(bot);
    let mut out: Vec<BlockPos> = Vec::new();
    for s in SIDES {
        for dy in [1, 0] {
            for k in [1, 2] {
                let p = pos + BlockPos::new(s.x * k, dy, s.z * k);
                if solid(block_at(bot, p + DOWN)) && open(block_at(bot, p)) && open(block_at(bot, p + UP)) {
                    out.push(p);
                }
            }
        }
    }
    out.into_iter().min_by(|a, b| a.center().distance_squared_to(here).total_cmp(&b.center().distance_squared_to(here)))
}

// ---------------------------------------------------------------- obsidian

/// A place to cast obsidian: we stand at `stand`; the cast hole `hole` is the block under
/// the one beside us (`hole + UP`), stone on its four sides and under it; a bucket of water
/// emptied against `wall`'s face towards `hole + UP` lands right above the hole.
#[derive(Clone, Copy, Debug)]
struct Station {
    stand: BlockPos,
    hole: BlockPos,
    wall: BlockPos,
}

/// The lava sources in reach of the eyes and in a bucket's sight, nearest first.
fn lava_in_reach(bot: &Client, avoid: BlockPos) -> Vec<BlockPos> {
    let f = feet(bot);
    let mut out = Vec::new();
    for dx in -4..=4 {
        for dy in -4..=3 {
            for dz in -4..=4 {
                let p = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                if p != avoid && is_source(block_at(bot, p), FluidKind::Lava) && fluid_aim(bot, p).is_some() {
                    out.push(p);
                }
            }
        }
    }
    let e = eye(bot);
    out.sort_by(|a, b| a.center().distance_squared_to(e).total_cmp(&b.center().distance_squared_to(e)));
    out
}

/// Can `p` be dug (or is open) without letting a liquid in?
fn diggable(bot: &Client, p: BlockPos) -> bool {
    let st = block_at(bot, p);
    open(st) || (fluid(st) == FluidKind::Empty && !unbreakable(bot, st) && !hazard(st) && liquid_next_to(bot, p, false).is_none())
}

/// Picks a cast hole beside where we stand, away from the lava the bucket fills from.
fn plan_station(bot: &Client, lava: BlockPos) -> Option<Station> {
    let s = feet(bot);
    if !solid(block_at(bot, s + DOWN)) {
        return None;
    }
    let mut best: Option<(f64, Station)> = None;
    for side in SIDES {
        let n = s + side; // the cell above the hole, where the water goes
        let hole = n + DOWN;
        let wall = n + side;
        if hole == lava || n == lava || neighbours(hole).contains(&lava) {
            continue;
        }
        // the hole: stone on its four sides and under it
        let around_solid = SIDES.iter().all(|&q| solid(block_at(bot, hole + q))) && solid(block_at(bot, hole + DOWN));
        let wall_ok = solid(block_at(bot, wall));
        let room = diggable(bot, n) && diggable(bot, n + UP) && diggable(bot, hole);
        let no_liquid_near = SIDES.iter().all(|&q| fluid(block_at(bot, n + q)) == FluidKind::Empty && fluid(block_at(bot, n + UP + q)) == FluidKind::Empty);
        if around_solid && wall_ok && room && no_liquid_near {
            // the water spreads from above the hole: the farther from the lava the better
            let d = n.center().distance_to(lava.center());
            if best.is_none_or(|(bd, _)| d > bd) {
                best = Some((d, Station { stand: s, hole, wall }));
            }
        }
    }
    best.map(|(_, st)| st)
}

/// Digs the station's room and hole.
async fn build_station(bot: &Client, st: &Station) -> Result<()> {
    let n = st.hole + UP;
    dig(bot, n + UP, false).await?;
    dig(bot, n, false).await?;
    dig(bot, st.hole, false).await?;
    Ok(())
}

/// One obsidian: lava into the hole, water over it (the lava turns to obsidian at once),
/// the water back into its bucket before it spreads, the obsidian mined and picked up.
async fn cast_one(bot: &Client, st: &Station) -> Result<()> {
    let n = st.hole + UP;
    // 1. lava
    if inv::count_kind(bot, ItemKind::LavaBucket) == 0 {
        let Some(&lava) = lava_in_reach(bot, st.hole).first() else { bail!("no lava source in reach") };
        fill(bot, lava).await?;
    }
    if inv::count_kind(bot, ItemKind::WaterBucket) == 0 {
        bail!("no water bucket");
    }
    // 2. into the hole: aim at its bottom; a full bucket's trace ignores liquids and puts
    // the lava in the cell before the face hit
    if !open(block_at(bot, st.hole)) {
        bail!("cast hole {} not empty: {:?}", st.hole, BlockKind::from(block_at(bot, st.hole)));
    }
    let bottom = st.hole.center() + Vec3::new(0.0, -0.5, 0.0);
    use_bucket(bot, BucketUse { hold: ItemKind::LavaBucket, aim: bottom, hit: st.hole + DOWN, face: Some(Direction::Up), cell: st.hole, after: FluidKind::Lava }).await?;
    wait_for(bot, 20, |bot| is_source(block_at(bot, st.hole), FluidKind::Lava)).await;
    // 3. water against the wall's face, into the cell over the hole
    let side = st.wall - n;
    let face = n.center() + Vec3::new(side.x as f64 * 0.5, 0.1, side.z as f64 * 0.5);
    let r = use_bucket(bot, BucketUse { hold: ItemKind::WaterBucket, aim: face, hit: st.wall, face: Some(face_towards(n - st.wall)), cell: n, after: FluidKind::Water }).await;
    // 4. take the water back at once, whatever happened: spread, it would reach the lava
    // we fill from and turn that to obsidian too
    let mut back = false;
    for _ in 0..4 {
        if inv::count_kind(bot, ItemKind::WaterBucket) > 0 {
            back = true;
            break;
        }
        if is_source(block_at(bot, n), FluidKind::Water) && fill(bot, n).await.is_ok() {
            back = true;
            break;
        }
        bot.wait_ticks(1).await;
    }
    if !back {
        warn!("water at {n} not taken back");
    }
    r?;
    if BlockKind::from(block_at(bot, st.hole)) != BlockKind::Obsidian {
        bail!("cast gave {:?}", BlockKind::from(block_at(bot, st.hole)));
    }
    // 5. mine it once the flowing water is gone
    wait_for(bot, 80, |bot| fluid(block_at(bot, n)) == FluidKind::Empty && fluid(block_at(bot, st.stand)) == FluidKind::Empty).await;
    let before = inv::count_kind(bot, ItemKind::Obsidian);
    dig(bot, st.hole, false).await?;
    // 6. the drop lies at the bottom of the hole, out of pick-up range: step in, and out
    if !wait_for(bot, 6, |bot| inv::count_kind(bot, ItemKind::Obsidian) > before).await {
        step(bot, st.hole).await?;
        wait_for(bot, 20, |bot| inv::count_kind(bot, ItemKind::Obsidian) > before).await;
        walk_to(bot, st.stand).await?;
    }
    if inv::count_kind(bot, ItemKind::Obsidian) == before {
        bail!("obsidian not picked up");
    }
    Ok(())
}

/// Standing spots at the edge of seen lava: a lava source with open air over it, and next
/// to that air a spot with a floor, nearest first.
fn lava_edges(bot: &Client, radius: i32) -> Vec<BlockPos> {
    let f = feet(bot);
    let mut lava = Vec::new();
    {
        let w = world_lock(bot);
        let w = w.read();
        let get = |p: BlockPos| w.get_block_state(p).unwrap_or_default();
        for dx in -radius..=radius {
            for dy in -6..=4 {
                for dz in -radius..=radius {
                    let l = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                    if is_source(get(l), FluidKind::Lava) && get(l + UP).is_air() {
                        lava.push(l);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for l in lava {
        if !in_view(bot, l, VIEW) {
            continue;
        }
        for s in SIDES {
            let stand = l + UP + s;
            let ok = solid(block_at(bot, stand + DOWN))
                && open(block_at(bot, stand))
                && open(block_at(bot, stand + UP))
                && SIDES.iter().all(|&q| fluid(block_at(bot, stand + q)) == FluidKind::Empty && fluid(block_at(bot, stand + UP + q)) == FluidKind::Empty);
            if ok {
                out.push(stand);
            }
        }
    }
    let here = me(bot);
    out.sort_by(|a, b| a.center().distance_squared_to(here).total_cmp(&b.center().distance_squared_to(here)));
    out.dedup();
    out
}

/// Have `want` obsidian, cast from lava with a water bucket and an empty one.
pub async fn obsidian(bot: &Client, want: i32) -> Result<()> {
    if inv::count_kind(bot, ItemKind::Obsidian) >= want {
        return Ok(());
    }
    let (_, water, _) = buckets(bot);
    if water == 0 {
        take_seen_water(bot).await;
    }
    let (empty, water, lava) = buckets(bot);
    if water == 0 || empty + lava == 0 {
        bail!("need a water bucket and an empty one (have {empty} empty, {water} water, {lava} lava)");
    }
    crate::obtain::hotbar_tools(bot).await;
    let t0 = bot.ticks_connected();
    let mut failed_edges: Vec<BlockPos> = Vec::new();
    let mut d = facing(bot);
    let mut steps = 0;
    let mut fails = 0;
    while inv::count_kind(bot, ItemKind::Obsidian) < want {
        // a lava edge in sight: go there and cast
        let edge = lava_edges(bot, 12).into_iter().find(|e| !failed_edges.contains(e));
        if let Some(edge) = edge {
            info!("lava edge at {edge}");
            if let Err(e) = cast_at(bot, edge, want).await {
                info!("casting at {edge} stopped: {e}");
                failed_edges.push(edge);
            }
            continue;
        }
        // none: tunnel on, opening the lava under the floor where the tunnel meets some
        if steps > 400 {
            bail!("no lava found in {steps} steps");
        }
        match tunnel_step(bot, d, false).await {
            Ok(()) => {
                steps += 1;
                fails = 0;
            }
            Err(Blocked::LavaBelow(p)) => {
                info!("tunnel met lava under the floor at {p}");
                // the edge scan finds it now that the two blocks over it are open; if not
                // (flowing lava, not a source), cover it and go on
                if lava_edges(bot, 3).iter().all(|e| failed_edges.contains(e)) {
                    if let Err(e) = tunnel_step(bot, d, true).await {
                        info!("tunnel {d:?} blocked: {e}");
                        d = turn_right(d);
                    }
                }
            }
            Err(e) => {
                info!("tunnel {d:?} blocked: {e}");
                fails += 1;
                d = if fails % 2 == 1 { turn_right(d) } else { turn_right(turn_right(d)) };
                if fails > 8 {
                    bail!("stuck looking for lava at {}", feet(bot));
                }
            }
        }
    }
    info!("obsidian {} in {} ticks", inv::count_kind(bot, ItemKind::Obsidian), bot.ticks_connected() - t0);
    Ok(())
}

/// Casts obsidian standing at `edge` until `want` or the lava in reach runs out.
async fn cast_at(bot: &Client, edge: BlockPos, want: i32) -> Result<()> {
    if feet(bot) != edge {
        walk_to(bot, edge).await?;
    }
    seal_around(bot).await?;
    let Some(&lava) = lava_in_reach(bot, BlockPos::new(0, -1000, 0)).first() else { bail!("no lava in reach from {edge}") };
    let Some(st) = plan_station(bot, lava) else { bail!("no room for a cast hole at {edge}") };
    info!("cast station {st:?}, lava at {lava}");
    build_station(bot, &st).await?;
    let mut fails = 0;
    while inv::count_kind(bot, ItemKind::Obsidian) < want {
        match cast_one(bot, &st).await {
            Ok(()) => {
                fails = 0;
                info!("obsidian {}/{want}", inv::count_kind(bot, ItemKind::Obsidian));
            }
            Err(e) => {
                fails += 1;
                info!("cast failed: {e}");
                if feet(bot) != st.stand {
                    walk_to(bot, st.stand).await?;
                }
                if fails >= 3 || lava_in_reach(bot, st.hole).is_empty() && inv::count_kind(bot, ItemKind::LavaBucket) == 0 {
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- portal room

/// Every block of a portal site that must be open: the frame (4×5) and the two rows in
/// front of it where the bot stands to build.
fn site_cells(site: &crate::portal::Site) -> Vec<BlockPos> {
    let mut v: Vec<BlockPos> = (0..4).flat_map(|i| (0..5).map(move |j| (i, j))).map(|(i, j)| site.at(i, j)).collect();
    for d in 1..=2 {
        for i in [1, 2] {
            let p = site.at(i, 0);
            let p = BlockPos::new(p.x + site.front.0 * d, p.y, p.z + site.front.1 * d);
            v.extend([p, p + UP]);
        }
    }
    v
}

/// Whether a portal site can be cleared safely: no liquid in or next to any of its cells,
/// nothing unbreakable, a floor under the frame and the standing rows. Returns how many
/// blocks are to dig.
fn site_cost(bot: &Client, site: &crate::portal::Site) -> Option<usize> {
    let cells = site_cells(site);
    let mut cost = 0;
    for &c in &cells {
        let st = block_at(bot, c);
        if fluid(st) != FluidKind::Empty || hazard(st) || liquid_next_to(bot, c, false).is_some() {
            return None;
        }
        if !open(st) {
            if unbreakable(bot, st) {
                return None;
            }
            cost += 1;
        }
        // the lowest cells stand on something
        if !cells.contains(&(c + DOWN)) && !solid(block_at(bot, c + DOWN)) {
            return None;
        }
        // gravel over the room would fall in
        if !cells.contains(&(c + UP)) && is_falling(block_at(bot, c + UP)) {
            return None;
        }
    }
    Some(cost)
}

/// Makes room for the nether portal next to where we stand: the frame's 4×5 and the spot
/// in front, dug block by block with the same checks as the tunnel, so the lead's
/// `portal::build` finds a site with nothing left to clear. Underground at diamond level:
/// going back up would cost the whole staircase again.
pub async fn portal_room(bot: &Client) -> Result<crate::portal::Site> {
    use crate::portal::Site;
    let f = feet(bot);
    let mut best: Option<(usize, Site)> = None;
    // the bot's standing spot (`Site::stand`) at our feet or a block around
    for (sx, sz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
        let stand = BlockPos::new(f.x + sx, f.y, f.z + sz);
        if !open(block_at(bot, stand)) {
            continue;
        }
        for (along, front) in [((1, 0), (0, 1)), ((1, 0), (0, -1)), ((0, 1), (1, 0)), ((0, 1), (-1, 0))] {
            // stand = at(1, 0) + 2·front, at(1, 0) = origin + along
            let origin = BlockPos::new(stand.x - front.0 * 2 - along.0, stand.y, stand.z - front.1 * 2 - along.1);
            let site = Site { origin, along, front };
            if let Some(cost) = site_cost(bot, &site)
                && best.as_ref().is_none_or(|(c, _)| cost < *c)
            {
                best = Some((cost, site));
            }
        }
    }
    let Some((cost, site)) = best else { bail!("no safe room for a portal at {f}") };
    info!("portal room at {:?} along {:?}, {cost} blocks to dig", site.origin, site.along);
    walk_to(bot, site.stand()).await?;
    let cells = site_cells(&site);
    let mut stuck = 0;
    loop {
        let mut left: Vec<BlockPos> = cells.iter().copied().filter(|&c| !open(block_at(bot, c))).collect();
        if left.is_empty() {
            break;
        }
        // top first, so nothing is dug out from under another block of the room
        left.sort_by_key(|c| (-c.y, (c.center().distance_squared_to(eye(bot)) * 100.0) as i64));
        let mut dug = false;
        for c in left {
            if click::sight(bot, c).is_none() {
                continue;
            }
            dig(bot, c, false).await?;
            dug = true;
            break;
        }
        if dug {
            stuck = 0;
            continue;
        }
        // nothing in sight from here: stand in an opened cell of the room nearer the rest
        stuck += 1;
        if stuck > 3 {
            bail!("portal room: blocks left out of sight");
        }
        let target = cells
            .iter()
            .copied()
            .filter(|&c| open(block_at(bot, c)) && open(block_at(bot, c + UP)) && solid(block_at(bot, c + DOWN)) && c != feet(bot))
            .min_by_key(|c| (c.y, (c.center().distance_squared_to(site.at(1, 2).center()) * 100.0) as i64));
        match target {
            Some(t) => walk_to(bot, t).await?,
            None => bail!("portal room: nowhere to stand"),
        }
    }
    walk_to(bot, site.stand()).await?;
    seal_around(bot).await?;
    Ok(site)
}
