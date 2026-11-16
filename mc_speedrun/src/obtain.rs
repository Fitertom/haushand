//! "Have N of this": the bot works out how — mine it, craft it from what it has (getting
//! the ingredients first), or smelt it. Every step checks the inventory first, so a run
//! stopped half way picks up where it was.

use std::{future::Future, pin::Pin, time::Duration};

use azalea::{BlockPos, Client, registry::builtin::{BlockKind, ItemKind}};
use eyre::{Result, bail};
use tracing::info;

use crate::{
    craft::{self, items},
    inv::{self, Items},
    world,
};

type Boxed<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// Blocks that drop `item` when mined, and the least pickaxe tier they need
/// (0 hand, 1 wood, 2 stone, 3 iron).
fn sources(item: ItemKind) -> Option<(Vec<BlockKind>, u8)> {
    use BlockKind as B;
    Some(match item {
        _ if craft::logs().contains(item) => (azalea::registry::tags::blocks::LOGS.clone().into_iter().collect(), 0),
        ItemKind::Cobblestone | ItemKind::CobbledDeepslate => (vec![B::Stone, B::Cobblestone, B::Deepslate, B::CobbledDeepslate], 1),
        ItemKind::Coal => (vec![B::CoalOre, B::DeepslateCoalOre], 1),
        ItemKind::RawIron => (vec![B::IronOre, B::DeepslateIronOre], 2),
        ItemKind::Diamond => (vec![B::DiamondOre, B::DeepslateDiamondOre], 3),
        ItemKind::Flint => (vec![B::Gravel], 0),
        ItemKind::Obsidian => (vec![B::Obsidian], 4),
        _ => return None,
    })
}

fn pickaxe_tier(bot: &Client) -> u8 {
    let has = |k| inv::count_kind(bot, k) > 0;
    if has(ItemKind::DiamondPickaxe) || has(ItemKind::NetheritePickaxe) {
        4
    } else if has(ItemKind::IronPickaxe) {
        3
    } else if has(ItemKind::StonePickaxe) {
        2
    } else if has(ItemKind::WoodenPickaxe) {
        1
    } else {
        0
    }
}

/// What to smelt into `item`.
fn smelts_from(item: ItemKind) -> Option<Items> {
    Some(match item {
        ItemKind::IronIngot => items!(RawIron),
        ItemKind::GoldIngot => items!(RawGold),
        ItemKind::CookedBeef => items!(Beef),
        ItemKind::CookedPorkchop => items!(Porkchop),
        ItemKind::CookedMutton => items!(Mutton),
        ItemKind::CookedChicken => items!(Chicken),
        ItemKind::Charcoal => craft::logs(),
        _ => return None,
    })
}

/// Have at least `want` of `items` in the inventory.
pub fn obtain<'a>(bot: &'a Client, items: Items, want: i32) -> Boxed<'a> {
    Box::pin(async move {
        let have = inv::count(bot, &items);
        if have >= want {
            return Ok(());
        }
        let first = items.0[0];
        info!("obtain {first:?} ×{want} (have {have})");

        // Mined.
        if let Some((blocks, tier)) = items.0.iter().find_map(|&k| sources(k)) {
            if tier > 0 {
                let tool = match tier {
                    1 => ItemKind::WoodenPickaxe,
                    2 => ItemKind::StonePickaxe,
                    3 => ItemKind::IronPickaxe,
                    _ => ItemKind::DiamondPickaxe,
                };
                if pickaxe_tier(bot) < tier {
                    obtain(bot, Items::one(tool), 1).await?;
                }
                hotbar_tools(bot).await;
            }
            let states = world::states_of(&blocks);
            return world::mine_until(bot, &states, &items, want).await;
        }

        // Smelted.
        if let Some(input) = smelts_from(first) {
            let n = want - have;
            obtain(bot, input.clone(), n).await?;
            return smelt(bot, &input, &items, n).await;
        }

        // Crafted.
        let Some(recipe) = items.0.iter().find_map(|&k| craft::recipe_for(k)) else {
            bail!("no way known to get {first:?}");
        };
        let times = (want - have + recipe.yields - 1) / recipe.yields;
        // The table first: made after the ingredients, it ate the planks set aside for a
        // pickaxe and the craft came up short. It is carried (picked up after use), so
        // having one means one in the inventory or one standing right here.
        if recipe.needs_table() && near_station(bot, BlockKind::CraftingTable).is_none() {
            obtain(bot, items!(CraftingTable), 1).await?;
        }
        // Again until all are there at once: getting a later ingredient can use up an earlier
        // one (the sticks for a pickaxe are made from the planks counted for its head).
        for round in 0.. {
            for (ingredient, per) in recipe.ingredients() {
                obtain(bot, ingredient, per * times).await?;
            }
            if recipe.ingredients().iter().all(|(i, per)| inv::count(bot, i) >= per * times) {
                break;
            }
            if round == 3 {
                bail!("ingredients for {} keep running out", recipe.name);
            }
        }
        if recipe.needs_table() {
            let table = station(bot, BlockKind::CraftingTable, &items!(CraftingTable)).await?;
            let Ok(Some(handle)) = bot.open_container_at(table).await else {
                bail!("crafting table at {table} did not open");
            };
            let r = craft::craft(bot, &recipe, times, Some(&handle)).await;
            drop(handle);
            bot.wait_ticks(2).await;
            pick_up(bot, table, &items!(CraftingTable)).await;
            r?;
        } else {
            craft::craft(bot, &recipe, times, None).await?;
        }
        Ok(())
    })
}

/// A standing `kind` (crafting table, furnace) close enough to use without a walk worth
/// the name. Farther ones are left: walking back to one, out of a pit just dug, timed out.
fn near_station(bot: &Client, kind: BlockKind) -> Option<BlockPos> {
    let states = world::states_of(&[kind]);
    world::find_blocks(bot, &states, 1, &[]).first().copied().filter(|p| p.center().distance_to(world::me(bot)) < 5.0)
}

/// A `kind` station to use right here: one standing near, or ours put down (the item must
/// be in the inventory). Walks within reach of it.
async fn station(bot: &Client, kind: BlockKind, item: &Items) -> Result<BlockPos> {
    let pos = match near_station(bot, kind) {
        Some(p) => p,
        None => crate::place::place_near(bot, item).await?,
    };
    world::reach(bot, pos).await?;
    Ok(pos)
}

/// Mines a station we put down and picks it up, to carry it to where it is needed next.
/// A failure only costs a new one later, so it is logged and not returned.
async fn pick_up(bot: &Client, pos: BlockPos, item: &Items) {
    let before = inv::count(bot, item);
    if let Err(e) = world::mine_at(bot, pos).await {
        info!("could not pick up {item:?} at {pos}: {e}");
        return;
    }
    // the drop reaches the inventory a few ticks after it is walked over
    for _ in 0..20 {
        if inv::count(bot, item) > before {
            return;
        }
        bot.wait_ticks(2).await;
        world::collect_drops(bot, pos.center(), 8.0).await;
    }
    info!("picked {item:?} at {pos} but it did not come back");
}

fn fuel() -> Items {
    let mut f = items!(Coal, Charcoal);
    f.0.extend(craft::planks().0);
    f.0.extend(craft::logs().0);
    f
}

/// Smelts `n` of `input` into `output` in a furnace, fuelled by coal or wood.
async fn smelt(bot: &Client, input: &Items, output: &Items, n: i32) -> Result<()> {
    // The furnace before the fuel: making it can take a table, made of the fuel's planks.
    if near_station(bot, BlockKind::Furnace).is_none() {
        obtain(bot, items!(Furnace), 1).await?;
    }
    // A coal smelts 8, a plank 1.5: take coal when there is any, else planks.
    let fuel_items = fuel();
    if inv::count(bot, &items!(Coal, Charcoal)) * 8 < n {
        let planks_needed = (n * 2 + 2) / 3;
        if inv::count(bot, &craft::planks()) < planks_needed {
            obtain(bot, craft::planks(), planks_needed).await?;
        }
    }
    let pos = station(bot, BlockKind::Furnace, &items!(Furnace)).await?;
    let Ok(Some(handle)) = bot.open_container_at(pos).await else {
        bail!("furnace at {pos} did not open");
    };
    // Furnace menu: 0 ingredient, 1 fuel, 2 result, then the player's rows from 3.
    let map = |s| inv::player_slot_in(3, s);
    let mut left = n;
    for (slot, _, count) in inv::slots_of(bot, input) {
        if left == 0 {
            break;
        }
        let put = count.min(left);
        handle.left_click(map(slot));
        for _ in 0..put {
            handle.right_click(0usize);
        }
        handle.left_click(map(slot));
        left -= put;
    }
    bot.wait_ticks(2).await;
    let coal = inv::slots_of(bot, &items!(Coal, Charcoal));
    let fuel_slots = if coal.is_empty() { inv::slots_of(bot, &fuel_items) } else { coal };
    if let Some(&(slot, _, _)) = fuel_slots.first() {
        handle.left_click(map(slot));
        handle.left_click(1usize);
        handle.left_click(map(slot));
    }
    // 10 s an item; take what is done as it comes.
    let before = inv::count(bot, output);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12 * n as u64 + 10);
    while inv::count(bot, output) < before + n && tokio::time::Instant::now() < deadline {
        bot.wait_ticks(20).await;
        handle.shift_click(2usize);
    }
    handle.shift_click(0usize);
    handle.shift_click(1usize);
    bot.wait_ticks(2).await;
    drop(handle);
    bot.wait_ticks(2).await;
    pick_up(bot, pos, &items!(Furnace)).await;
    if inv::count(bot, output) < before + n {
        bail!("furnace gave {} of {n}", inv::count(bot, output) - before);
    }
    Ok(())
}

/// Moves pickaxes, axes and the sword into the hotbar: the auto tool only looks there.
pub async fn hotbar_tools(bot: &Client) {
    use ItemKind::*;
    let tools = [
        [DiamondPickaxe, IronPickaxe, StonePickaxe, WoodenPickaxe],
        [DiamondAxe, IronAxe, StoneAxe, WoodenAxe],
        [DiamondShovel, IronShovel, StoneShovel, WoodenShovel],
        [DiamondSword, IronSword, StoneSword, WoodenSword],
    ];
    for (hotbar, group) in tools.iter().enumerate() {
        let slots = inv::player_slots(bot);
        let Some(best) = group.iter().find(|&&k| inv::count_kind(bot, k) > 0) else { continue };
        if slots[36 + hotbar].kind() == *best {
            continue;
        }
        if let Some(&(from, _, _)) = inv::slots_of(bot, &Items::one(*best)).first()
            && let Some(h) = crate::craft::open_own_inventory(bot).await
        {
            h.click(azalea::inventory::operations::SwapClick { source_slot: from as u16, target_slot: hotbar as u8 });
            bot.wait_ticks(2).await;
        }
    }
}
