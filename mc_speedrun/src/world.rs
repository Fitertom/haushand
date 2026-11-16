//! Acting on the world: finding blocks, walking to them, mining, picking up what drops,
//! placing a block.

use std::time::Duration;

use azalea::{
    BlockPos, Client, Vec3,
    block::{BlockState, BlockStates},
    entity::{Position, metadata},
    pathfinder::goals::{BlockPosGoal, RadiusGoal, ReachBlockPosGoal},
    ecs::query::With,
    prelude::*,
    registry::builtin::BlockKind,
};
use eyre::{Result, bail};
use tracing::{debug, info};

use crate::inv::{self, Items};

pub fn states_of(kinds: &[BlockKind]) -> BlockStates {
    BlockStates::from(&kinds.iter().copied().collect::<std::collections::HashSet<_>>())
}

/// Our position; the bot has one from its first tick on.
pub fn me(bot: &Client) -> Vec3 {
    bot.position().unwrap_or_default()
}

pub fn world_lock(bot: &Client) -> std::sync::Arc<parking_lot::RwLock<azalea::world::World>> {
    bot.world().expect("bot has no world")
}

pub fn block_at(bot: &Client, pos: BlockPos) -> BlockState {
    world_lock(bot).read().get_block_state(pos).unwrap_or_default()
}

pub fn kind_at(bot: &Client, pos: BlockPos) -> BlockKind {
    BlockKind::from(block_at(bot, pos))
}

pub fn feet(bot: &Client) -> BlockPos {
    BlockPos::from(me(bot))
}

/// Nearest loaded blocks of `states`, skipping `avoid`, up to `max` of them.
pub fn find_blocks(bot: &Client, states: &BlockStates, max: usize, avoid: &[BlockPos]) -> Vec<BlockPos> {
    // Our position first, the world's lock after: holding the world while asking the ECS
    // for it inverted azalea's lock order (the ECS, then the world) and froze the client.
    let me = me(bot);
    let mut found: Vec<BlockPos> = {
        let world = world_lock(bot);
        let world = world.read();
        world.find_blocks(me, states).filter(|p| !avoid.contains(p)).take(max * 4).collect()
    };
    // find_blocks sorts by x+y+z distance; the walk is by true distance.
    found.sort_by(|a, b| a.center().distance_squared_to(me).total_cmp(&b.center().distance_squared_to(me)));
    found.truncate(max);
    found
}

/// Walks until `pos` is within reach. Fails when the pathfinder gave up.
pub async fn reach(bot: &Client, pos: BlockPos) -> Result<()> {
    let chunks = world_lock(bot).read().chunks.clone();
    let goal = ReachBlockPosGoal::new(pos, chunks);
    let from = me(bot);
    // time by the distance: half a second a block walked, ten at least
    let secs = 10.0 + from.distance_to(pos.center()) * 0.5;
    let r = goto_timeout(bot, goal, Duration::from_secs_f64(secs)).await;
    if r.is_err() {
        info!(
            "reach {pos} failed: walked {:.1} of {:.1} blocks in {secs:.0}s, now {:.1} away",
            from.distance_to(me(bot)),
            from.distance_to(pos.center()),
            me(bot).distance_to(pos.center())
        );
    }
    r
}

/// `goto` with a deadline: a path that never ends (a goal walled off) must not hang a run.
/// Also gives up early when the bot has not moved for a while: a goal the pathfinder
/// cannot reach (a log up in a crown) left it standing for the whole deadline.
/// Stops on the way to fight or eat when needed (`combat::goto_guarded`).
pub async fn goto_timeout(bot: &Client, goal: impl azalea::pathfinder::goals::Goal + Clone + 'static, limit: Duration) -> Result<()> {
    crate::combat::goto_guarded(bot, goal, limit).await
}

/// Stops the pathfinder and waits for it to take the stop. The stop is a message read in
/// the next update: a `goto` sent before then was cancelled by it, and the bot stood
/// still for the whole of the next walk.
pub async fn stop_path(bot: &Client) {
    bot.force_stop_pathfinding();
    bot.wait_ticks(2).await;
}

/// Mines the block at `pos` with the best tool in the hotbar and picks up what it drops.
pub async fn mine_at(bot: &Client, pos: BlockPos) -> Result<()> {
    if block_at(bot, pos).is_air() {
        return Ok(());
    }
    reach(bot, pos).await?;
    if block_at(bot, pos).is_air() {
        // The path mined it on the way.
        collect_drops(bot, pos.center(), 6.0).await;
        return Ok(());
    }
    bot.look_at(pos.center());
    bot.wait_ticks(1).await;
    match tokio::time::timeout(Duration::from_secs(40), bot.mine_with_auto_tool(pos)).await {
        Ok(r) => r?,
        Err(_) => bail!("mining {pos} took too long"),
    }
    bot.wait_ticks(2).await;
    collect_drops(bot, pos.center(), 6.0).await;
    Ok(())
}

/// Dropped items within `radius` of `around`, nearest first.
///
/// One query under one lock: azalea's `nearest_entities_by` reads our position while
/// already holding the ECS lock, and froze the client the first time it was called.
pub fn drops_near(bot: &Client, around: Vec3, radius: f64) -> Vec<Vec3> {
    let world_name = bot.component::<azalea::world::WorldName>().ok().map(|w| w.clone());
    let Some(world_name) = world_name else { return Vec::new() };
    let mut ecs = bot.ecs.write();
    let mut query = ecs.query_filtered::<(&Position, &azalea::world::WorldName), With<metadata::Item>>();
    let mut found: Vec<Vec3> = query
        .iter(&ecs)
        .filter(|(_, w)| **w == world_name)
        .map(|(p, _)| **p)
        .filter(|p| p.distance_squared_to(around) < radius * radius)
        .collect();
    found.sort_by(|a, b| a.distance_squared_to(around).total_cmp(&b.distance_squared_to(around)));
    found
}

/// Walks onto every dropped item within `radius` of `around`.
pub async fn collect_drops(bot: &Client, around: Vec3, radius: f64) {
    for _ in 0..8 {
        let Some(&target) = drops_near(bot, around, radius).first() else { return };
        debug!("picking up item at {target}");
        if goto_timeout(bot, RadiusGoal::new(target, 0.8), Duration::from_secs(10)).await.is_err() {
            return;
        }
        bot.wait_ticks(4).await;
    }
}

/// How far `pos` stands above the ground under it: the first block down that is neither air,
/// a log nor leaves. A log higher than the reach from the ground is in a crown the path
/// cannot climb to (it places no blocks), and chasing it times out.
pub fn height_above_ground(bot: &Client, pos: BlockPos) -> i32 {
    for d in 1..24 {
        let below = pos.down(d);
        let state = block_at(bot, below);
        let name = format!("{:?}", BlockKind::from(state));
        if !state.is_air() && !name.ends_with("Log") && !name.ends_with("Leaves") && !name.ends_with("Wood") {
            return d - 1;
        }
    }
    24
}

/// Walks outward in a widening square until a block of `states` is loaded, and returns the
/// nearest one. A spawn in a desert or on an ocean shore has no tree in view.
pub async fn find_or_explore(bot: &Client, states: &BlockStates, avoid: &[BlockPos]) -> Result<BlockPos> {
    let start = feet(bot);
    // what stands out of reach from the ground is skipped, as if it were not there
    let reachable = |bot: &Client| -> Option<BlockPos> {
        find_blocks(bot, states, 16, avoid).into_iter().find(|&p| height_above_ground(bot, p) <= 3)
    };
    // eight legs of a widening spiral, then give up: 8 × 64 blocks is far past any biome
    let dirs = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    for leg in 0..12 {
        crate::survival::tend(bot).await?;
        if let Some(pos) = reachable(bot) {
            return Ok(pos);
        }
        let (dx, dz) = dirs[leg % 4];
        let reach = 32 * (leg as i32 / 2 + 1);
        let target = azalea::pathfinder::goals::XZGoal { x: start.x + dx * reach, z: start.z + dz * reach };
        info!("nothing in view, exploring towards {} {}", target.x, target.z);
        // a short walk at a time, looking again after each
        let _ = goto_timeout(bot, target, Duration::from_secs(25)).await;
        bot.wait_ticks(20).await;
    }
    bail!("explored and found none")
}

/// Waits until the chunk under the bot and its neighbours are loaded.
pub async fn wait_for_world(bot: &Client) {
    for _ in 0..200 {
        let f = feet(bot);
        let loaded = [(0, 0), (16, 0), (-16, 0), (0, 16), (0, -16)].iter().all(|(dx, dz)| {
            world_lock(bot).read().chunks.get(&azalea::core::position::ChunkPos::from(BlockPos::new(f.x + dx, f.y, f.z + dz))).is_some()
        });
        if loaded && f.y != 0 {
            return;
        }
        bot.wait_ticks(5).await;
    }
}

/// Mines blocks of `states` until `items` count reaches `want`, exploring when none is near.
pub async fn mine_until(bot: &Client, states: &BlockStates, items: &Items, want: i32) -> Result<()> {
    let mut failed: Vec<BlockPos> = Vec::new();
    while inv::count(bot, items) < want {
        crate::survival::tend(bot).await?;
        let pos = find_or_explore(bot, states, &failed).await?;
        info!("mining {:?} at {pos} ({}/{want})", kind_at(bot, pos), inv::count(bot, items));
        if let Err(e) = mine_at(bot, pos).await {
            info!("could not mine {pos}: {e}");
            failed.push(pos);
            // the rest of a tree is as out of reach as the log that failed: skip its column
            if format!("{:?}", kind_at(bot, pos)).ends_with("Log") {
                let near: Vec<BlockPos> = find_blocks(bot, states, 64, &failed)
                    .into_iter()
                    .filter(|p| (p.x - pos.x).abs() <= 2 && (p.z - pos.z).abs() <= 2)
                    .collect();
                failed.extend(near);
            }
            if failed.len() > 60 {
                bail!("too many unreachable blocks for {items:?}");
            }
        }
    }
    Ok(())
}

/// Stands still on the spot for a moment so the server and client agree on where we are.
pub async fn settle(bot: &Client) {
    bot.walk(azalea::WalkDirection::None);
    bot.wait_ticks(5).await;
}

pub async fn goto_block(bot: &Client, pos: BlockPos) -> Result<()> {
    goto_timeout(bot, BlockPosGoal(pos), Duration::from_secs(120)).await
}
