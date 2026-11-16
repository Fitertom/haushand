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
        // A portal from what the bot carries (10 obsidian, 4 corner blocks), lit, entered.
        "portal" => {
            if crate::portal::dimension(bot) != "minecraft:overworld" {
                return Ok(());
            }
            obtain(bot, Items::one(ItemKind::FlintAndSteel), 1).await?;
            // 4 corners and a pillar 3 high to reach the top (the pillar comes back)
            obtain(bot, craft::stone_material(), 7).await?;
            let site = crate::portal::build(bot).await?;
            crate::portal::light(bot, &site).await?;
            crate::portal::enter(bot, &site).await?;
        }
        // What is loaded around the bot (fortress bricks, spawners, gold, forests, lava).
        "survey" => {
            bot.wait_ticks(100).await;
            crate::nether::survey(bot);
        }
        "fortress" => {
            let p = crate::nether::find_fortress(bot).await?;
            info!("fortress: {p}, {:.0} away", p.center().distance_to(crate::world::me(bot)));
            crate::nether::walk_to(bot, p, 24.0, 40.0).await?;
            let s = crate::nether::find_spawner(bot, p).await?;
            crate::nether::walk_to(bot, s, 6.0, 40.0).await?;
            info!("at the spawner {s}: {:?}", crate::world::me(bot));
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
