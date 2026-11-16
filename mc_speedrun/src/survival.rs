//! Staying alive: eating, getting food, getting out of lava and fire, coming back after a
//! death. `tend` is the one entry point for the run's loops: it handles whatever needs it
//! now (a mob, hunger, a hazard) and returns at once when nothing does.

use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Duration,
};

use azalea::{
    BlockPos, Client, Vec3, WalkDirection,
    entity::{Physics, metadata},
    protocol::packets::game::{
        ClientboundGamePacket, ServerboundClientCommand, ServerboundUseItem, s_client_command, s_interact::InteractionHand,
    },
    registry::builtin::{BlockKind, EntityKind, ItemKind},
};
use eyre::Result;
use tracing::{info, warn};

use crate::{
    combat,
    inv::{self, Items},
    world,
};

// ---- food ------------------------------------------------------------------------------

/// Hunger points a food gives, and whether it is one to eat only when nothing else is left
/// (raw chicken and rotten flesh give hunger, golden apples are worth more in a fight).
fn food_value(kind: ItemKind) -> Option<(u32, bool)> {
    use ItemKind::*;
    Some(match kind {
        CookedBeef | CookedPorkchop | PumpkinPie => (8, false),
        CookedMutton | CookedSalmon | CookedChicken | GoldenCarrot | MushroomStew | RabbitStew | BeetrootSoup => (6, false),
        Bread | BakedPotato | CookedCod | CookedRabbit => (5, false),
        Apple => (4, false),
        Carrot | Beef | Porkchop | Rabbit => (3, false),
        Mutton | Cod | Salmon | Cookie | MelonSlice | SweetBerries | GlowBerries | TropicalFish => (2, false),
        Potato | Beetroot | DriedKelp => (1, false),
        RottenFlesh => (4, true),
        Chicken => (2, true),
        GoldenApple => (4, true),
        // spider eyes, pufferfish and poisonous potatoes poison; chorus fruit teleports
        _ => return None,
    })
}

fn foods(bot: &Client) -> Vec<(ItemKind, u32, bool)> {
    let slots = inv::player_slots(bot);
    let mut kinds: Vec<ItemKind> = inv::STORAGE.filter_map(|i| slots.get(i)).filter(|s| s.is_present()).map(|s| s.kind()).collect();
    kinds.sort_by_key(|k| *k as u32);
    kinds.dedup();
    kinds.into_iter().filter_map(|k| food_value(k).map(|(n, bad)| (k, n, bad))).collect()
}

/// Hunger points of all the good food we carry.
pub fn food_points(bot: &Client) -> i32 {
    foods(bot).iter().filter(|f| !f.2).map(|f| f.1 as i32 * inv::count_kind(bot, f.0)).sum()
}

pub fn food_level(bot: &Client) -> u32 {
    bot.hunger().map(|h| h.food).unwrap_or(20)
}

/// What to eat now, if anything: the food that fills the gap best without waste, eaten
/// once the gap is that big; any food when the bar is low or we are hurt and need to heal
/// (healing needs a nearly full bar).
fn what_to_eat(bot: &Client) -> Option<ItemKind> {
    let food = food_level(bot);
    if food >= 20 {
        return None;
    }
    let missing = 20 - food;
    let hp = combat::health(bot);
    let have = foods(bot);
    let good: Vec<_> = have.iter().filter(|f| !f.2).collect();
    // the biggest that fits the gap
    if let Some(f) = good.iter().filter(|f| f.1 <= missing).max_by_key(|f| f.1) {
        return Some(f.0);
    }
    let urgent = food <= 6 || (hp < 14.0 && food < 18) || hp < 8.0;
    if !urgent {
        return None;
    }
    if let Some(f) = good.iter().min_by_key(|f| f.1) {
        return Some(f.0);
    }
    // the bad ones: only to get out of starving, a golden apple only to survive
    let bad = |k| have.iter().find(|f| f.0 == k).map(|f| f.0);
    if food <= 6 {
        bad(ItemKind::RottenFlesh).or(bad(ItemKind::Chicken))
    } else if hp < 8.0 {
        bad(ItemKind::GoldenApple)
    } else {
        None
    }
}

/// Sends "use the held item" without looking anywhere: azalea's `start_use_item` clicks the
/// block or entity in the crosshair instead when there is one (a wall in a cave, a cow).
fn use_held_item(bot: &Client) {
    let seq = {
        let mut ecs = bot.ecs.write();
        ecs.get_mut::<azalea::interact::BlockStatePredictionHandler>(bot.entity).map(|mut h| h.start_predicting()).unwrap_or(0)
    };
    let (y_rot, x_rot) = bot.direction().map(|d| (d.y_rot(), d.x_rot())).unwrap_or((0.0, 0.0));
    bot.write_packet(ServerboundUseItem { hand: InteractionHand::MainHand, seq, y_rot, x_rot });
}

/// Eats one `kind` and waits until the server says it is eaten (the stack shrank or the
/// food bar rose). Eating takes 32 ticks; moving or switching items would cancel it.
pub async fn eat(bot: &Client, kind: ItemKind) -> Result<bool> {
    let before = (inv::count_kind(bot, kind), food_level(bot));
    if before.0 == 0 || !inv::hold(bot, &Items::one(kind)).await {
        return Ok(false);
    }
    combat::note_item_change(bot);
    bot.walk(WalkDirection::None);
    // the use goes with whatever the server has in our hand: a bucket there would pour
    for _ in 0..20 {
        bot.wait_ticks(1).await;
        if combat::held(bot) == kind {
            break;
        }
    }
    if combat::held(bot) != kind {
        warn!("could not hold {kind:?} to eat it");
        return Ok(false);
    }
    let hp = combat::health(bot);
    for attempt in 0..2 {
        use_held_item(bot);
        // 32 ticks of eating, then the server's answer; real time too, for a slow server
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        let mut interrupted = false;
        for _ in 0..60 {
            bot.wait_ticks(1).await;
            // the server's word: the food bar it sends (golden apples and a full bar
            // aside, eating always raises it), or the stack it shrank
            if food_level(bot) > before.1 || inv::count_kind(bot, kind) < before.0 {
                // the food bar and the stack come in separate packets: let both land
                bot.wait_ticks(2).await;
                info!("ate {kind:?}: food {} -> {}, hp {hp:.0}", before.1, food_level(bot));
                return Ok(true);
            }
            if !combat::alive(bot) || tokio::time::Instant::now() > deadline {
                interrupted = true;
                break;
            }
            // a mob at arm's length: standing still eating is taking its hits
            if combat::pick_target(bot).is_some_and(|m| m.box_distance(crate::click::eye(bot)) < 2.5) {
                info!("eating {kind:?} interrupted: a mob is on us");
                release_use_item(bot);
                return Ok(false);
            }
        }
        // the hand is still up as far as the server knows: let it down before anything else
        release_use_item(bot);
        if interrupted && !combat::alive(bot) {
            return Ok(false);
        }
        warn!("eating {kind:?} gave nothing (attempt {attempt})");
        bot.wait_ticks(2).await;
    }
    Ok(false)
}

/// Lets go of the use button: the server stops the eating (or the bow, the shield).
fn release_use_item(bot: &Client) {
    use azalea::protocol::packets::game::s_player_action::{Action, ServerboundPlayerAction};
    bot.write_packet(ServerboundPlayerAction {
        action: Action::ReleaseUseItem,
        pos: BlockPos::new(0, 0, 0),
        direction: azalea::core::direction::Direction::Down,
        seq: 0,
    });
}

/// Eats when `what_to_eat` says so. True when something was eaten.
pub async fn eat_if_needed(bot: &Client) -> Result<bool> {
    let mut ate = false;
    // a few in a row when starving; each rechecks
    for _ in 0..4 {
        let Some(kind) = what_to_eat(bot) else { break };
        if !eat(bot, kind).await? {
            break;
        }
        ate = true;
    }
    Ok(ate)
}

const FOOD_ANIMALS: &[EntityKind] = &[EntityKind::Cow, EntityKind::Pig, EntityKind::Sheep, EntityKind::Mooshroom, EntityKind::Chicken, EntityKind::Rabbit];

/// The game tick of the last look for food animals: when there are none in view, looking
/// again each `tend` would cost nothing but finding nothing would repeat in the log.
static LAST_HUNT: AtomicU64 = AtomicU64::new(0);

/// Kills a food animal in view when we are getting hungry and carry no food.
pub async fn get_food_if_needed(bot: &Client) -> Result<bool> {
    let food = food_level(bot);
    if food > 14 || food_points(bot) >= 8 {
        return Ok(false);
    }
    let now = bot.ticks_connected();
    let last = LAST_HUNT.load(Ordering::Relaxed);
    if food > 6 && last != 0 && now.saturating_sub(last) < 400 {
        return Ok(false);
    }
    LAST_HUNT.store(now.max(1), Ordering::Relaxed);
    let before = food_points(bot);
    for _ in 0..3 {
        match combat::hunt(bot, FOOD_ANIMALS, 32.0).await {
            Ok(_) => {}
            Err(_) => break, // nothing in view
        }
        if food_points(bot) >= 12 {
            break;
        }
    }
    let got = food_points(bot) - before;
    if got > 0 {
        info!("hunted: food points {before} -> {}", food_points(bot));
    }
    Ok(got > 0)
}

/// Hunts and cooks until we carry at least `points` hunger points of food; for a stage
/// that stocks up before a long trip. Cooks when a furnace and fuel are at hand.
pub async fn stock_food(bot: &Client, points: i32) -> Result<()> {
    let mut misses = 0;
    while food_points(bot) < points && misses < 6 {
        combat::defend(bot).await?;
        match combat::hunt(bot, FOOD_ANIMALS, 48.0).await {
            Ok(_) => misses = 0,
            Err(_) => {
                misses += 1;
                // none in view: walk a bit and look again
                let f = world::feet(bot);
                let (dx, dz) = [(48, 0), (0, 48), (-48, 0), (0, -48), (48, 48), (-48, -48)][misses % 6];
                info!("no animals in view, walking towards {} {}", f.x + dx, f.z + dz);
                let _ = world::goto_timeout(bot, azalea::pathfinder::goals::XZGoal { x: f.x + dx, z: f.z + dz }, Duration::from_secs(20)).await;
            }
        }
    }
    cook(bot).await.ok();
    info!("food stocked: {} points", food_points(bot));
    Ok(())
}

/// Cooks the raw meat we carry when a furnace is near and coal is in the inventory (planks
/// would cost the wood a run needs elsewhere).
pub async fn cook(bot: &Client) -> Result<()> {
    use ItemKind::*;
    let near_furnace = !world::find_blocks(bot, &world::states_of(&[BlockKind::Furnace]), 1, &[])
        .first()
        .is_none_or(|p| p.center().distance_to(world::me(bot)) > 24.0);
    if !near_furnace || inv::count(bot, &crate::craft::items!(Coal, Charcoal)) == 0 {
        return Ok(());
    }
    for (raw, cooked) in [(Beef, CookedBeef), (Porkchop, CookedPorkchop), (Mutton, CookedMutton), (Chicken, CookedChicken), (Rabbit, CookedRabbit)] {
        let n = inv::count_kind(bot, raw);
        if n > 0 {
            info!("cooking {n} {raw:?}");
            crate::obtain::obtain(bot, Items::one(cooked), inv::count_kind(bot, cooked) + n).await?;
        }
    }
    Ok(())
}

// ---- hazards ---------------------------------------------------------------------------

pub fn is_hazard_block(k: BlockKind) -> bool {
    matches!(k, BlockKind::Lava | BlockKind::Fire | BlockKind::SoulFire | BlockKind::MagmaBlock | BlockKind::Campfire | BlockKind::SoulCampfire | BlockKind::SweetBerryBush | BlockKind::Cactus | BlockKind::PowderSnow)
}

fn in_hazard(bot: &Client) -> bool {
    // Burning alone is not counted: it goes on for seconds after stepping out of the fire,
    // and running about for it does no good away from water.
    let f = world::feet(bot);
    let lava = bot.query_self::<&Physics, _>(|p| p.lava_fluid_height > 0.0).unwrap_or(false);
    lava || [f, f.up(1), f.down(1)].iter().any(|&p| is_hazard_block(world::kind_at(bot, p)))
}

fn drowning(bot: &Client) -> bool {
    let air = bot.query_self::<&metadata::AirSupply, _>(|a| a.0).unwrap_or(300);
    air < 150 && world::kind_at(bot, world::feet(bot).up(1)) == BlockKind::Water
}

/// The nearest column (within 6) where air lies right on top of the water above us: where
/// to surface.
fn air_spot(bot: &Client) -> Option<BlockPos> {
    let f = world::feet(bot);
    let me = world::me(bot);
    let mut best: Option<(f64, BlockPos)> = None;
    for dx in -6i32..=6 {
        for dz in -6i32..=6 {
            for dy in 0..=8 {
                let p = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                let k = world::kind_at(bot, p);
                if k == BlockKind::Water {
                    continue;
                }
                if world::block_at(bot, p).is_air() && world::kind_at(bot, p.down(1)) == BlockKind::Water {
                    let d = p.center().distance_to(me);
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, p));
                    }
                }
                break;
            }
        }
    }
    best.map(|(_, p)| p)
}

/// A spot within a few blocks to stand on safely: solid ground, two free blocks, no hazard
/// next to it. Water to stand in counts (it puts fire out).
fn safe_spot(bot: &Client) -> Option<BlockPos> {
    use azalea::physics::collision::BlockWithShape;
    let f = world::feet(bot);
    let me = world::me(bot);
    let mut best: Option<(f64, BlockPos)> = None;
    for dx in -5i32..=5 {
        for dz in -5i32..=5 {
            for dy in -2i32..=3 {
                let p = BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                let feet = world::block_at(bot, p);
                let head = world::block_at(bot, p.up(1));
                let ground = world::block_at(bot, p.down(1));
                let ok = (feet.is_collision_shape_empty() || BlockKind::from(feet) == BlockKind::Water)
                    && head.is_collision_shape_empty()
                    && !ground.is_collision_shape_empty()
                    && [p, p.up(1), p.down(1)].iter().all(|&q| !is_hazard_block(world::kind_at(bot, q)))
                    && [p.north(1), p.south(1), p.east(1), p.west(1)].iter().all(|&q| world::kind_at(bot, q) != BlockKind::Lava);
                if ok {
                    let d = p.center().distance_to(me) + dy.max(0) as f64;
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, p));
                    }
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Gets out of lava, fire and the like (straight to the nearest safe spot, jumping, since
/// the pathfinder will not start from inside lava), and swims up when out of air.
pub async fn escape_hazards(bot: &Client) -> Result<bool> {
    if drowning(bot) {
        // up, and towards the nearest place with air over the water: straight up may be
        // under a ledge or the ice
        let air_at = air_spot(bot);
        info!("out of air, swimming to {air_at:?}");
        for _ in 0..80 {
            bot.set_jumping(true).ok();
            if let Some(p) = air_at {
                let me = world::me(bot);
                combat::face(bot, Vec3::new(p.x as f64 + 0.5, me.y, p.z as f64 + 0.5));
                bot.walk(if Vec3::new(p.x as f64 + 0.5 - me.x, 0.0, p.z as f64 + 0.5 - me.z).length() > 0.5 { WalkDirection::Forward } else { WalkDirection::None });
            }
            bot.wait_ticks(1).await;
            if !drowning(bot) && bot.query_self::<&metadata::AirSupply, _>(|a| a.0).unwrap_or(300) > 250 {
                break;
            }
        }
        bot.set_jumping(false).ok();
        bot.walk(WalkDirection::None);
        return Ok(true);
    }
    if !in_hazard(bot) {
        return Ok(false);
    }
    crate::world::stop_path(bot).await;
    let Some(spot) = safe_spot(bot) else {
        warn!("in a hazard at {} and no safe spot near", world::feet(bot));
        return Ok(false);
    };
    info!("hazard at {}: moving to {spot}", world::feet(bot));
    for _ in 0..60 {
        let me = world::me(bot);
        let target = spot.center_bottom();
        if Vec3::new(me.x - target.x, 0.0, me.z - target.z).length() < 0.4 && !in_hazard(bot) {
            break;
        }
        combat::face(bot, target);
        bot.sprint(azalea::SprintDirection::Forward);
        bot.set_jumping(true).ok();
        bot.wait_ticks(1).await;
        if !combat::alive(bot) {
            break;
        }
    }
    bot.set_jumping(false).ok();
    bot.walk(WalkDirection::None);
    Ok(true)
}

// ---- the entry point -------------------------------------------------------------------

/// Whether `tend` has something to do now: a cheap check for long walks to poll.
pub fn urgent(bot: &Client) -> bool {
    if !combat::alive(bot) || TENDING.load(Ordering::Relaxed) > 1 {
        return false;
    }
    in_hazard(bot) || drowning(bot) || combat::pick_target(bot).is_some() || what_to_eat(bot).is_some()
}

/// How many `tend`s are running inside each other (a fight picks up drops, the walk to
/// them meets another mob...). Two deep at most.
static TENDING: AtomicUsize = AtomicUsize::new(0);

struct TendGuard;
impl Drop for TendGuard {
    fn drop(&mut self) {
        TENDING.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Handles whatever needs handling now: hazards, mobs, hunger, then food when there is
/// none. Returns at once when nothing does. Call it often from long loops; walks call it by
/// themselves (`combat::goto_guarded`). Boxed, since walks inside it call it again.
pub async fn tend(bot: &Client) -> Result<()> {
    tend_boxed(bot).await
}

fn tend_boxed(bot: &Client) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
    Box::pin(async move {
        if !combat::alive(bot) || TENDING.load(Ordering::Relaxed) >= 2 {
            return Ok(());
        }
        TENDING.fetch_add(1, Ordering::Relaxed);
        let _guard = TendGuard;
        escape_hazards(bot).await?;
        combat::defend(bot).await?;
        escape_hazards(bot).await?;
        eat_if_needed(bot).await?;
        if TENDING.load(Ordering::Relaxed) == 1 {
            get_food_if_needed(bot).await?;
            eat_if_needed(bot).await?;
        }
        Ok(())
    })
}

// ---- death -----------------------------------------------------------------------------

/// Respawn packets the server sent us: its answer to our respawn request.
static RESPAWNS: AtomicU64 = AtomicU64::new(0);

/// Watches for the server's respawn (also sent on a dimension change, which is harmless
/// here: we only compare counts around our own request).
pub fn packet(_bot: &Client, packet: &ClientboundGamePacket) {
    if let ClientboundGamePacket::Respawn(_) = packet {
        RESPAWNS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Asks the server to respawn us and waits for its answer: a respawn packet and our health
/// back above 0. The server ignores the request while our health is above 0, so it is
/// sent only while dead, and again every few seconds when nothing came back (a request
/// sent in the tick of the death can be read before the death is final).
pub async fn respawn(bot: &Client) -> bool {
    let before = RESPAWNS.load(Ordering::Relaxed);
    let mut sent_at: Option<tokio::time::Instant> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while tokio::time::Instant::now() < deadline {
        let answered = RESPAWNS.load(Ordering::Relaxed) > before;
        if answered && combat::alive(bot) {
            info!("respawned (the server answered our request)");
            return true;
        }
        if !answered && !combat::alive(bot) && sent_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            bot.write_packet(ServerboundClientCommand { action: s_client_command::Action::PerformRespawn });
            sent_at = Some(tokio::time::Instant::now());
        }
        bot.wait_ticks(1).await;
    }
    warn!("no respawn from the server within a minute (alive: {})", combat::alive(bot));
    combat::alive(bot)
}

/// Respawns after a death (our own request, azalea's auto-respawn is off) and waits for
/// the world around the new spawn.
pub async fn respawn_and_settle(bot: &Client) {
    // the death screen comes with the kill packet; health 0 may land a tick before it
    bot.wait_ticks(2).await;
    respawn(bot).await;
    // the respawn moves us; wait until the position settles in loaded chunks
    bot.wait_ticks(20).await;
    world::wait_for_world(bot).await;
}

/// Walks back for what we dropped at `died_at` when it is not far.
pub async fn fetch_drops(bot: &Client, died_at: Option<Vec3>) {
    let Some(at) = died_at else { return };
    let here = world::me(bot);
    let lava_there = (-1..=1).any(|dy| world::kind_at(bot, BlockPos::from(at).up(dy)) == BlockKind::Lava);
    info!("respawned at {here:.0}, died at {at:.0} ({:.0} blocks){}", here.distance_to(at), if lava_there { ", in lava" } else { "" });
    // items lie 5 minutes; walking more than ~100 blocks back (at night, through the mobs that killed us) is slower than starting over
    if lava_there || here.distance_to(at) > 100.0 || at.y < -60.0 {
        return;
    }
    let goal = azalea::pathfinder::goals::RadiusGoal::new(at, 2.0);
    if world::goto_timeout(bot, goal, Duration::from_secs(90)).await.is_ok() {
        world::collect_drops(bot, at, 8.0).await;
        info!("picked up what was left at {at:.0}");
    }
}
