//! Underground: a staircase down to diamond level, strip mining with ore spotting, and
//! obsidian cast from lava with two buckets.
//!
//! Digging is done by hand here, not by the pathfinder: the pathfinder places no blocks
//! (no floor over a gap, no cobblestone into lava), digs straight down, and decides
//! itself which blocks to break. A staircase dug block by block lets every block be
//! checked before it is opened: nothing liquid next to it, a floor under the next step.
//! The pathfinder only walks — through what was dug, or to an ore in a cave — with moves
//! that never dig straight down and never jump gaps.

use std::time::Duration;

use azalea::{
    BlockPos, Client, Vec3,
    block::{BlockState, fluid_state::{FluidKind, FluidState}},
    core::direction::Direction,
    entity::LookDirection,
    pathfinder::{
        PathfinderOpts,
        goals::{BlockPosGoal, Goal, RadiusGoal, ReachBlockPosGoal},
        moves,
    },
    physics::{clip::{BlockShapeType, ClipContext, FluidPickType, clip}, collision::BlockWithShape},
    prelude::*,
    registry::builtin::{BlockKind, ItemKind},
};
use eyre::{Result, bail, eyre};
use tracing::{debug, info, warn};

use crate::{
    craft::{self, items},
    inv::{self, Items},
    place,
    world::{block_at, drops_near, feet, me, world_lock},
};

/// Feet level of the strip mine. Diamonds get more common down to -64, but below -54
/// caves are filled with lava instead of air (the lava aquifer level); at -54 the tunnel's
/// floor is the top of those lava caves (lava does not flow up, and a pool under the floor
/// is where obsidian is cast from) while walls and ceiling are seldom lava. Ore in the
/// walls, floor and ceiling spans -55 … -52.
pub const DIAMOND_Y: i32 = -54;

/// How far a seen ore may be for a detour to it (blocks).
const ORE_RADIUS: i32 = 12;

const EYE: f64 = 1.62;
/// The player's block interaction range.
const REACH: f64 = 4.5;

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
    matches!(BlockKind::from(state), BlockKind::Fire | BlockKind::SoulFire | BlockKind::PowderSnow | BlockKind::SweetBerryBush | BlockKind::MagmaBlock)
}

fn unbreakable(bot: &Client, state: BlockState) -> bool {
    let k = BlockKind::from(state);
    let diamond = inv::count_kind(bot, ItemKind::DiamondPickaxe) + inv::count_kind(bot, ItemKind::NetheritePickaxe) > 0;
    matches!(k, BlockKind::Bedrock | BlockKind::Barrier | BlockKind::ReinforcedDeepslate | BlockKind::EndPortalFrame | BlockKind::Spawner)
        || (!diamond && matches!(k, BlockKind::Obsidian | BlockKind::CryingObsidian | BlockKind::AncientDebris))
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
    me(bot) + Vec3::new(0.0, EYE, 0.0)
}

/// Where a ray from our eyes towards `to` stops, as the server traces it for a bucket
/// (`fluids`: SourceOnly for an empty bucket, None for a full one) or as the crosshair
/// does for mining (None).
fn trace(bot: &Client, to: Vec3, fluids: FluidPickType) -> azalea::core::hit_result::BlockHitResult {
    let from = eye(bot);
    let dir = (to - from).normalize();
    let end = from + dir * REACH;
    let w = world_lock(bot);
    let w = w.read();
    clip(&w.chunks, ClipContext { from, to: end, block_shape_type: BlockShapeType::Outline, fluid_pick_type: fluids })
}

/// Walks to stand at `pos` through what is already open: no digging on the way.
pub async fn walk_to(bot: &Client, pos: BlockPos) -> Result<()> {
    let secs = 8.0 + me(bot).distance_to(pos.center()) * 0.6;
    goto_opts(bot, BlockPosGoal(pos), false, Duration::from_secs_f64(secs)).await?;
    if feet(bot) != pos {
        bail!("walked to {} instead of {pos}", feet(bot));
    }
    Ok(())
}

/// The pathfinder with our underground moves, optionally digging, with a deadline.
async fn goto_opts(bot: &Client, goal: impl Goal + 'static, mining: bool, limit: Duration) -> Result<()> {
    let opts = PathfinderOpts::new().allow_mining(mining).successors_fn(moves::underground_move);
    match tokio::time::timeout(limit, bot.goto_with_opts(goal, opts)).await {
        Ok(()) => Ok(()),
        Err(_) => {
            bot.force_stop_pathfinding();
            bot.walk(azalea::WalkDirection::None);
            bail!("path timed out")
        }
    }
}

/// Breaks the block at `pos` (in reach and in view) with the best tool. Refuses when a
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
        if eye(bot).distance_to(pos.center()) > REACH + 0.5 {
            bail!("{pos} out of reach");
        }
        // aim at the nearest point of the block that is in view: its middle may be hidden
        // behind the edge of the tunnel
        let Some(aim) = aim_point(bot, pos) else { bail!("{pos} not in view") };
        bot.look_at(aim);
        bot.wait_ticks(1).await;
        let falls_in = is_falling(block_at(bot, pos + UP));
        match tokio::time::timeout(Duration::from_secs(60), bot.mine_with_auto_tool(pos)).await {
            Ok(r) => r?,
            Err(_) => bail!("mining {pos} took too long"),
        }
        // wait for the server's word: the client predicts the break
        for _ in 0..10 {
            bot.wait_ticks(1).await;
            if open(block_at(bot, pos)) {
                break;
            }
        }
        if falls_in {
            // gravel turns into a falling entity a tick or two later and lands in the gap
            bot.wait_ticks(8).await;
        }
    }
    bail!("{pos} keeps filling up")
}

fn is_falling(state: BlockState) -> bool {
    matches!(BlockKind::from(state), BlockKind::Gravel | BlockKind::Sand | BlockKind::RedSand | BlockKind::SuspiciousGravel | BlockKind::SuspiciousSand)
}

/// A point of `pos`'s outline that the crosshair hits from where we stand.
fn aim_point(bot: &Client, pos: BlockPos) -> Option<Vec3> {
    let c = pos.center();
    let mut points = vec![c];
    for d in [-0.4, 0.4] {
        points.push(c + Vec3::new(d, 0.0, 0.0));
        points.push(c + Vec3::new(0.0, d, 0.0));
        points.push(c + Vec3::new(0.0, 0.0, d));
    }
    points.into_iter().find(|&p| {
        let hit = trace(bot, p, FluidPickType::None);
        !hit.miss && hit.block_pos == pos
    })
}

/// Places cobblestone (or any stone tool material) at `pos`.
async fn put_stone(bot: &Client, pos: BlockPos) -> Result<()> {
    let stone = craft::stone_material();
    if inv::count(bot, &stone) == 0 {
        bail!("no cobblestone to put at {pos}");
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
    let here = fluid(block_at(bot, f));
    if here == FluidKind::Lava {
        bail!("standing in lava at {f}");
    }
    Ok(())
}

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

/// The floor of a step: solid, or made solid with cobblestone. Lava or water there stops
/// the step.
fn check_floor(bot: &Client, pos: BlockPos) -> Result<bool, Blocked> {
    let st = block_at(bot, pos);
    if solid(st) {
        return Ok(true);
    }
    match fluid(st) {
        FluidKind::Lava => Err(Blocked::LavaBelow(pos)),
        FluidKind::Water => Err(Blocked::Way(format!("water floor at {pos}"))),
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
        dig(bot, p, false).await.map_err(|e| Blocked::Way(e.to_string()))?;
    }
    if !has_floor {
        put_stone(bot, floor).await.map_err(|e| Blocked::Way(format!("no floor: {e}")))?;
    }
    walk_to(bot, c).await.map_err(|e| Blocked::Way(e.to_string()))?;
    seal_around(bot).await.map_err(|e| Blocked::Way(e.to_string()))?;
    Ok(())
}

/// One step of a flat 1×2 tunnel. Lava under the next block (and nothing else wrong)
/// is reported as `LavaBelow` once the two blocks are dug: the bot stands at the edge of
/// that lava then, which is what casting obsidian wants. With `open_lava` false nothing
/// is dug in that case.
async fn tunnel_step(bot: &Client, d: Dir, open_lava: bool) -> Result<(), Blocked> {
    let s = feet(bot);
    let (a, b) = (s + d + UP, s + d);
    let floor = b + DOWN;
    check_open(bot, a, false)?;
    let lava_floor = matches!(check_floor(bot, floor), Err(Blocked::LavaBelow(_)));
    check_open(bot, b, lava_floor)?;
    let has_floor = match check_floor(bot, floor) {
        Err(Blocked::LavaBelow(p)) => {
            if open_lava && is_source(block_at(bot, p), FluidKind::Lava) {
                for q in [a, b] {
                    dig(bot, q, q == b).await.map_err(|e| Blocked::Way(e.to_string()))?;
                }
            }
            return Err(Blocked::LavaBelow(p));
        }
        r => r?,
    };
    for q in [a, b] {
        dig(bot, q, false).await.map_err(|e| Blocked::Way(e.to_string()))?;
    }
    if !has_floor {
        put_stone(bot, floor).await.map_err(|e| Blocked::Way(format!("no floor: {e}")))?;
    }
    walk_to(bot, b).await.map_err(|e| Blocked::Way(e.to_string()))?;
    seal_around(bot).await.map_err(|e| Blocked::Way(e.to_string()))?;
    Ok(())
}

/// The direction we face, rounded to a side.
fn facing(bot: &Client) -> Dir {
    let yaw = bot.query_self::<&LookDirection, _>(|l| l.y_rot()).unwrap_or(0.0);
    // yaw 0 looks south (+z), 90 west (-x), 180 north, 270 east
    let q = (((yaw.rem_euclid(360.0) + 45.0) / 90.0) as i32) % 4;
    [BlockPos::new(0, 0, 1), BlockPos::new(-1, 0, 0), BlockPos::new(0, 0, -1), BlockPos::new(1, 0, 0)][q as usize]
}

/// Digs a staircase down until our feet are at `target_y`.
pub async fn descend(bot: &Client, target_y: i32) -> Result<()> {
    crate::obtain::hotbar_tools(bot).await;
    let mut d = facing(bot);
    let mut fails = 0;
    let start = feet(bot);
    info!("descending from {start} to y {target_y}");
    while feet(bot).y > target_y {
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
                        if tunnel_step(bot, d, false).await.is_ok() {
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
    info!("at y {} after a staircase from {start}", feet(bot).y);
    Ok(())
}

/// An ore worth a detour, and until how many of its drop.
struct Want {
    blocks: &'static [BlockKind],
    drop: ItemKind,
    until: i32,
    /// Further than this from the tunnel it is left.
    radius: i32,
}

const WANTED: &[Want] = &[
    Want { blocks: &[BlockKind::DiamondOre, BlockKind::DeepslateDiamondOre], drop: ItemKind::Diamond, until: 64, radius: ORE_RADIUS },
    // iron for a flint and steel and a spare bucket
    Want { blocks: &[BlockKind::IronOre, BlockKind::DeepslateIronOre], drop: ItemKind::RawIron, until: 6, radius: 4 },
    // gold for bartering with piglins
    Want { blocks: &[BlockKind::GoldOre, BlockKind::DeepslateGoldOre], drop: ItemKind::RawGold, until: 12, radius: 3 },
];

/// Seen ores: ore blocks with a face open to air or liquid, as a player looking around
/// would see them (not ore hidden inside stone), nearest first.
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
                    let seen = neighbours(p).iter().any(|&n| {
                        let st = get(n);
                        st.is_air() || fluid(st) != FluidKind::Empty || !st.is_collision_shape_full()
                    });
                    // a drop falling into lava or water is lost or carried off
                    let under = fluid(get(p + DOWN));
                    if seen && under == FluidKind::Empty {
                        found.push((p, *i));
                    }
                }
            }
        }
    }
    let here = me(bot);
    found.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.center().distance_squared_to(here).total_cmp(&b.0.center().distance_squared_to(here))));
    found
}

/// Mines one seen ore: from here when it is in view, else after a walk (digging allowed,
/// never straight down) to where it is. Picks up what drops.
async fn mine_ore(bot: &Client, pos: BlockPos) -> Result<()> {
    if let Some((n, k)) = liquid_next_to(bot, pos, false) {
        bail!("{k:?} next to the ore at {n}");
    }
    if aim_point(bot, pos).is_none() || eye(bot).distance_to(pos.center()) > REACH {
        let chunks = world_lock(bot).read().chunks.clone();
        let secs = 15.0 + me(bot).distance_to(pos.center());
        goto_opts(bot, ReachBlockPosGoal::new(pos, chunks), true, Duration::from_secs_f64(secs)).await?;
        seal_around(bot).await?;
    }
    if BlockKind::from(block_at(bot, pos)) == BlockKind::Air {
        // the path dug it on the way
    } else {
        // dig whatever stands between us and the ore first (one block, the usual case after
        // the pathfinder's "head next to it" goal)
        if aim_point(bot, pos).is_none() {
            let hit = trace(bot, pos.center(), FluidPickType::None);
            if !hit.miss && hit.block_pos != pos {
                dig(bot, hit.block_pos, false).await?;
            }
        }
        dig(bot, pos, false).await?;
    }
    collect(bot, pos.center()).await;
    Ok(())
}

/// Walks onto the items dropped near `around`, without digging.
async fn collect(bot: &Client, around: Vec3) {
    for _ in 0..6 {
        bot.wait_ticks(2).await;
        let Some(&target) = drops_near(bot, around, 5.0).first() else { return };
        if target.distance_to(me(bot)) < 1.0 {
            bot.wait_ticks(6).await;
            if drops_near(bot, around, 5.0).first().is_some_and(|p| p.distance_to(me(bot)) < 1.0) {
                return; // not picked up while standing on it: inventory full or still in the air
            }
            continue;
        }
        if goto_opts(bot, RadiusGoal::new(target, 0.6), false, Duration::from_secs(10)).await.is_err() {
            return;
        }
    }
}

/// Mines every seen ore of interest near us. Returns how many it got.
async fn mine_seen(bot: &Client, failed: &mut Vec<BlockPos>, done: &dyn Fn(&Client) -> bool) -> usize {
    let mut n = 0;
    while !done(bot) {
        let Some(&(pos, _)) = seen_ores(bot, failed).first() else { break };
        let kind = BlockKind::from(block_at(bot, pos));
        info!("ore {kind:?} at {pos}, {:.1} away", pos.center().distance_to(me(bot)));
        match mine_ore(bot, pos).await {
            Ok(()) => {
                n += 1;
                info!("mined {kind:?}; diamonds {} raw iron {} raw gold {}", inv::count_kind(bot, ItemKind::Diamond), inv::count_kind(bot, ItemKind::RawIron), inv::count_kind(bot, ItemKind::RawGold));
            }
            Err(e) => {
                info!("left the ore at {pos}: {e}");
                failed.push(pos);
            }
        }
    }
    n
}

/// Gets back to the strip mine's level after a detour took us up or down.
async fn back_to_level(bot: &Client, y: i32) -> Result<()> {
    if feet(bot).y > y + 1 {
        descend(bot, y).await?;
    }
    Ok(())
}

/// Strip mines at `y` until `done`: a straight 1×2 tunnel, turning where liquid or
/// bedrock is ahead, mining every seen ore of interest on the way.
async fn strip_mine(bot: &Client, y: i32, done: &dyn Fn(&Client) -> bool, max_steps: usize) -> Result<()> {
    crate::obtain::hotbar_tools(bot).await;
    let mut failed = Vec::new();
    let mut d = facing(bot);
    let mut steps = 0;
    let mut fails = 0;
    loop {
        mine_seen(bot, &mut failed, done).await;
        if done(bot) {
            return Ok(());
        }
        back_to_level(bot, y).await?;
        if steps >= max_steps {
            bail!("strip mined {steps} steps without enough");
        }
        match tunnel_step(bot, d, false).await {
            Ok(()) => {
                steps += 1;
                fails = 0;
                if steps % 20 == 0 {
                    info!("strip mine: {steps} steps, at {}", feet(bot));
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
        if inv::free_slots(bot) == 0 {
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
        if let Ok(Some(h)) = bot.open_inventory() {
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

// ---------------------------------------------------------------- buckets and obsidian

fn buckets(bot: &Client) -> (i32, i32, i32) {
    (inv::count_kind(bot, ItemKind::Bucket), inv::count_kind(bot, ItemKind::WaterBucket), inv::count_kind(bot, ItemKind::LavaBucket))
}

/// Uses the bucket in hand towards `target` and waits until the inventory shows the
/// change. `fluids` is the server's trace: SourceOnly fills, None empties.
async fn use_bucket(bot: &Client, hold: ItemKind, target: Vec3, expect: ItemKind) -> Result<()> {
    if !inv::hold(bot, &Items::one(hold)).await {
        bail!("no {hold:?} to use");
    }
    let before = inv::count_kind(bot, expect);
    // stand still: the server traces from where it has us
    bot.walk(azalea::WalkDirection::None);
    bot.look_at(target);
    bot.wait_ticks(2).await;
    let dir = azalea::bot::direction_looking_at(eye(bot), target);
    bot.use_item_facing(dir)?;
    for _ in 0..30 {
        bot.wait_ticks(1).await;
        if inv::count_kind(bot, expect) > before {
            return Ok(());
        }
    }
    bail!("{hold:?} towards {target} did not give {expect:?}")
}

/// Fills a bucket from the water source `pos` (in reach and in view).
async fn fill_water(bot: &Client, pos: BlockPos) -> Result<()> {
    let target = pos.center() + Vec3::new(0.0, 0.3, 0.0);
    let hit = trace(bot, target, FluidPickType::SourceOnly);
    if hit.miss || hit.block_pos != pos {
        bail!("water at {pos} not in view (trace hits {})", hit.block_pos);
    }
    use_bucket(bot, ItemKind::Bucket, target, ItemKind::WaterBucket).await
}

/// Have a water bucket: fill one from the nearest water source in the open (a lake, a
/// river, the sea). Gives up when there is none loaded within `radius`.
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
                // open to the sky side: air right above, so it can be seen and reached
                .filter(|p| w.get_block_state(*p + UP).is_some_and(|s| s.is_air()))
                .take(40)
                .collect()
        };
        let Some(&pos) = candidates.iter().min_by(|a, b| a.center().distance_squared_to(here).total_cmp(&b.center().distance_squared_to(here))) else {
            bail!("no water in sight");
        };
        if pos.center().distance_to(here) > radius {
            bail!("nearest water {pos} is {:.0} away", pos.center().distance_to(here));
        }
        info!("water bucket: filling at {pos}");
        // walk next to it: the goal is a spot from which the block is in reach
        let chunks = world_lock(bot).read().chunks.clone();
        let secs = 15.0 + here.distance_to(pos.center()) * 0.6;
        let r = goto_opts(bot, ReachBlockPosGoal::new_with_distance(pos, 3.5, chunks), true, Duration::from_secs_f64(secs)).await;
        if r.is_ok() && fill_water(bot, pos).await.is_ok() {
            return Ok(());
        }
        failed.push(pos);
    }
    bail!("could not fill a water bucket")
}

/// A place to cast obsidian: we stand at `stand`; `lava` is a lava source in reach; the
/// cast hole `hole` is a floor block beside us with stone all around and under it; a
/// bucket of water emptied against `wall`'s face towards `hole + UP` lands right above
/// the hole.
#[derive(Clone, Copy, Debug)]
struct Station {
    stand: BlockPos,
    hole: BlockPos,
    wall: BlockPos,
}

/// The lava sources in reach of `stand`'s eyes and in a bucket's view.
fn lava_in_reach(bot: &Client, avoid: BlockPos) -> Vec<BlockPos> {
    let f = feet(bot);
    let mut out = Vec::new();
    for dx in -4..=4 {
        for dy in -4..=3 {
            for dz in -4..=4 {
                let p = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                if p == avoid || !is_source(block_at(bot, p), FluidKind::Lava) {
                    continue;
                }
                let target = p.center() + Vec3::new(0.0, 0.3, 0.0);
                if eye(bot).distance_to(target) > REACH - 0.2 {
                    continue;
                }
                let hit = trace(bot, target, FluidPickType::SourceOnly);
                if !hit.miss && hit.block_pos == p {
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
    open(st) || (fluid(st) == FluidKind::Empty && !unbreakable(bot, st) && liquid_next_to(bot, p, false).is_none())
}

/// Picks a cast hole beside where we stand (left, right or behind, never towards the lava
/// the bucket fills from).
fn plan_station(bot: &Client, lava: BlockPos) -> Option<Station> {
    let s = feet(bot);
    if !solid(block_at(bot, s + DOWN)) {
        return None;
    }
    for side in SIDES {
        let n = s + side; // the cell above the hole, where the water goes
        let hole = n + DOWN;
        let wall = n + side;
        if hole == lava || n == lava || neighbours(hole).contains(&lava) {
            continue;
        }
        // the hole: stone on its four sides and under it, nothing liquid near
        let around_solid = SIDES.iter().all(|&q| hole + q == s + DOWN || solid(block_at(bot, hole + q)))
            && solid(block_at(bot, hole + DOWN));
        let wall_ok = solid(block_at(bot, wall));
        let room = diggable(bot, n) && diggable(bot, n + UP) && (diggable(bot, hole) || block_at(bot, hole).is_air());
        let no_liquid_near = SIDES.iter().all(|&q| fluid(block_at(bot, n + q)) == FluidKind::Empty);
        if around_solid && wall_ok && room && no_liquid_near {
            return Some(Station { stand: s, hole, wall });
        }
    }
    None
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
async fn cast_one(bot: &Client, st: &Station, lava_avoid: BlockPos) -> Result<()> {
    let n = st.hole + UP;
    // 1. lava
    if inv::count_kind(bot, ItemKind::LavaBucket) == 0 {
        let Some(&lava) = lava_in_reach(bot, lava_avoid).first() else { bail!("no lava source in reach") };
        use_bucket(bot, ItemKind::Bucket, lava.center() + Vec3::new(0.0, 0.3, 0.0), ItemKind::LavaBucket).await?;
    }
    // 2. into the hole: aim at its bottom; a full bucket's trace ignores liquids and puts
    // the lava in the cell before the face hit
    if !open(block_at(bot, st.hole)) {
        bail!("cast hole {} not empty: {:?}", st.hole, BlockKind::from(block_at(bot, st.hole)));
    }
    let bottom = st.hole.center() + Vec3::new(0.0, -0.49, 0.0);
    let hit = trace(bot, bottom, FluidPickType::None);
    if hit.miss || hit.block_pos != st.hole + DOWN || hit.direction != Direction::Up {
        bail!("hole bottom not in view: trace hits {} {:?}", hit.block_pos, hit.direction);
    }
    use_bucket(bot, ItemKind::LavaBucket, bottom, ItemKind::Bucket).await?;
    wait_for(bot, 20, |bot| is_source(block_at(bot, st.hole), FluidKind::Lava)).await;
    // 3. water against the wall's face, into the cell over the hole
    let side = st.wall - n;
    let face = n.center() + Vec3::new(side.x as f64 * 0.49, -0.2, side.z as f64 * 0.49);
    let hit = trace(bot, face, FluidPickType::None);
    if hit.miss || hit.block_pos != st.wall || hit.direction != face_towards(n - st.wall) {
        bail!("wall face not in view: trace hits {} {:?}", hit.block_pos, hit.direction);
    }
    let r = use_bucket(bot, ItemKind::WaterBucket, face, ItemKind::Bucket).await;
    // 4. take the water back at once, whatever happened: spread, it would reach the lava
    // we fill from and turn that to obsidian too (over lava, where a drop burns)
    let water_back = async {
        for _ in 0..3 {
            if is_source(block_at(bot, n), FluidKind::Water) {
                let t = n.center() + Vec3::new(0.0, -0.1, 0.0);
                if use_bucket(bot, ItemKind::Bucket, t, ItemKind::WaterBucket).await.is_ok() {
                    return true;
                }
            } else if inv::count_kind(bot, ItemKind::WaterBucket) > 0 {
                return true;
            }
            bot.wait_ticks(1).await;
        }
        false
    };
    if !water_back.await {
        warn!("water at {n} not taken back");
    }
    r?;
    if BlockKind::from(block_at(bot, st.hole)) != BlockKind::Obsidian {
        bail!("cast gave {:?}", BlockKind::from(block_at(bot, st.hole)));
    }
    // 5. mine it once the flowing water is gone
    wait_for(bot, 80, |bot| fluid(block_at(bot, n)) == FluidKind::Empty).await;
    let before = inv::count_kind(bot, ItemKind::Obsidian);
    dig(bot, st.hole, false).await?;
    // 6. the drop lies at the bottom of the hole: step in for it, and back
    bot.wait_ticks(4).await;
    if inv::count_kind(bot, ItemKind::Obsidian) == before {
        walk_to(bot, st.hole).await?;
        wait_for(bot, 20, |bot| inv::count_kind(bot, ItemKind::Obsidian) > before).await;
        walk_to(bot, st.stand).await?;
    }
    if inv::count_kind(bot, ItemKind::Obsidian) == before {
        bail!("obsidian not picked up");
    }
    Ok(())
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

/// Standing spots at the edge of seen lava: a lava source with open air over it, and next
/// to that air a spot with a floor, nearest first.
fn lava_edges(bot: &Client, radius: i32) -> Vec<BlockPos> {
    let f = feet(bot);
    let mut out = Vec::new();
    let w = world_lock(bot);
    let w = w.read();
    let get = |p: BlockPos| w.get_block_state(p).unwrap_or_default();
    for dx in -radius..=radius {
        for dy in -6..=6 {
            for dz in -radius..=radius {
                let l = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                if !is_source(get(l), FluidKind::Lava) || !get(l + UP).is_air() {
                    continue;
                }
                for s in SIDES {
                    let stand = l + UP + s;
                    let ok = solid(get(stand + DOWN)) && open(get(stand)) && open(get(stand + UP))
                        && SIDES.iter().all(|&q| fluid(get(stand + q)) == FluidKind::Empty && fluid(get(stand + UP + q)) == FluidKind::Empty);
                    if ok {
                        out.push(stand);
                    }
                }
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
    let (empty, water, lava) = buckets(bot);
    if water == 0 {
        // underground the nearest water may be far; any seen within 48 blocks will do
        water_bucket(bot, 48.0).await.map_err(|e| eyre!("no water bucket: {e}"))?;
    }
    let (empty, water, lava) = if water == 0 { buckets(bot) } else { (empty, water, lava) };
    if water == 0 || empty + lava == 0 {
        bail!("need a water bucket and an empty one (have {empty} empty, {water} water, {lava} lava)");
    }
    crate::obtain::hotbar_tools(bot).await;
    let mut failed_edges: Vec<BlockPos> = Vec::new();
    let mut d = facing(bot);
    let mut steps = 0;
    while inv::count_kind(bot, ItemKind::Obsidian) < want {
        // a lava edge in sight: go there and cast
        let edge = lava_edges(bot, 16).into_iter().find(|e| !failed_edges.contains(e));
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
        match tunnel_step(bot, d, true).await {
            Ok(()) => steps += 1,
            Err(Blocked::LavaBelow(p)) => {
                info!("tunnel met lava under the floor at {p}");
                // the edge scan finds it now that the two blocks over it are open; if
                // it does not (flowing lava, not a source), go another way
                if lava_edges(bot, 3).is_empty() {
                    d = turn_right(d);
                }
            }
            Err(e) => {
                info!("tunnel {d:?} blocked: {e}");
                d = turn_right(d);
            }
        }
    }
    Ok(())
}

/// Casts obsidian standing at `edge` until `want` or the lava in reach runs out.
async fn cast_at(bot: &Client, edge: BlockPos, want: i32) -> Result<()> {
    let chunks = world_lock(bot).read().chunks.clone();
    let _ = chunks;
    if feet(bot) != edge {
        let secs = 20.0 + me(bot).distance_to(edge.center());
        goto_opts(bot, BlockPosGoal(edge), true, Duration::from_secs_f64(secs)).await?;
        if feet(bot) != edge {
            bail!("did not get to {edge}");
        }
    }
    seal_around(bot).await?;
    let Some(&lava) = lava_in_reach(bot, BlockPos::new(0, -1000, 0)).first() else { bail!("no lava in reach from {edge}") };
    let Some(st) = plan_station(bot, lava) else { bail!("no room for a cast hole at {edge}") };
    info!("cast station {st:?}, lava at {lava}");
    build_station(bot, &st).await?;
    let mut fails = 0;
    while inv::count_kind(bot, ItemKind::Obsidian) < want {
        match cast_one(bot, &st, st.hole).await {
            Ok(()) => {
                fails = 0;
                info!("obsidian {}/{want}", inv::count_kind(bot, ItemKind::Obsidian));
            }
            Err(e) => {
                fails += 1;
                info!("cast failed: {e}");
                if fails >= 3 {
                    return Err(e);
                }
                if feet(bot) != st.stand {
                    walk_to(bot, st.stand).await?;
                }
            }
        }
    }
    Ok(())
}
