//! A nether portal: a frame of 10 obsidian put down by the bot, lit, and walked into.
//!
//! The frame is 4 wide and 5 high. Its corners need not be obsidian, and corners of
//! cobblestone are what makes the frame buildable without scaffolding: each side column
//! stands on a bottom corner, and the top row is clicked against the top corners.
//!
//! Frame coordinates: `i` along the frame (0..4), `j` up (0..5); (0, 0) is a bottom
//! corner. The bot stands in front of the frame, two blocks out, where it reaches every
//! block of it.

use std::time::Duration;

use azalea::{
    BlockPos, Client,
    pathfinder::goals::BlockPosGoal,
    physics::collision::BlockWithShape,
    registry::builtin::{BlockKind, ItemKind},
};
use eyre::{Result, bail};
use tracing::info;

use crate::{
    craft::{self, items},
    inv::{self, Items},
    place::{place_at, replaceable},
    world::{self, block_at, kind_at},
};

/// Where a frame goes: its corner (0, 0), the direction `i` runs, and the side the bot
/// stands on.
#[derive(Clone, Copy, Debug)]
pub struct Site {
    pub origin: BlockPos,
    /// unit step along the frame: (1, 0) or (0, 1) in x/z
    pub along: (i32, i32),
    /// unit step from the frame towards where the bot stands
    pub front: (i32, i32),
}

impl Site {
    pub fn at(&self, i: i32, j: i32) -> BlockPos {
        BlockPos::new(self.origin.x + self.along.0 * i, self.origin.y + j, self.origin.z + self.along.1 * i)
    }
    /// The spot the bot builds from: in front of the frame's middle.
    pub fn stand(&self) -> BlockPos {
        let p = self.at(1, 0);
        BlockPos::new(p.x + self.front.0 * 2, p.y, p.z + self.front.1 * 2)
    }
    /// The frame's blocks in building order, each with whether it must be obsidian.
    /// Every block stands on, or is clicked against, one put before it.
    fn order(&self) -> Vec<(BlockPos, bool)> {
        let mut v = vec![(self.at(0, 0), false), (self.at(3, 0), false), (self.at(1, 0), true), (self.at(2, 0), true)];
        for j in 1..=3 {
            v.push((self.at(0, j), true));
            v.push((self.at(3, j), true));
        }
        v.extend([(self.at(0, 4), false), (self.at(3, 4), false), (self.at(1, 4), true), (self.at(2, 4), true)]);
        v
    }
    fn inside(&self) -> Vec<BlockPos> {
        (1..=2).flat_map(|i| (1..=3).map(move |j| (i, j))).map(|(i, j)| self.at(i, j)).collect()
    }
}

fn solid(bot: &Client, p: BlockPos) -> bool {
    let s = block_at(bot, p);
    !replaceable(s) && !s.is_collision_shape_empty() && !is_fluid(BlockKind::from(s))
}

fn is_fluid(k: BlockKind) -> bool {
    matches!(k, BlockKind::Water | BlockKind::Lava)
}

/// How many blocks must be mined before a frame fits at `site`; None when it cannot stand
/// there (no ground under it or the bot, fluid in the way).
fn clearing_cost(bot: &Client, site: &Site) -> Option<usize> {
    let mut cost = 0;
    for i in 0..4 {
        if !solid(bot, site.at(i, 0).down(1)) {
            return None;
        }
        for j in 0..5 {
            let k = kind_at(bot, site.at(i, j));
            if is_fluid(k) {
                return None;
            }
            if !replaceable(block_at(bot, site.at(i, j))) {
                cost += 1;
            }
        }
    }
    // the bot's spot and the walk between: ground, and two free blocks to stand in
    for d in 1..=2 {
        for i in [1, 2] {
            let p = site.at(i, 0);
            let p = BlockPos::new(p.x + site.front.0 * d, p.y, p.z + site.front.1 * d);
            if !solid(bot, p.down(1)) {
                return None;
            }
            for up in 0..2 {
                let s = block_at(bot, p.up(up));
                if is_fluid(BlockKind::from(s)) {
                    return None;
                }
                if !s.is_collision_shape_empty() {
                    cost += 1;
                }
            }
        }
    }
    Some(cost)
}

/// The best place near the bot for a frame: least to dig, then nearest.
pub fn find_site(bot: &Client) -> Option<Site> {
    let f = world::feet(bot);
    let me = world::me(bot);
    let mut best: Option<(f64, Site)> = None;
    for dy in -2..=2 {
        for dx in -6..=6 {
            for dz in -6..=6 {
                let origin = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                for (along, front) in [((1, 0), (0, 1)), ((1, 0), (0, -1)), ((0, 1), (1, 0)), ((0, 1), (-1, 0))] {
                    let site = Site { origin, along, front };
                    let Some(cost) = clearing_cost(bot, &site) else { continue };
                    // a block dug costs about as much as a few blocks walked
                    let score = cost as f64 * 3.0 + site.stand().center().distance_to(me) + dy.abs() as f64 * 2.0;
                    if best.as_ref().is_none_or(|(s, _)| score < *s) {
                        best = Some((score, site));
                    }
                }
            }
        }
    }
    best.map(|(_, s)| s)
}

/// Blocks a frame can have in its corners: anything full we are likely to carry.
fn corner_items() -> Items {
    let mut v = craft::stone_material();
    v.0.extend([ItemKind::Dirt, ItemKind::Netherrack, ItemKind::Obsidian]);
    v
}

/// Builds a frame near the bot and returns its site. Needs 10 obsidian and 4 corner blocks.
pub async fn build(bot: &Client) -> Result<Site> {
    if inv::count_kind(bot, ItemKind::Obsidian) < 10 {
        bail!("a portal needs 10 obsidian, have {}", inv::count_kind(bot, ItemKind::Obsidian));
    }
    let Some(site) = find_site(bot) else { bail!("no place for a portal near {}", world::feet(bot)) };
    info!("portal at {:?} along {:?}, standing at {}", site.origin, site.along, site.stand());

    // clear the frame, its inside and the spot in front
    let mut to_clear: Vec<BlockPos> = (0..4).flat_map(|i| (0..5).map(move |j| (i, j))).map(|(i, j)| site.at(i, j)).collect();
    for d in 1..=2 {
        for i in [1, 2] {
            let p = site.at(i, 0);
            let p = BlockPos::new(p.x + site.front.0 * d, p.y, p.z + site.front.1 * d);
            to_clear.extend([p, p.up(1)]);
        }
    }
    // top first: a block mined under another lets nothing fall but gravel and sand, and
    // those come down into a spot about to be cleared anyway
    to_clear.sort_by_key(|p| -p.y);
    for p in to_clear {
        if !replaceable(block_at(bot, p)) {
            world::mine_at(bot, p).await?;
        }
    }

    world::goto_timeout(bot, BlockPosGoal(site.stand()), Duration::from_secs(30)).await?;
    for (pos, obsidian) in site.order() {
        if solid(bot, pos) && (!obsidian || kind_at(bot, pos) == BlockKind::Obsidian) {
            continue;
        }
        let what = if obsidian {
            items!(Obsidian)
        } else {
            // a corner takes any block; obsidian only when there is more than the sides need
            let sides_left = site.order().iter().filter(|(p, o)| *o && kind_at(bot, *p) != BlockKind::Obsidian).count() as i32;
            let mut c = Items(corner_items().0.into_iter().filter(|&k| k != ItemKind::Obsidian).collect());
            if inv::count(bot, &c) == 0 && inv::count_kind(bot, ItemKind::Obsidian) > sides_left {
                c = items!(Obsidian);
            }
            if inv::count(bot, &c) == 0 {
                bail!("no block for the frame's corner at {pos}");
            }
            c
        };
        place_at(bot, &what, pos).await?;
        // the walk back after a far click
        if world::me(bot).distance_to(site.stand().center()) > 1.5 {
            let _ = world::goto_timeout(bot, BlockPosGoal(site.stand()), Duration::from_secs(15)).await;
        }
    }
    for (pos, obsidian) in site.order() {
        if obsidian && kind_at(bot, pos) != BlockKind::Obsidian {
            bail!("the frame at {pos} is {:?}, not obsidian", kind_at(bot, pos));
        }
    }
    Ok(site)
}

/// Lights the frame with flint and steel (clicking the top of a bottom obsidian).
pub async fn light(bot: &Client, site: &Site) -> Result<()> {
    if inside_lit(bot, site) {
        return Ok(());
    }
    if inv::count_kind(bot, ItemKind::FlintAndSteel) == 0 {
        bail!("no flint and steel");
    }
    // `place_at` clicks a neighbour of the target with the held item: on the bottom
    // obsidian's top face, flint and steel sets the fire that becomes the portal
    let r = place_at(bot, &items!(FlintAndSteel), site.at(1, 1)).await;
    for _ in 0..20 {
        if inside_lit(bot, site) {
            info!("portal lit");
            return Ok(());
        }
        bot.wait_ticks(2).await;
    }
    bail!("portal did not light ({r:?}); inside is {:?}", kind_at(bot, site.at(1, 1)))
}

fn inside_lit(bot: &Client, site: &Site) -> bool {
    site.inside().iter().all(|&p| kind_at(bot, p) == BlockKind::NetherPortal)
}

/// The name of the dimension the bot is in.
pub fn dimension(bot: &Client) -> String {
    bot.world_name().map(|w| w.to_string()).unwrap_or_default()
}

/// Walks into the portal and waits until the bot is in another dimension.
pub async fn enter(bot: &Client, site: &Site) -> Result<()> {
    let from = dimension(bot);
    let inside = site.at(1, 1);
    // The pathfinder stops short of portal blocks (it ended 0.3 off the frame's plane),
    // so it only brings us in front; the steps in are ours: face the middle of the
    // opening and walk until our feet are in a portal block.
    let _ = world::goto_timeout(bot, BlockPosGoal(site.stand()), Duration::from_secs(15)).await;
    let middle = {
        let a = site.at(1, 1).center();
        let b = site.at(2, 1).center();
        azalea::Vec3::new((a.x + b.x) / 2.0, a.y, (a.z + b.z) / 2.0)
    };
    for _ in 0..60 {
        if kind_at(bot, world::feet(bot)) == BlockKind::NetherPortal {
            break;
        }
        let eye_level = azalea::Vec3::new(middle.x, crate::click::eye(bot).y, middle.z);
        crate::click::look(bot, eye_level).await;
        bot.walk(azalea::WalkDirection::Forward);
        // the opening starts on top of the bottom row: a step up
        if world::feet(bot).y < inside.y {
            bot.jump();
        }
    }
    bot.walk(azalea::WalkDirection::None);
    if kind_at(bot, world::feet(bot)) != BlockKind::NetherPortal {
        bail!("could not step into the portal at {inside} (at {:?})", world::me(bot));
    }
    // 4 s of game time in a portal in survival, then the new dimension loads
    for _ in 0..200 {
        bot.wait_ticks(2).await;
        if dimension(bot) != from && !dimension(bot).is_empty() {
            info!("now in {}", dimension(bot));
            world::wait_for_world(bot).await;
            return Ok(());
        }
    }
    bail!("still in {from} after walking into the portal")
}
