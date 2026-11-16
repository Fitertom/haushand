//! Putting a block down by the real shapes of the blocks around it.
//!
//! Blocks are not all cubes: a slab, a fence, bamboo (its shape shifted at random per
//! position), short grass a click replaces. So the spot, the block clicked, the point
//! clicked and the check that the new block will not be inside us are all worked out from
//! the shapes the client has, never from a unit cube.

use std::time::Duration;

use azalea::{
    BlockPos, Client, Vec3,
    block::BlockState,
    core::{aabb::Aabb, direction::Direction},
    pathfinder::goals::RadiusGoal,
    physics::collision::BlockWithShape,
    registry::builtin::BlockKind,
};
use eyre::{Result, bail};
use tracing::{debug, info};

use crate::{
    inv::{self, Items},
    click,
    world::{block_at, feet, goto_timeout, me, world_lock},
};

const DIRECTIONS: [Direction; 6] =
    [Direction::Down, Direction::Up, Direction::North, Direction::South, Direction::West, Direction::East];

fn normal(d: Direction) -> BlockPos {
    let n = d.normal();
    BlockPos::new(n.x, n.y, n.z)
}

/// A click on such a block puts the new one in its place (air, water, grass, flowers, a
/// snow layer, ...): vanilla's `#replaceable` tag.
pub fn replaceable(state: BlockState) -> bool {
    state.is_air() || azalea::registry::tags::blocks::REPLACEABLE.contains(&BlockKind::from(state))
}

/// The outline boxes of the block at `pos`, in world coordinates. Empty: nothing to click.
fn outline_boxes(state: BlockState, pos: BlockPos) -> Vec<Aabb> {
    let shape = state.outline_shape(pos);
    let off = Vec3::new(pos.x as f64, pos.y as f64, pos.z as f64);
    shape.to_aabbs().into_iter().map(|b| b.move_relative(off)).collect()
}

/// One way to put a block at `target`: click `against` on `face` at `point`.
#[derive(Clone, Copy, Debug)]
pub struct Click {
    pub target: BlockPos,
    pub against: BlockPos,
    pub face: Direction,
    pub point: Vec3,
}

/// The ways to put a full block at `target`, nearest click first; none if the spot is taken
/// or the block would be inside us.
pub fn clicks_for(bot: &Client, target: BlockPos) -> Vec<Click> {
    let Some(state) = world_lock(bot).read().get_block_state(target) else {
        return Vec::new(); // not loaded: nothing known to click
    };
    if !replaceable(state) || !click::height_ok(bot, target.y) {
        return Vec::new();
    }
    // the new block is taken as a full cube: tables, furnaces, stone, dirt are
    let cube = Aabb {
        min: Vec3::new(target.x as f64, target.y as f64, target.z as f64),
        max: Vec3::new(target.x as f64 + 1.0, target.y as f64 + 1.0, target.z as f64 + 1.0),
    };
    // the server refuses a block where an entity stands: us, a mob, another player
    if click::entity_in(bot, &cube) {
        return Vec::new();
    }
    let eye = click::eye(bot);
    let mut out = Vec::new();

    // A replaceable block with an outline (grass) is itself clicked: the new block takes
    // its place, whatever face is hit.
    if !state.is_air() && !outline_boxes(state, target).is_empty()
        && let Some((face, point)) = click::sight(bot, target)
    {
        out.push(Click { target, against: target, face, point });
    }

    for face in DIRECTIONS {
        // `against` is the neighbour whose `face` looks at the target
        let against = target - normal(face);
        let st = block_at(bot, against);
        // a replaceable one would be replaced, not built against; a table would open
        if replaceable(st) || !click::clickable(world_lock(bot).read().get_block_state(against)) {
            continue;
        }
        // only a face the eyes see, at a point on it the ray reaches unblocked
        if let Some(point) = click::visible_point(bot, against, face) {
            out.push(Click { target, against, face, point });
        }
    }
    out.sort_by(|a, b| a.point.distance_squared_to(eye).total_cmp(&b.point.distance_squared_to(eye)));
    out
}

/// Puts a block of `items` at `target`. Walks near when out of reach.
pub async fn place_at(bot: &Client, items: &Items, target: BlockPos) -> Result<()> {
    if !inv::hold(bot, items).await {
        bail!("no {items:?} to place");
    }
    for attempt in 0..3 {
        let mut clicks = clicks_for(bot, target);
        if clicks.is_empty() {
            // no face to click seen from here: closer, then look again
            goto_timeout(bot, RadiusGoal::new(target.center(), 2.5), Duration::from_secs(20)).await?;
            clicks = clicks_for(bot, target);
        }
        if clicks.is_empty() {
            bail!("no face seen to put a block at {target} (or it would be inside something)");
        }
        for c in &clicks {
            inv::hold(bot, items).await;
            if let Err(e) = click::use_item_on(bot, c.against, c.face, c.point).await {
                debug!("click {c:?} not sent: {e}");
                continue;
            }
            for _ in 0..20 {
                bot.wait_ticks(2).await;
                if !replaceable(block_at(bot, target)) {
                    info!("placed at {target} by its {:?} neighbour", c.face);
                    return Ok(());
                }
            }
        }
        debug!("place attempt {attempt} at {target} failed");
    }
    bail!("could not put a block at {target}")
}

/// Puts a block of `items` down near us, on something it can stand on, and returns where.
pub async fn place_near(bot: &Client, items: &Items) -> Result<BlockPos> {
    let f = feet(bot);
    let mut spots = Vec::new();
    for dx in -2i32..=2 {
        for dz in -2i32..=2 {
            for dy in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let p = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                // on solid ground, so the block stands where we can walk up to it
                let under = block_at(bot, p.down(1));
                if replaceable(under) || under.is_collision_shape_empty() {
                    continue;
                }
                if !clicks_for(bot, p).is_empty() {
                    spots.push(p);
                }
            }
        }
    }
    let me = me(bot);
    spots.sort_by(|a, b| a.center().distance_squared_to(me).total_cmp(&b.center().distance_squared_to(me)));
    for p in spots.into_iter().take(6) {
        if place_at(bot, items, p).await.is_ok() {
            return Ok(p);
        }
    }
    // Down a pit dug for stone every spot around is wall: dig a niche at foot level, on
    // solid ground, and put it there.
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let p = BlockPos::new(f.x + dx, f.y, f.z + dz);
        let under = block_at(bot, p.down(1));
        if block_at(bot, p).is_air() || replaceable(under) || under.is_collision_shape_empty() {
            continue;
        }
        if let Err(e) = crate::world::mine_at(bot, p).await {
            info!("niche at {p}: could not dig it: {e}");
            continue;
        }
        if !block_at(bot, p).is_air() {
            info!("niche at {p}: still {:?} after digging", BlockKind::from(block_at(bot, p)));
            continue;
        }
        match place_at(bot, items, p).await {
            Ok(()) => return Ok(p),
            Err(e) => info!("niche at {p}: {e}"),
        }
    }
    bail!("no spot near {f} to put {items:?}")
}

#[allow(dead_code)]
pub fn describe(bot: &Client, pos: BlockPos) -> String {
    let st = block_at(bot, pos);
    format!("{:?} replaceable={} boxes={}", BlockKind::from(st), replaceable(st), outline_boxes(st, pos).len())
}
