//! Fighting: seeing mobs, choosing one, holding a weapon, aiming and hitting at full charge.
//!
//! Everything here is cooperative: it runs inside the run's task, between or instead of its
//! walks (`survival::tend`, `goto_guarded`), so there is never a second task steering the
//! bot. Entities are read with one ECS query under one lock, like `world::drops_near`:
//! azalea's `nearest_entities_by` reads our position while holding the ECS lock and froze
//! the client.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use azalea::{
    Client, Vec3, WalkDirection,
    core::entity_id::MinecraftEntityId,
    ecs::{entity::Entity, query::Without},
    entity::{Dead, EntityKindComponent, LocalEntity, LookDirection, Physics, Position, dimensions::EntityDimensions, metadata},
    pathfinder::goals::{Goal, RadiusGoal},
    prelude::*,
    protocol::packets::game::{ClientboundGamePacket, ServerboundAttack, ServerboundPunch},
    registry::builtin::{EntityKind, ItemKind},
    world::WorldName,
};
use eyre::{Result, bail};
use parking_lot::Mutex;
use tracing::{debug, info};

use crate::inv::{self, Items};

/// How far the server lets a hit land, from our eyes to the target's box. Vanilla's reach
/// is 3.0; a little less so a hit sent as the target steps away still lands.
const REACH: f64 = 2.9;

/// How a mob is to be met.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// Walks up and hits: fight it when it comes near.
    Melee,
    /// Shoots from a distance: close in (strafing) and hit.
    Ranged,
    /// Explodes: hit and step back, run when it starts to swell.
    Creeper,
    /// Out of a sword's reach most of the time (phantom, ghast, blaze, vex): hit only when it
    /// comes within reach.
    Flying,
    /// Too strong for us (warden, ravager, creaking): keep away, never fight.
    Avoid,
    /// Attacks only when provoked (enderman, zombified piglin, wolf, ...): fight only what hit
    /// us, and an enderman that screams at us.
    Neutral,
    /// Drops meat.
    Food,
    Other,
}

pub fn role(kind: EntityKind) -> Role {
    use EntityKind as E;
    match kind {
        E::Zombie | E::ZombieVillager | E::Husk | E::Drowned | E::Spider | E::CaveSpider | E::Silverfish
        | E::Endermite | E::Vindicator | E::Zoglin | E::Hoglin | E::PiglinBrute | E::WitherSkeleton | E::Slime
        | E::MagmaCube | E::Giant | E::ZombieNautilus | E::CamelHusk | E::SulfurCube => Role::Melee,
        E::Skeleton | E::Stray | E::Bogged | E::Parched | E::Pillager | E::Witch | E::Evoker | E::Illusioner
        | E::Breeze | E::Guardian | E::ElderGuardian | E::Shulker => Role::Ranged,
        E::Creeper => Role::Creeper,
        E::Phantom | E::Ghast | E::Blaze | E::Vex => Role::Flying,
        E::Warden | E::Ravager | E::Creaking | E::Wither | E::EnderDragon => Role::Avoid,
        E::Enderman | E::ZombifiedPiglin | E::Piglin | E::Wolf | E::Bee | E::PolarBear | E::IronGolem | E::Llama
        | E::TraderLlama | E::Panda | E::Goat | E::Dolphin => Role::Neutral,
        E::Cow | E::Pig | E::Sheep | E::Chicken | E::Rabbit | E::Mooshroom => Role::Food,
        _ => Role::Other,
    }
}

/// A living entity as seen this tick.
#[derive(Clone, Debug)]
pub struct Mob {
    pub entity: Entity,
    pub id: MinecraftEntityId,
    pub kind: EntityKind,
    pub pos: Vec3,
    pub width: f64,
    pub height: f64,
    pub health: f32,
    pub baby: bool,
    /// An enderman screaming (angry at someone, usually us).
    pub angry: bool,
    /// A creeper counting down to its explosion.
    pub swelling: bool,
}

impl Mob {
    pub fn role(&self) -> Role {
        role(self.kind)
    }
    pub fn center(&self) -> Vec3 {
        self.pos.up(self.height / 2.0)
    }
    /// The point of its box nearest to `from`: what reach is measured to, and where to aim.
    pub fn nearest_point(&self, from: Vec3) -> Vec3 {
        let r = self.width / 2.0;
        Vec3::new(
            from.x.clamp(self.pos.x - r, self.pos.x + r),
            from.y.clamp(self.pos.y, self.pos.y + self.height),
            from.z.clamp(self.pos.z - r, self.pos.z + r),
        )
    }
    pub fn box_distance(&self, from: Vec3) -> f64 {
        self.nearest_point(from).distance_to(from)
    }
}

type MobRow<'a> = (
    Entity,
    &'a MinecraftEntityId,
    &'a EntityKindComponent,
    &'a Position,
    &'a EntityDimensions,
    &'a WorldName,
    Option<&'a metadata::Health>,
    Option<&'a Dead>,
    Option<&'a metadata::AbstractAgeableBaby>,
    Option<&'a metadata::Creepy>,
    Option<&'a metadata::SwellDir>,
    Option<&'a metadata::IsIgnited>,
);

fn to_mob(row: MobRow) -> Option<Mob> {
    let (entity, id, kind, pos, dims, _, health, dead, baby, creepy, swell, ignited) = row;
    let health = health.map(|h| h.0)?; // no health: not living (items, arrows, boats)
    if dead.is_some() || health <= 0.0 {
        return None;
    }
    Some(Mob {
        entity,
        id: *id,
        kind: **kind,
        pos: **pos,
        width: dims.width as f64,
        height: dims.height as f64,
        health,
        baby: baby.is_some_and(|b| b.0),
        angry: creepy.is_some_and(|c| c.0),
        swelling: swell.is_some_and(|s| s.0 > 0) || ignited.is_some_and(|i| i.0),
    })
}

/// Living mobs (and other players) within `radius` of us, nearest first.
pub fn mobs_near(bot: &Client, radius: f64) -> Vec<Mob> {
    // our own position and world first, the ECS lock after (lock order, see TESTING.md)
    let me = crate::world::me(bot);
    let world_name = bot.component::<WorldName>().ok().map(|w| w.clone());
    let Some(world_name) = world_name else { return Vec::new() };
    let mut ecs = bot.ecs.write();
    let mut query = ecs.query_filtered::<MobRow, Without<LocalEntity>>();
    let mut found: Vec<Mob> = query
        .iter(&ecs)
        .filter(|row| *row.5 == world_name && row.3.distance_squared_to(me) < radius * radius)
        .filter_map(to_mob)
        .collect();
    found.sort_by(|a, b| a.pos.distance_squared_to(me).total_cmp(&b.pos.distance_squared_to(me)));
    found
}

/// One entity as it is now; None once it died or went out of view.
pub fn mob(bot: &Client, entity: Entity) -> Option<Mob> {
    let mut ecs = bot.ecs.write();
    let mut query = ecs.query::<MobRow>();
    query.get(&ecs, entity).ok().and_then(to_mob)
}

// ---- who hit us ------------------------------------------------------------------------

/// Entity ids that hurt us, with the game tick it happened at.
static HURT_BY: Mutex<Vec<(i32, u64)>> = Mutex::new(Vec::new());
static LAST_HURT: AtomicU64 = AtomicU64::new(0);

/// Watches the server's damage events for hits on us, to fight back against what hit us
/// (a skeleton out of the usual range, a provoked neutral mob).
pub fn packet(bot: &Client, packet: &ClientboundGamePacket) {
    let ClientboundGamePacket::DamageEvent(p) = packet else { return };
    let Some(me) = bot.component::<MinecraftEntityId>().ok().map(|i| *i) else { return };
    if p.entity_id != me {
        return;
    }
    let now = bot.ticks_connected();
    LAST_HURT.store(now, Ordering::Relaxed);
    if let Some(by) = p.source_cause_id.0 {
        debug!("hurt by entity {}", by.0);
        let mut hurt = HURT_BY.lock();
        hurt.retain(|(id, t)| *id != by.0 && now.saturating_sub(*t) < 600);
        hurt.push((by.0, now));
    }
}

/// Whether `id` hurt us within the last `ticks`.
pub fn hurt_by(bot: &Client, id: MinecraftEntityId, ticks: u64) -> bool {
    let now = bot.ticks_connected();
    HURT_BY.lock().iter().any(|(i, t)| *i == id.0 && now.saturating_sub(*t) < ticks)
}

/// Ticks since anything last hurt us.
pub fn ticks_since_hurt(bot: &Client) -> u64 {
    bot.ticks_connected().saturating_sub(LAST_HURT.load(Ordering::Relaxed))
}

// ---- targets ---------------------------------------------------------------------------

/// Mobs we gave up on (could not reach), and until which tick to leave them be.
static GIVEN_UP: Mutex<Vec<(Entity, u64)>> = Mutex::new(Vec::new());

fn given_up(bot: &Client, e: Entity) -> bool {
    let now = bot.ticks_connected();
    let mut g = GIVEN_UP.lock();
    g.retain(|(_, until)| *until > now);
    g.iter().any(|(x, _)| *x == e)
}

fn give_up(bot: &Client, e: Entity, ticks: u64) {
    GIVEN_UP.lock().push((e, bot.ticks_connected() + ticks));
}

/// Whether `m` is something to fight now, and how urgent (lower first).
fn threat_score(bot: &Client, me: Vec3, m: &Mob) -> Option<f64> {
    let dy = (m.pos.y - me.y).abs();
    let d = m.pos.horizontal_distance_squared_to(me).sqrt();
    let hit_us = hurt_by(bot, m.id, 100);
    let engage = match m.role() {
        Role::Melee => d < 10.0 && dy < 5.0,
        Role::Ranged => d < 14.0 && dy < 6.0,
        Role::Creeper => d < 8.0 && dy < 4.0,
        Role::Flying => m.box_distance(me.up(1.62)) < REACH + 0.5,
        Role::Neutral => hit_us || (m.kind == EntityKind::Enderman && m.angry && d < 16.0),
        Role::Avoid | Role::Food | Role::Other => false,
    } || (hit_us && d < 16.0 && m.role() != Role::Avoid && m.kind != EntityKind::Player);
    if !engage || given_up(bot, m.entity) {
        return None;
    }
    let mut score = d;
    if hit_us {
        score -= 4.0;
    }
    if m.role() == Role::Creeper {
        score -= 2.0;
    }
    Some(score)
}

/// The mob to fight now, if any. Cheap: one entity query.
pub fn pick_target(bot: &Client) -> Option<Mob> {
    let me = crate::world::me(bot);
    mobs_near(bot, 20.0)
        .into_iter()
        .filter_map(|m| threat_score(bot, me, &m).map(|s| (s, m)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, m)| m)
}

// ---- weapon and cooldown ---------------------------------------------------------------

/// Weapons, best first, with their attack speed (attacks a second at full charge).
const WEAPONS: &[(ItemKind, f64)] = {
    use ItemKind::*;
    &[
        (NetheriteSword, 1.6),
        (DiamondSword, 1.6),
        (IronSword, 1.6),
        (NetheriteAxe, 1.0),
        (DiamondAxe, 1.0),
        (StoneSword, 1.6),
        (IronAxe, 0.9),
        (GoldenSword, 1.6),
        (StoneAxe, 0.8),
        (WoodenSword, 1.6),
        (GoldenAxe, 1.0),
        (WoodenAxe, 0.8),
    ]
};

/// Ticks of full charge for the held item: a sword 13 (1.6/s), a fist 5 (4/s).
fn charge_ticks(held: ItemKind) -> u64 {
    let speed = WEAPONS.iter().find(|(k, _)| *k == held).map(|(_, s)| *s).unwrap_or(match held {
        // pickaxes and shovels hit at 1.2 and 1.0; anything else as a fist
        k if format!("{k:?}").ends_with("Pickaxe") => 1.2,
        k if format!("{k:?}").ends_with("Shovel") => 1.0,
        _ => 4.0,
    });
    (20.0 / speed).ceil() as u64
}

/// The game tick of our last swing or weapon change: both restart the charge, on the
/// server too (it resets the attack ticker when the main hand item changes).
static CHARGE_START: AtomicU64 = AtomicU64::new(0);

/// Call after changing the held item: the next hit waits for a full charge.
pub fn note_item_change(bot: &Client) {
    CHARGE_START.store(bot.ticks_connected(), Ordering::Relaxed);
}

pub fn held(bot: &Client) -> ItemKind {
    let slot = bot.selected_hotbar_slot().unwrap_or(0) as usize;
    inv::player_slots(bot).get(36 + slot).map(|s| s.kind()).unwrap_or(ItemKind::Air)
}

fn charged(bot: &Client) -> bool {
    // one tick more than the charge: our tick count and the server's drift by a tick
    bot.ticks_connected().saturating_sub(CHARGE_START.load(Ordering::Relaxed)) > charge_ticks(held(bot))
}

/// Holds the best weapon we have (nothing changes if it is already in hand).
pub async fn hold_weapon(bot: &Client) {
    let Some(&(best, _)) = WEAPONS.iter().find(|(k, _)| inv::count_kind(bot, *k) > 0) else { return };
    if held(bot) == best {
        return;
    }
    if inv::hold(bot, &Items::one(best)).await {
        note_item_change(bot);
    }
}

/// Puts a shield into the offhand when there is one and the offhand is empty. A shield in
/// the offhand is not raised by itself, but `raise_shield` can then block arrows.
pub async fn equip_shield(bot: &Client) {
    let slots = inv::player_slots(bot);
    if slots[45].is_present() {
        return;
    }
    let Some(&(from, _, _)) = inv::slots_of(bot, &Items::one(ItemKind::Shield)).first() else { return };
    if let Ok(Some(h)) = bot.open_inventory() {
        // a swap click with button 40 swaps with the offhand
        h.click(azalea::inventory::operations::SwapClick { source_slot: from as u16, target_slot: 40 });
        bot.wait_ticks(2).await;
    }
}

// ---- fighting --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Killed,
    /// Out of view, or stopped being a threat (a creeper we ran from).
    Gone,
    GaveUp,
}

pub fn alive(bot: &Client) -> bool {
    bot.query_self::<&metadata::Health, _>(|h| h.0 > 0.0).unwrap_or(false)
}

pub fn health(bot: &Client) -> f32 {
    bot.query_self::<&metadata::Health, _>(|h| h.0).unwrap_or(0.0)
}

/// Our velocity, on ground, walking into a wall, in water.
fn physics(bot: &Client) -> (Vec3, bool, bool, bool) {
    bot.query_self::<&Physics, _>(|p| (p.velocity, p.on_ground(), p.horizontal_collision, p.water_fluid_height > 0.0))
        .unwrap_or((Vec3::ZERO, true, false, false))
}

/// Whether a step towards `dir` (horizontal, normalized) from our feet is safe: ground within
/// a short drop and no lava or fire there.
fn safe_step(bot: &Client, dir: Vec3) -> bool {
    let me = crate::world::me(bot);
    let ahead = azalea::BlockPos::from(me + dir * 0.9);
    for dy in [0, 1] {
        let k = crate::world::kind_at(bot, ahead.up(dy));
        if crate::survival::is_hazard_block(k) {
            return false;
        }
    }
    for d in 1..=3 {
        let st = crate::world::block_at(bot, ahead.down(d));
        let k = crate::world::kind_at(bot, ahead.down(d));
        if crate::survival::is_hazard_block(k) {
            return false;
        }
        if !st.is_air() && k != azalea::registry::builtin::BlockKind::Water {
            return true;
        }
    }
    false
}

/// Sets our yaw and pitch towards `point` without waiting: the movement packet of the next
/// tick carries them (a rotation packet of our own would make two movement packets in one
/// tick, which 26.3 kicks for). `click::look` is the same plus the wait.
pub fn aim(bot: &Client, point: Vec3) {
    let (yaw, pitch) = crate::click::angles(crate::click::eye(bot), point);
    let mut ecs = bot.ecs.write();
    if let Some(mut l) = ecs.get_mut::<LookDirection>(bot.entity) {
        *l = LookDirection::new(yaw, pitch);
    }
}

/// Turns towards `target` (yaw only, eyes level with it), for walking.
pub fn face(bot: &Client, target: Vec3) {
    let eye = crate::click::eye(bot);
    aim(bot, Vec3::new(target.x, eye.y, target.z));
}

/// Hits `target` with what we hold: our own attack packet, then the arm swing.
///
/// The server's `handleAttack` checks: the target exists in our world and inside the border,
/// it is within reach of our eyes (entity interaction range 3.0 to its box, plus a
/// tolerance), and it is not an item, an orb, an arrow or ourselves (any of those is a
/// kick for an invalid attack). It does not check the look, but we have looked at it a
/// tick before, as a player does. The damage scales with the charge: the server counts
/// ticks since the last swing (`ServerboundPunch` resets the counter, the attack itself
/// does not), so the punch goes after the attack, and we hit only at full charge.
/// Returns whether the hit was sent.
pub fn strike(bot: &Client, target: Entity) -> bool {
    let Some(m) = mob(bot, target) else { return false };
    // the same world: a mob row from the dimension we just left is not attackable
    let ours = bot.component::<WorldName>().ok().map(|w| w.clone());
    let theirs = bot.ecs.read().get::<WorldName>(target).cloned();
    if ours.is_none() || ours != theirs || m.entity == bot.entity {
        return false;
    }
    let dist = m.box_distance(crate::click::eye(bot));
    if dist > REACH || !charged(bot) || !alive(bot) {
        return false;
    }
    bot.write_packet(ServerboundAttack { entity_id: m.id });
    bot.write_packet(ServerboundPunch);
    CHARGE_START.store(bot.ticks_connected(), Ordering::Relaxed);
    // a sprinting hit knocks back and ends the sprint, on the server; walk instead
    if bot.query_self::<&metadata::Sprinting, _>(|s| s.0).unwrap_or(false) {
        bot.walk(WalkDirection::Forward);
    }
    debug!("hit {:?} (hp {:.0}) at {dist:.2}", m.kind, m.health);
    true
}

/// Fights `target` until it dies, leaves, or `limit` passes. Steps of one game tick.
pub async fn fight(bot: &Client, target: Entity, limit: Duration) -> Result<Outcome> {
    let Some(first) = mob(bot, target) else { return Ok(Outcome::Gone) };
    let role = first.role();
    info!("fight {:?} at {:.1} blocks (hp {:.0}, ours {:.0})", first.kind, first.pos.distance_to(crate::world::me(bot)), first.health, health(bot));
    hold_weapon(bot).await;
    let start = Instant::now();
    let mut last_pos = first.pos;
    let mut last_kind = first.kind;
    let mut pathing_to: Option<Vec3> = None;
    let mut path_issued = 0u64;
    let mut direct_until = 0u64;
    let mut retreat_until = 0u64;
    let mut fleeing = false;
    let mut hits = 0;
    let mut want_hit = false;
    // progress: the target's health or distance must change now and then, else it is out
    // of reach (behind a wall, up a cliff) and we leave it
    let mut best = (first.health, first.box_distance(crate::world::me(bot).up(1.62)));
    let mut last_progress = Instant::now();
    let outcome = loop {
        if !alive(bot) {
            break Outcome::Gone;
        }
        let Some(m) = mob(bot, target) else {
            let exists = bot.ecs.read().get_entity(target).is_ok();
            info!("fight: {last_kind:?} no longer seen (entity {}), last at {:.1} blocks", if exists { "still there" } else { "removed" }, last_pos.distance_to(crate::world::me(bot)));
            break if last_pos.distance_to(crate::world::me(bot)) < 8.0 { Outcome::Killed } else { Outcome::Gone };
        };
        last_kind = m.kind;
        let now = bot.ticks_connected();
        let me = crate::world::me(bot);
        let eye = me.up(1.62);
        let vel = m.pos - last_pos;
        last_pos = m.pos;
        let dist = m.box_distance(eye);
        let flat = Vec3::new(m.pos.x - me.x, 0.0, m.pos.z - me.z);
        let flat_d = flat.length().max(0.001);
        let toward = flat * (1.0 / flat_d);

        if m.health < best.0 - 0.1 || dist < best.1 - 0.5 {
            best = (best.0.min(m.health), best.1.min(dist));
            last_progress = Instant::now();
        }
        if start.elapsed() > limit || last_progress.elapsed() > Duration::from_secs(12) {
            info!("fight: giving up on {:?} ({:.1} away, hp {:.0})", m.kind, dist, m.health);
            give_up(bot, target, 1200);
            break Outcome::GaveUp;
        }
        // a creeper about to blow: run until it stops (it swells only within 7 blocks)
        if role == Role::Creeper && m.swelling && (flat_d < 7.5 || fleeing) {
            if !fleeing {
                info!("creeper swelling at {flat_d:.1}, running");
                if pathing_to.take().is_some() {
                    crate::world::stop_path(bot).await;
                }
            }
            fleeing = true;
            let away = me - toward * 4.0;
            face(bot, away);
            if safe_step(bot, toward * -1.0) {
                bot.sprint(azalea::SprintDirection::Forward);
            } else {
                bot.walk(WalkDirection::Left);
            }
            let (_, on_ground, blocked, in_water) = physics(bot);
            bot.set_jumping(in_water || (blocked && on_ground)).ok();
            bot.wait_ticks(1).await;
            continue;
        }
        if fleeing {
            // it stopped swelling: come back for the next hit (or leave it, if we must)
            fleeing = false;
            bot.walk(WalkDirection::None);
            if health(bot) < 8.0 {
                give_up(bot, target, 1200);
                break Outcome::Gone;
            }
        }

        let (_, _, _, in_water) = physics(bot);
        // the pathfinder finds no path out of water, nor to a mob standing in it: when it
        // has nothing after a second, walk (swim) straight at it for a while
        if pathing_to.is_some() && now.saturating_sub(path_issued) > 40 && !bot.is_executing_path() {
            direct_until = now + 60;
        }
        let close = flat_d < 4.5 && (m.pos.y - me.y).abs() < 2.0;
        let direct = close || in_water || now < direct_until || role == Role::Flying;
        if !direct {
            // far, or above/below: the pathfinder knows the way round
            let moved = pathing_to.is_none_or(|p| p.distance_to(m.pos) > 2.0);
            if moved && now.saturating_sub(path_issued) >= 10 {
                bot.start_goto(RadiusGoal::new(m.pos, 2.0));
                pathing_to = Some(m.pos);
                path_issued = now;
            }
        } else {
            if pathing_to.take().is_some() {
                // waited for: a goto sent before the stop is read gets cancelled by it
                crate::world::stop_path(bot).await;
            }
            // aim at the box point nearest our eyes, a tick ahead of its movement
            let p = m.nearest_point(eye) + vel;
            let aim_at = Vec3::new(p.x, p.y.clamp(m.pos.y + 0.1, m.pos.y + m.height - 0.1), p.z);
            aim(bot, aim_at);
            let walk = if now < retreat_until {
                WalkDirection::Backward
            } else if dist > REACH - 0.6 && role != Role::Flying {
                if !in_water && !safe_step(bot, toward) {
                    WalkDirection::None
                } else if role == Role::Ranged && dist > 2.0 {
                    // a skeleton aims where we walk: side to side makes it miss
                    if (now / 15) % 2 == 0 { WalkDirection::ForwardLeft } else { WalkDirection::ForwardRight }
                } else {
                    WalkDirection::Forward
                }
            } else if role == Role::Creeper && !charged(bot) {
                // wait for the charge outside its swelling range
                WalkDirection::Backward
            } else {
                WalkDirection::None
            };
            bot.walk(walk);
            let (_, on_ground, blocked, _) = physics(bot);
            let forward = matches!(walk, WalkDirection::Forward | WalkDirection::ForwardLeft | WalkDirection::ForwardRight);
            // jumping is swimming up in water: keeps our head out of it
            bot.set_jumping(in_water || (blocked && on_ground && forward)).ok();
            // the hit goes out next tick, after the movement packet with this look
            want_hit = dist <= REACH + 0.3 && charged(bot);
        }
        bot.wait_ticks(1).await;
        if std::mem::take(&mut want_hit) && strike(bot, target) {
            hits += 1;
            if role == Role::Creeper {
                // its knockback plus our steps back keep it out of its 3-block trigger
                retreat_until = bot.ticks_connected() + 10;
            }
        }
    };
    if pathing_to.is_some() {
        crate::world::stop_path(bot).await;
    }
    bot.walk(WalkDirection::None);
    bot.set_jumping(false).ok();
    info!("fight {last_kind:?}: {outcome:?} after {hits} hits, {:.1}s, our hp {:.0}", start.elapsed().as_secs_f64(), health(bot));
    Ok(outcome)
}

/// Fights whatever threatens us until nothing does (a few at most). Returns how many
/// fights there were.
pub async fn defend(bot: &Client) -> Result<usize> {
    let mut n = 0;
    while n < 8 {
        let Some(t) = pick_target(bot) else { break };
        // hurt and with a moment before it arrives: eat first, the fight goes better with
        // the health coming back
        if health(bot) < 10.0 && t.pos.distance_to(crate::world::me(bot)) > 6.0 {
            crate::survival::eat_if_needed(bot).await?;
        }
        let limit = if t.role() == Role::Creeper { 25 } else { 30 };
        let out = fight(bot, t.entity, Duration::from_secs(limit)).await?;
        n += 1;
        if out == Outcome::Killed && matches!(t.kind, EntityKind::Enderman | EntityKind::Blaze) {
            // ender pearls and blaze rods are worth the steps
            crate::world::collect_drops(bot, t.pos, 6.0).await;
        }
        if !alive(bot) {
            break;
        }
    }
    Ok(n)
}

/// Kills one mob of `kinds` within `radius` (nearest), for what it drops. Picks the drops up.
pub async fn hunt(bot: &Client, kinds: &[EntityKind], radius: f64) -> Result<Outcome> {
    let me = crate::world::me(bot);
    let Some(m) = mobs_near(bot, radius)
        .into_iter()
        .filter(|m| kinds.contains(&m.kind) && !m.baby && !given_up(bot, m.entity))
        .min_by(|a, b| a.pos.distance_squared_to(me).total_cmp(&b.pos.distance_squared_to(me)))
    else {
        bail!("no {kinds:?} within {radius}");
    };
    let out = fight(bot, m.entity, Duration::from_secs(45)).await?;
    if out == Outcome::Killed {
        // the drop lands where it died; give the server a moment to spawn the items
        bot.wait_ticks(5).await;
        let at = mob(bot, m.entity).map(|x| x.pos).unwrap_or(m.pos);
        crate::world::collect_drops(bot, at, 6.0).await;
    }
    Ok(out)
}

/// `goto` with a deadline that stops to fight or eat when something needs it, then walks on
/// to the same goal. Every walk of the run goes through here (`world::goto_timeout`), so a
/// zombie met on the way is fought instead of followed home.
pub async fn goto_guarded<G: Goal + Clone + 'static>(bot: &Client, goal: G, limit: Duration) -> Result<()> {
    // game ticks, so the check means the same at any server tick rate; mining a block on
    // the way stands still too, so the window is longer than any block the path mines
    const STUCK_WINDOW: u32 = 100;
    let mut deadline = Instant::now() + limit;
    bot.start_goto(goal.clone());
    // make sure the goto event was handled before asking whether the goal is reached
    bot.wait_updates(1).await;
    let mut ticks = 0u32;
    let mut window = (0u32, crate::world::me(bot));
    loop {
        if bot.is_goto_target_reached() {
            return Ok(());
        }
        if Instant::now() > deadline {
            crate::world::stop_path(bot).await;
            bail!("path timed out");
        }
        bot.wait_ticks(1).await;
        ticks += 1;
        if ticks - window.0 >= STUCK_WINDOW {
            let now = crate::world::me(bot);
            if now.distance_to(window.1) < 0.5 {
                crate::world::stop_path(bot).await;
                bail!("path stuck: no move in {STUCK_WINDOW} ticks");
            }
            window = (ticks, now);
        }
        if ticks % 4 == 0 && crate::survival::urgent(bot) {
            crate::world::stop_path(bot).await;
            bot.walk(WalkDirection::None);
            let paused = Instant::now();
            crate::survival::tend(bot).await?;
            deadline += paused.elapsed();
            bot.start_goto(goal.clone());
            bot.wait_updates(1).await;
            window = (ticks, crate::world::me(bot));
        }
    }
}
