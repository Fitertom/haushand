//! The nether: where the bot came in, and finding a fortress.
//!
//! A fortress is the only structure built of nether bricks (bastions are blackstone), so
//! the first nether brick seen in the loaded chunks (the server sends a view distance of
//! them, ~10 chunks each way) is a fortress. Fortresses and bastions share one grid of
//! regions 27 chunks apart, so a straight walk crosses a region's worth of ground every
//! ~430 blocks; the bot walks straight, a leg at a time, looking after each.

use std::time::Duration;

use azalea::{BlockPos, Client, pathfinder::goals::XZGoal, registry::builtin::BlockKind};
use eyre::Result;
use tracing::info;

use crate::world::{self, find_blocks, states_of};

/// Blocks only a fortress has.
pub const FORTRESS: &[BlockKind] = &[BlockKind::NetherBricks, BlockKind::NetherBrickFence, BlockKind::NetherBrickStairs];

/// The nearest fortress block in the loaded chunks.
pub fn fortress_block(bot: &Client) -> Option<BlockPos> {
    find_blocks(bot, &states_of(FORTRESS), 1, &[]).first().copied()
}

/// How many of each kind of interesting block are loaded, and the nearest: for a look at a
/// world before deciding.
pub fn survey(bot: &Client) {
    for kinds in [
        &[BlockKind::NetherBricks][..],
        &[BlockKind::Spawner],
        &[BlockKind::NetherGoldOre],
        &[BlockKind::WarpedNylium],
        &[BlockKind::CrimsonNylium],
        &[BlockKind::SoulSand, BlockKind::SoulSoil],
        &[BlockKind::Blackstone, BlockKind::PolishedBlackstoneBricks, BlockKind::GildedBlackstone],
        &[BlockKind::Lava],
    ] {
        let found = find_blocks(bot, &states_of(kinds), 2000, &[]);
        let me = world::me(bot);
        info!(
            "survey {:?}: {} loaded, nearest {:?} ({:.0} away)",
            kinds[0],
            found.len(),
            found.first(),
            found.first().map_or(f64::NAN, |p| p.center().distance_to(me))
        );
    }
}

/// Walks a square spiral out from here until a fortress block is loaded; returns it. The
/// legs grow by 96 blocks every two turns: with ~160 blocks loaded each way, the strips
/// seen from neighbouring legs overlap, so nothing within the spiral is missed.
pub async fn find_fortress(bot: &Client) -> Result<BlockPos> {
    let mut at = world::feet(bot);
    let dirs = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    for leg in 0..16 {
        if let Some(p) = fortress_block(bot) {
            info!("fortress block at {p}");
            return Ok(p);
        }
        let (dx, dz) = dirs[leg % 4];
        let len = 96 * (leg as i32 / 2 + 1);
        // a long leg in short walks: a fortress seen half way ends it
        let steps = (len / 48).max(1);
        for _ in 0..steps {
            let target = XZGoal { x: at.x + dx * 48, z: at.z + dz * 48 };
            if world::goto_timeout(bot, target, Duration::from_secs(40)).await.is_err() {
                info!("walk towards {} {} failed; next leg", target.x, target.z);
                break;
            }
            at = world::feet(bot);
            bot.wait_ticks(10).await;
            if fortress_block(bot).is_some() {
                break;
            }
        }
    }
    fortress_block(bot).ok_or_else(|| eyre::eyre!("no fortress found"))
}

/// The blaze spawner of the fortress near `fortress` (the spawners within 96 blocks of its
/// bricks; a fortress has no other kind).
pub fn blaze_spawner(bot: &Client, fortress: BlockPos) -> Option<BlockPos> {
    let found = find_blocks(bot, &states_of(&[BlockKind::Spawner]), 16, &[]);
    // near any fortress brick, not only the first one seen: the fortress is long
    let bricks = find_blocks(bot, &states_of(FORTRESS), 200_000, &[]);
    found.into_iter().filter(|p| p.center().distance_to(fortress.center()) < 300.0 && bricks.iter().any(|b| b.center().distance_to(p.center()) < 12.0)).min_by(|a, b| {
        a.center().distance_to(world::me(bot)).total_cmp(&b.center().distance_to(world::me(bot)))
    })
}

/// Walks towards `pos` in legs of at most `leg` blocks, until within `radius`. A long walk
/// in the nether in one go timed out over its lava lakes; legs let each path be found.
pub async fn walk_to(bot: &Client, pos: BlockPos, radius: f64, leg: f64) -> Result<()> {
    let mut fails = 0;
    // a leg is ~`leg` blocks, so this many are far more than any walk needs
    for _ in 0..64 {
        let me = world::me(bot);
        let d = me.distance_to(pos.center());
        if d <= radius {
            return Ok(());
        }
        let last = d <= leg;
        let r = if last {
            world::goto_timeout(bot, azalea::pathfinder::goals::RadiusGoal::new(pos.center(), radius as f32), Duration::from_secs(45)).await
        } else {
            let t = me + (pos.center() - me) * (leg / d);
            world::goto_timeout(bot, world::NearXZ { x: t.x, z: t.z, r: 8.0 }, Duration::from_secs(45)).await
        };
        match r {
            // the pathfinder's own word on the last leg: it measures from the block we
            // stand on, a little off our exact position, and asking again only spun
            Ok(()) if last => return Ok(()),
            Ok(()) => {}
            Err(e) => {
                fails += 1;
                info!("walk to {pos}: leg failed ({fails}: {e}), {:.0} to go", world::me(bot).distance_to(pos.center()));
                if fails >= 4 {
                    eyre::bail!("could not walk to {pos}");
                }
            }
        }
    }
    eyre::bail!("walk to {pos} took too many legs")
}


/// Walks through the fortress until its blaze spawner is loaded: to the nearest fortress
/// brick at least 40 blocks from everywhere already stood, again and again. A fortress
/// runs a few hundred blocks; the spawner rooms can be at the far end of it.
pub async fn find_spawner(bot: &Client, fortress: BlockPos) -> Result<BlockPos> {
    let mut visited = vec![world::me(bot)];
    for _ in 0..10 {
        if let Some(s) = blaze_spawner(bot, fortress) {
            info!("blaze spawner at {s}");
            return Ok(s);
        }
        let bricks = find_blocks(bot, &states_of(FORTRESS), 200_000, &[]);
        let me = world::me(bot);
        let Some(next) = bricks
            .into_iter()
            .filter(|p| visited.iter().all(|v| v.distance_to(p.center()) > 40.0))
            .min_by(|a, b| a.center().distance_to(me).total_cmp(&b.center().distance_to(me)))
        else {
            break;
        };
        info!("no spawner in view; on through the fortress to {next}");
        let _ = walk_to(bot, next, 6.0, 40.0).await;
        visited.push(world::me(bot));
        visited.push(next.center());
    }
    blaze_spawner(bot, fortress).ok_or_else(|| eyre::eyre!("no blaze spawner found in the fortress at {fortress}"))
}
