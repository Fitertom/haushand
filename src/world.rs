//! Acting on the world: finding blocks, walking to them, mining, picking up what drops,
//! placing a block.

use std::time::Duration;

use azalea::{
    BlockPos, Client, Vec3,
    block::{BlockState, BlockStates},
    core::direction::Direction,
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
    let world = world_lock(bot);
    let world = world.read();
    let mut found: Vec<BlockPos> =
        world.find_blocks(me(bot), states).filter(|p| !avoid.contains(p)).take(max * 4).collect();
    let me = me(bot);
    // find_blocks sorts by x+y+z distance; the walk is by true distance.
    found.sort_by(|a, b| a.center().distance_squared_to(me).total_cmp(&b.center().distance_squared_to(me)));
    found.truncate(max);
    found
}

/// Walks until `pos` is within reach. Fails when the pathfinder gave up.
pub async fn reach(bot: &Client, pos: BlockPos) -> Result<()> {
    let chunks = world_lock(bot).read().chunks.clone();
    let goal = ReachBlockPosGoal::new(pos, chunks);
    goto_timeout(bot, goal, Duration::from_secs(90)).await
}

/// `goto` with a deadline: a path that never ends (a goal walled off) must not hang a run.
pub async fn goto_timeout(bot: &Client, goal: impl azalea::pathfinder::goals::Goal + 'static, limit: Duration) -> Result<()> {
    match tokio::time::timeout(limit, bot.goto(goal)).await {
        Ok(()) => Ok(()),
        Err(_) => {
            bot.force_stop_pathfinding();
            bail!("path timed out")
        }
    }
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

/// Walks onto every dropped item within `radius` of `around`.
pub async fn collect_drops(bot: &Client, around: Vec3, radius: f64) {
    for _ in 0..8 {
        let drops: Vec<Vec3> = bot
            .nearest_entities_by::<&Position, With<metadata::Item>>(|_: &Position| true)
            .unwrap_or_default()
            .iter()
            .filter_map(|e| e.position().ok())
            .filter(|p| p.distance_squared_to(around) < radius * radius)
            .collect();
        let Some(&target) = drops.first() else { return };
        debug!("picking up item at {target}");
        if goto_timeout(bot, RadiusGoal::new(target, 0.8), Duration::from_secs(10)).await.is_err() {
            return;
        }
        bot.wait_ticks(4).await;
    }
}

/// Mines blocks of `states` until `items` count reaches `want`. Fails when none is in view.
pub async fn mine_until(bot: &Client, states: &BlockStates, items: &Items, want: i32) -> Result<()> {
    let mut failed: Vec<BlockPos> = Vec::new();
    while inv::count(bot, items) < want {
        let Some(&pos) = find_blocks(bot, states, 1, &failed).first() else {
            bail!("no block to mine for {items:?} in loaded chunks");
        };
        info!("mining {:?} at {pos} ({}/{want})", kind_at(bot, pos), inv::count(bot, items));
        if let Err(e) = mine_at(bot, pos).await {
            info!("could not mine {pos}: {e}");
            failed.push(pos);
            if failed.len() > 20 {
                bail!("too many unreachable blocks for {items:?}");
            }
        }
    }
    Ok(())
}

/// Places the held block onto the `face` of `against`. The block appears at
/// `against.offset(face)`.
pub async fn place_against(bot: &Client, against: BlockPos, face: Direction) -> Result<()> {
    let n = face.normal();
    let target = against.center() + Vec3::new(n.x as f64 * 0.5, n.y as f64 * 0.5, n.z as f64 * 0.5);
    bot.look_at(target);
    bot.wait_ticks(2).await;
    bot.block_interact(against);
    bot.wait_ticks(3).await;
    let placed = against + n;
    if block_at(bot, placed).is_air() {
        bail!("placing at {placed} did nothing");
    }
    Ok(())
}

/// A free spot beside the bot with solid ground under it, to put a block (a crafting
/// table, a furnace) on.
pub fn spot_beside(bot: &Client) -> Option<BlockPos> {
    let f = feet(bot);
    let around = [f.north(1), f.south(1), f.east(1), f.west(1), f.north(1).east(1), f.south(1).west(1)];
    around.into_iter().find(|&p| {
        block_at(bot, p).is_air() && block_at(bot, p.up(1)).is_air() && !block_at(bot, p.down(1)).is_air()
    })
}

/// Puts a block of `items` down beside the bot and returns where.
pub async fn place_beside(bot: &Client, items: &Items) -> Result<BlockPos> {
    if !inv::hold(bot, items).await {
        bail!("no {items:?} to place");
    }
    let spot = match spot_beside(bot) {
        Some(s) => s,
        None => {
            // Boxed in: clear the block in front at head height and use the floor there.
            let f = feet(bot).north(1);
            mine_at(bot, f).await?;
            mine_at(bot, f.up(1)).await?;
            inv::hold(bot, items).await;
            f
        }
    };
    place_against(bot, spot.down(1), Direction::Up).await?;
    Ok(spot)
}

/// Stands still on the spot for a moment so the server and client agree on where we are.
pub async fn settle(bot: &Client) {
    bot.walk(azalea::WalkDirection::None);
    bot.wait_ticks(5).await;
}

pub async fn goto_block(bot: &Client, pos: BlockPos) -> Result<()> {
    goto_timeout(bot, BlockPosGoal(pos), Duration::from_secs(120)).await
}
