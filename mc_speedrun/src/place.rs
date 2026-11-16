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
    core::{aabb::Aabb, direction::Direction, hit_result::HitResult},
    entity::dimensions::EntityDimensions,
    pathfinder::goals::RadiusGoal,
    physics::collision::BlockWithShape,
    registry::builtin::BlockKind,
};
use eyre::{Result, bail};
use tracing::{debug, info};

use crate::{
    inv::{self, Items},
    world::{block_at, feet, goto_timeout, me},
};

/// How far from the eyes a block may be clicked: the player's `block_interaction_range`.
const REACH: f64 = 4.5;

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

/// Our hitbox where we stand now.
fn my_box(bot: &Client) -> Aabb {
    // the position first: a component guard held while asking for another one deadlocks
    // with azalea's writer
    let pos = me(bot);
    let dims = bot.component::<EntityDimensions>().ok().map(|d| d.clone());
    match dims {
        Some(d) => d.make_bounding_box(pos),
        // a standing player: 0.6 wide, 1.8 high
        None => Aabb { min: pos + Vec3::new(-0.3, 0.0, -0.3), max: pos + Vec3::new(0.3, 1.8, 0.3) },
    }
}

fn eye(bot: &Client) -> Vec3 {
    let h = bot.component::<EntityDimensions>().map(|d| d.eye_height as f64).unwrap_or(1.62);
    let pos = me(bot);
    pos + Vec3::new(0.0, h, 0.0)
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
    let state = block_at(bot, target);
    if !replaceable(state) {
        return Vec::new();
    }
    // the new block is taken as a full cube: tables, furnaces, stone, dirt are
    let cube = Aabb {
        min: Vec3::new(target.x as f64, target.y as f64, target.z as f64),
        max: Vec3::new(target.x as f64 + 1.0, target.y as f64 + 1.0, target.z as f64 + 1.0),
    };
    if cube.intersects_aabb(&my_box(bot)) {
        return Vec::new();
    }
    let eye = eye(bot);
    let mut out = Vec::new();

    // A replaceable block with an outline (grass) is itself clicked: the new block takes
    // its place, whatever face is hit.
    let own = outline_boxes(state, target);
    if !state.is_air() && !own.is_empty() {
        let b = own[0];
        let point = Vec3::new((b.min.x + b.max.x) / 2.0, b.max.y, (b.min.z + b.max.z) / 2.0);
        out.push(Click { target, against: target, face: Direction::Up, point });
    }

    for face in DIRECTIONS {
        // `against` is the neighbour whose `face` looks at the target
        let against = target - normal(face);
        let st = block_at(bot, against);
        if replaceable(st) {
            continue; // a click on it would replace it, not put a block beside it
        }
        let boxes = outline_boxes(st, against);
        if boxes.is_empty() {
            continue;
        }
        // the box reaching furthest towards the target, and the middle of its face there
        let n = face.normal();
        let reach_of = |b: &Aabb| b.max.x * n.x as f64 + b.max.y * n.y as f64 + b.max.z * n.z as f64
            - (b.min.x * (n.x.min(0)) as f64 + b.min.y * (n.y.min(0)) as f64 + b.min.z * (n.z.min(0)) as f64);
        let b = boxes.iter().copied().max_by(|a, c| reach_of(a).total_cmp(&reach_of(c))).unwrap();
        let mid = Vec3::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0, (b.min.z + b.max.z) / 2.0);
        let point = Vec3::new(
            match n.x { 1 => b.max.x, -1 => b.min.x, _ => mid.x },
            match n.y { 1 => b.max.y, -1 => b.min.y, _ => mid.y },
            match n.z { 1 => b.max.z, -1 => b.min.z, _ => mid.z },
        );
        // a face turned away from our eyes cannot be clicked
        let to_eye = eye - point;
        if to_eye.x * n.x as f64 + to_eye.y * n.y as f64 + to_eye.z * n.z as f64 <= 0.0 {
            continue;
        }
        out.push(Click { target, against, face, point });
    }
    out.sort_by(|a, b| a.point.distance_squared_to(eye).total_cmp(&b.point.distance_squared_to(eye)));
    out
}

/// Looks at the click's point and checks that the look really lands on that block and face
/// (nothing in between). The hit result is the client's own ray against the outline shapes.
async fn aim(bot: &Client, c: &Click) -> bool {
    bot.look_at(c.point);
    bot.wait_ticks(2).await;
    match bot.hit_result() {
        Ok(HitResult::Block(h)) => h.block_pos == c.against && (c.against == c.target || h.direction == c.face),
        _ => false,
    }
}

/// Puts a block of `items` at `target`. Walks near when out of reach.
pub async fn place_at(bot: &Client, items: &Items, target: BlockPos) -> Result<()> {
    if !inv::hold(bot, items).await {
        bail!("no {items:?} to place");
    }
    for attempt in 0..3 {
        let mut clicks = clicks_for(bot, target);
        if clicks.is_empty() {
            bail!("nothing to click to put a block at {target} (or it would be inside us)");
        }
        if clicks[0].point.distance_to(eye(bot)) > REACH {
            goto_timeout(bot, RadiusGoal::new(target.center(), 3.0), Duration::from_secs(20)).await?;
            clicks = clicks_for(bot, target);
        }
        for c in clicks.iter().filter(|c| c.point.distance_to(eye(bot)) <= REACH) {
            if !aim(bot, c).await {
                debug!("click {c:?} is not in view");
                continue;
            }
            inv::hold(bot, items).await;
            bot.block_interact(c.against);
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
    bail!("no spot near {f} to put {items:?}")
}

#[allow(dead_code)]
pub fn describe(bot: &Client, pos: BlockPos) -> String {
    let st = block_at(bot, pos);
    format!("{:?} replaceable={} boxes={}", BlockKind::from(st), replaceable(st), outline_boxes(st, pos).len())
}
