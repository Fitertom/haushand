//! The run, stage by stage. Each stage is a goal on the inventory or the world, so running
//! it again after a failure (or a death) goes on from what is already there.

use azalea::{Client, prelude::*, registry::builtin::ItemKind};
use eyre::{Result, bail};
use tracing::info;

use crate::{
    craft::{self, items},
    inv::{self, Items},
    obtain::{hotbar_tools, obtain},
};

pub const STAGES: &[&str] = &["wood", "stone", "iron"];

pub async fn all(bot: &Client) -> Result<()> {
    for s in STAGES {
        stage(bot, s).await?;
    }
    Ok(())
}

pub async fn stage(bot: &Client, name: &str) -> Result<()> {
    crate::telemetry::record(bot, "stage_start", serde_json::json!({"stage": name}));
    let r = stage_body(bot, name).await;
    crate::telemetry::record(bot, "stage_end", serde_json::json!({"stage": name, "ok": r.is_ok(), "error": r.as_ref().err().map(|e| e.to_string())}));
    r
}

async fn stage_body(bot: &Client, name: &str) -> Result<()> {
    info!("stage {name}");
    crate::world::wait_for_world(bot).await;
    bot.chat(format!("stage {name}"));
    match name {
        // Enough wood for a table, the wooden pick and a stone set's sticks.
        "wood" => {
            obtain(bot, craft::logs(), 6).await?;
            obtain(bot, Items::one(ItemKind::WoodenPickaxe), 1).await?;
        }
        "stone" => {
            obtain(bot, Items::one(ItemKind::StonePickaxe), 1).await?;
            obtain(bot, Items::one(ItemKind::StoneAxe), 1).await?;
            obtain(bot, Items::one(ItemKind::StoneSword), 1).await?;
            obtain(bot, craft::stone_material(), 8).await?;
            obtain(bot, items!(Coal), 4).await.ok();
            hotbar_tools(bot).await;
        }
        "iron" => {
            // All the stage's iron in one smelt (pickaxe 3, bucket 3, sword 2): smelted per
            // tool, each batch waited for its own furnace.
            let ingots = 3 * (inv::count_kind(bot, ItemKind::IronPickaxe) == 0) as i32
                + 3 * (inv::count_kind(bot, ItemKind::Bucket) == 0) as i32
                + 2 * (inv::count_kind(bot, ItemKind::IronSword) == 0) as i32;
            obtain(bot, Items::one(ItemKind::IronIngot), ingots).await?;
            obtain(bot, Items::one(ItemKind::IronPickaxe), 1).await?;
            obtain(bot, Items::one(ItemKind::Bucket), 1).await?;
            obtain(bot, Items::one(ItemKind::IronSword), 1).await?;
            hotbar_tools(bot).await;
        }
        // What the bot thinks stands in a column under it and around: to check chunk decoding
        // against the server's view.
        // A short walk, our position logged each half second, to compare with the server's.
        "walk" => {
            let f = crate::world::feet(bot);
            let goal = azalea::pathfinder::goals::XZGoal { x: f.x + 10, z: f.z };
            bot.start_goto(goal);
            for _ in 0..30 {
                bot.wait_ticks(10).await;
                info!("walk at {:?} executing={}", crate::world::me(bot), bot.is_executing_path());
            }
        }
        // A path to PATHTO="x y z" (reach goal, as for mining), logging where we are.
        "pathto" => {
            let v: Vec<i32> = std::env::var("PATHTO")?.split_whitespace().map(|w| w.parse()).collect::<Result<_, _>>()?;
            let target = azalea::BlockPos::new(v[0], v[1], v[2]);
            info!("pathto: from {:?} to {target}", crate::world::me(bot));
            let r = crate::world::reach(bot, target).await;
            info!("pathto: {r:?}, now at {:?}", crate::world::me(bot));
        }
        // How many game ticks the client runs in ten real seconds: 200 at 20 TPS, 600 at 60.
        "tps" => {
            bot.wait_ticks(40).await;
            for _ in 0..3 {
                let t0 = bot.ticks_connected();
                let w0 = std::time::Instant::now();
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                let dt = bot.ticks_connected() - t0;
                info!("tps: {dt} ticks in {:.2}s = {:.1}/s, server rate {}", w0.elapsed().as_secs_f64(), dt as f64 / w0.elapsed().as_secs_f64(), azalea::core::tick::tick_rate());
            }
        }
        // Walk to the first log, logging where we stand and what is under us each second.
        "stand" => {
            let states = crate::world::states_of(&azalea::registry::tags::blocks::LOGS.clone().into_iter().collect::<Vec<_>>());
            if let Some(&log) = crate::world::find_blocks(bot, &states, 1, &[]).first() {
                info!("stand: walking to {log}");
                let chunks = crate::world::world_lock(bot).read().chunks.clone();
                bot.start_goto(azalea::pathfinder::goals::ReachBlockPosGoal::new(log, chunks));
            }
            for _ in 0..40 {
                bot.wait_ticks(20).await;
                let f = crate::world::feet(bot);
                let on_ground = bot.query_self::<&azalea::entity::Physics, _>(|p| p.on_ground()).unwrap_or(false);
                info!("stand: at {:?} ground={on_ground} under {:?} feet {:?}", crate::world::me(bot), crate::world::kind_at(bot, f.down(1)), crate::world::kind_at(bot, f));
            }
        }
        // Put a crafting table down near us (give one first).
        "place" => {
            let r = crate::place::place_near(bot, &items!(CraftingTable)).await;
            info!("place: result {r:?}");
        }
        // Craft one crafting table from planks given beforehand, logging every click.
        "crafttest" => {
            let dump = |bot: &Client, tag: &str| {
                let carried = bot.query_self::<&azalea::entity::inventory::Inventory, _>(|i| (i.carried.kind(), i.carried.count(), i.state_id)).ok();
                let slots: Vec<_> = crate::inv::player_slots(bot).iter().enumerate().filter(|(_, s)| s.is_present()).map(|(i, s)| (i, s.kind(), s.count())).collect();
                info!("crafttest {tag}: carried {carried:?} slots {slots:?}");
            };
            dump(bot, "start");
            let Ok(Some(h)) = bot.open_inventory() else { bail!("no inventory") };
            let src = crate::inv::slots_of(bot, &crate::craft::planks())[0].0;
            for cell in 1..=4usize {
                h.left_click(src); dump(bot, &format!("pick {src}"));
                h.right_click(cell); dump(bot, &format!("put {cell}"));
                h.left_click(src); dump(bot, &format!("back {src}"));
                bot.wait_ticks(5).await; dump(bot, "after server");
            }
            bot.wait_ticks(10).await; dump(bot, "grid ready");
            h.shift_click(0usize);
            bot.wait_ticks(10).await; dump(bot, "taken");
        }
        // Stock up on food (hunt, cook when a furnace and coal are near).
        "food" => crate::survival::stock_food(bot, 30).await?,
        // Combat tests; the bot must be op for the commands (rcon "op SpeedBot").
        // `fight`: one of each common hostile summoned 6 blocks away, fought one at a time.
        "fight" => {
            bot.chat("/time set midnight");
            if crate::inv::count_kind(bot, ItemKind::StoneSword) == 0 {
                bot.chat("/give @s stone_sword");
            }
            for kind in ["zombie", "skeleton", "spider", "creeper", "husk", "enderman"] {
                bot.chat(format!("/summon minecraft:{kind} ~6 ~ ~{}", if kind == "enderman" { " {AngerTime:600}" } else { "" }));
                bot.wait_ticks(20).await;
                let hp = crate::combat::health(bot);
                let t0 = std::time::Instant::now();
                let n = test_fight(bot).await?;
                info!("test fight {kind}: {n} fights in {:.1}s, hp {hp:.0} -> {:.0}, left {:?}", t0.elapsed().as_secs_f64(), crate::combat::health(bot), crate::combat::mobs_near(bot, 16.0).iter().map(|m| m.kind).collect::<Vec<_>>());
                bot.chat("/effect give @s instant_health 1 3");
                bot.wait_ticks(10).await;
            }
        }
        // `fightn <n>`-like: several zombies at once.
        "crowd" => {
            bot.chat("/time set midnight");
            for dx in [5, -5, 0] {
                bot.chat(format!("/summon minecraft:zombie ~{dx} ~ ~5"));
            }
            bot.wait_ticks(20).await;
            let n = test_fight(bot).await?;
            info!("test crowd: {n} fights, hp {:.0}", crate::combat::health(bot));
        }
        // `eat`: food given, the bar drained by a hunger effect, then eaten back up.
        "eat" => {
            bot.chat("/give @s cooked_beef 3");
            bot.chat("/give @s bread 3");
            bot.chat("/effect give @s hunger 60 200");
            for _ in 0..600 {
                if crate::survival::food_level(bot) <= 8 { break }
                bot.wait_ticks(5).await;
            }
            bot.chat("/effect clear @s hunger");
            bot.wait_ticks(10).await;
            info!("test eat: food {} before", crate::survival::food_level(bot));
            crate::survival::eat_if_needed(bot).await?;
            info!("test eat: food {} after, left beef {} bread {}", crate::survival::food_level(bot), crate::inv::count_kind(bot, ItemKind::CookedBeef), crate::inv::count_kind(bot, ItemKind::Bread));
        }
        // `hunt`: no food, hungry, a cow and a pig nearby: kill one and eat it raw.
        "hunt" => {
            bot.chat("/clear @s");
            bot.chat("/give @s stone_sword");
            bot.chat("/time set day");
            bot.chat("/summon minecraft:cow ~8 ~ ~3");
            bot.chat("/summon minecraft:pig ~-6 ~ ~6");
            bot.chat("/effect give @s hunger 60 200");
            for _ in 0..600 {
                if crate::survival::food_level(bot) <= 12 { break }
                bot.wait_ticks(5).await;
            }
            bot.chat("/effect clear @s hunger");
            bot.wait_ticks(10).await;
            info!("test hunt: food {} before", crate::survival::food_level(bot));
            crate::survival::tend(bot).await?;
            info!("test hunt: food {} after, carrying {} points", crate::survival::food_level(bot), crate::survival::food_points(bot));
        }
        // `lava`: lava put at our feet (fire resistance so a slow escape is not a death).
        "lava" => {
            bot.chat("/effect give @s fire_resistance 30");
            bot.wait_ticks(5).await;
            bot.chat("/setblock ~ ~ ~ lava");
            bot.wait_ticks(10).await;
            let urgent = crate::survival::urgent(bot);
            crate::survival::tend(bot).await?;
            info!("test lava: urgent {urgent}, now at {} in {:?}", crate::world::feet(bot), crate::world::kind_at(bot, crate::world::feet(bot)));
        }
        // `die`: killed once; after the respawn the stage runs again and ends.
        "die" => {
            static DIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !DIED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                bot.chat("/give @s cobblestone 5");
                bot.wait_ticks(10).await;
                bot.chat("/kill @s");
                bot.wait_ticks(200).await;
                bail!("still alive after /kill");
            }
            info!("test die: resumed after death at {}, cobblestone {}", crate::world::feet(bot), crate::inv::count_kind(bot, ItemKind::Cobblestone));
        }
        // `guard`: stand and tend for a while (at night on easy: does it survive).
        "guard" => {
            for _ in 0..120 {
                crate::survival::tend(bot).await?;
                bot.wait_ticks(10).await;
            }
        }
        "probe" => {
            let f = crate::world::feet(bot);
            use azalea::physics::collision::BlockWithShape;
            for dx in -3..=1 { for dy in 0..=2 { for dz in -1..=1 {
                let p = azalea::BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                let st = crate::world::block_at(bot, p);
                if !st.is_air() {
                    info!("near {} {} {} {:?} empty_shape={}", p.x, p.y, p.z, azalea::registry::builtin::BlockKind::from(st), st.is_collision_shape_empty());
                }
            }}}
            for (dx, dz) in [(0, 0), (3, 0), (0, 5), (-7, 2)] {
                for dy in -3..=1 {
                    let p = azalea::BlockPos::new(f.x + dx, f.y + dy, f.z + dz);
                    info!("probe {} {} {} {:?}", p.x, p.y, p.z, crate::world::kind_at(bot, p));
                }
            }
        }
        other => bail!("unknown stage {other}; stages: {}", STAGES.join(", ")),
    }
    Ok(())
}

/// Tends until no hostile is in view (or a minute passes); returns how many fights.
async fn test_fight(bot: &Client) -> Result<usize> {
    let t0 = std::time::Instant::now();
    let mut n = 0;
    while t0.elapsed() < std::time::Duration::from_secs(60) {
        let f = crate::combat::defend(bot).await?;
        n += f;
        if f == 0 {
            if crate::combat::mobs_near(bot, 20.0).iter().all(|m| !matches!(m.role(), crate::combat::Role::Melee | crate::combat::Role::Ranged | crate::combat::Role::Creeper)) {
                break;
            }
            bot.wait_ticks(5).await;
        }
    }
    Ok(n)
}
