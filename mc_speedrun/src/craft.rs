//! Crafting by clicking ingredients into a grid, as a player does.
//!
//! The recipe book packet would need the server's recipe display ids; clicks need nothing
//! but the menu layout, and the server stays authoritative over what comes out.

use azalea::{Client, registry::builtin::ItemKind};
use eyre::{Result, bail};
use tracing::info;

use crate::inv::{self, Items};

#[derive(Clone, Debug)]
pub struct Recipe {
    pub name: &'static str,
    /// What one craft gives (a group when the output follows the input, as planks).
    pub output: Items,
    pub yields: i32,
    /// Rows of the pattern; a space is an empty cell. At most 3×3, 2×2 fits the inventory.
    pub pattern: &'static [&'static str],
    pub key: Vec<(char, Items)>,
}

impl Recipe {
    pub fn needs_table(&self) -> bool {
        self.pattern.len() > 2 || self.pattern.iter().any(|r| r.chars().count() > 2)
    }

    fn ingredient(&self, c: char) -> &Items {
        &self.key.iter().find(|(k, _)| *k == c).expect("pattern char not in key").1
    }

    /// How many of each key char one craft takes.
    pub fn per_craft(&self) -> Vec<(char, i32)> {
        let mut out: Vec<(char, i32)> = Vec::new();
        for row in self.pattern {
            for c in row.chars().filter(|c| *c != ' ') {
                match out.iter_mut().find(|(k, _)| *k == c) {
                    Some((_, n)) => *n += 1,
                    None => out.push((c, 1)),
                }
            }
        }
        out
    }

    pub fn ingredients(&self) -> Vec<(Items, i32)> {
        self.per_craft().into_iter().map(|(c, n)| (self.ingredient(c).clone(), n)).collect()
    }
}

/// Crafts `times` crafts of `recipe` in the open grid: the player's own 2×2 when `table` is
/// None, else the crafting table menu already opened by the caller.
pub async fn craft(bot: &Client, recipe: &Recipe, times: i32, table: Option<&azalea::container::ContainerHandle>) -> Result<()> {
    if recipe.needs_table() && table.is_none() {
        bail!("{} needs a crafting table", recipe.name);
    }
    let width = if table.is_some() { 3 } else { 2 };
    // Container slots before the player's rows: crafting table 10 (result + 9), inventory 0
    // (the player menu IS the player's numbering).
    let map = |player_slot: usize| if table.is_some() { inv::player_slot_in(10, player_slot) } else { player_slot };

    let mut left = times;
    while left > 0 {
        // One stack holds 64, so at most 64 crafts per round.
        let round = left.min(64);
        let before = inv::count(bot, &recipe.output);
        let own;
        let menu_handle: &azalea::container::ContainerHandleRef = match table {
            Some(t) => t,
            None => {
                own = match open_own_inventory(bot).await {
                    Some(h) => h,
                    None => bail!("another container is open"),
                };
                &own
            }
        };
        // leftovers from an earlier failed craft would change the recipe in the grid
        clear_grid(bot, menu_handle, width).await;
        settle_menu(bot).await;
        for (r, row) in recipe.pattern.iter().enumerate() {
            for (c, ch) in row.chars().enumerate() {
                if ch == ' ' {
                    continue;
                }
                let cell = 1 + r * width + c;
                let mut need = round;
                // Sources are read from the menu before each cell, once the server has answered
                // the clicks before: its answers come late and overwrite what the client
                // predicted, and a source read too early was a stack already moved.
                settle_menu(bot).await;
                for (slot, _, n) in inv::slots_of(bot, recipe.ingredient(ch)) {
                    if need == 0 {
                        break;
                    }
                    let take = n.min(need);
                    menu_handle.left_click(map(slot));
                    for _ in 0..take {
                        menu_handle.right_click(cell);
                    }
                    menu_handle.left_click(map(slot));
                    need -= take;
                }
                if need > 0 {
                    let menu = menu_handle.menu().ok().flatten();
                    info!(
                        "short of {:?} at cell {cell}: player menu {:?}; open menu {:?}",
                        recipe.ingredient(ch).0.first(),
                        inv::slots_of(bot, recipe.ingredient(ch)),
                        menu.map(|m| m.slots().iter().enumerate().filter(|(_, s)| s.is_present()).map(|(i, s)| (i, s.kind(), s.count())).collect::<Vec<_>>())
                    );
                    clear_grid(bot, menu_handle, width).await;
                    bail!("not enough {:?} for {}", recipe.ingredient(ch), recipe.name);
                }
            }
        }
        // The result is the server's: wait until it shows in slot 0 (a fixed wait was too
        // short once the server ran three times faster).
        let mut ready = false;
        for _ in 0..40 {
            bot.wait_ticks(2).await;
            if menu_handle.menu().ok().flatten().is_some_and(|m| m.slots().first().is_some_and(|s| s.is_present())) {
                ready = true;
                break;
            }
        }
        if !ready {
            clear_grid(bot, menu_handle, width).await;
            bail!("no result for {} in the grid", recipe.name);
        }
        // Shift-click on the result takes every craft the grid allows into the inventory.
        menu_handle.shift_click(0usize);
        for _ in 0..20 {
            bot.wait_ticks(2).await;
            if inv::count(bot, &recipe.output) > before {
                break;
            }
        }
        clear_grid(bot, menu_handle, width).await;
        let got = inv::count(bot, &recipe.output) - before;
        info!("crafted {} ×{} (+{got})", recipe.name, round);
        if got <= 0 {
            bail!("crafting {} gave nothing", recipe.name);
        }
        left -= round;
    }
    Ok(())
}

/// The player's own inventory as a handle. A container left open (seen once after
/// smelting: the furnace's id still current) is closed first, with a log line saying which.
pub async fn open_own_inventory(bot: &Client) -> Option<azalea::container::ContainerHandle> {
    for attempt in 0..3 {
        if let Ok(Some(h)) = bot.open_inventory() {
            return Some(h);
        }
        let open = bot.query_self::<&azalea::entity::inventory::Inventory, _>(|i| (i.id, i.container_menu.as_ref().map(|m| format!("{:?}", std::mem::discriminant(m))))).ok();
        info!("inventory busy (attempt {attempt}): open container {open:?}; closing it");
        if let Ok(h) = bot.get_inventory() {
            h.close();
        }
        bot.wait_ticks(2).await;
    }
    None
}

/// Waits until the menu's state id has not changed for a few ticks: the server has answered
/// every click sent.
async fn settle_menu(bot: &Client) {
    let state = |bot: &Client| bot.query_self::<&azalea::entity::inventory::Inventory, _>(|i| i.state_id).ok();
    let mut last = state(bot);
    let mut still = 0;
    for _ in 0..40 {
        bot.wait_ticks(1).await;
        let now = state(bot);
        if now == last {
            still += 1;
            if still >= 3 {
                return;
            }
        } else {
            still = 0;
            last = now;
        }
    }
}

/// Anything left in the grid goes back into the inventory, or it would be crafted next time.
async fn clear_grid(bot: &Client, handle: &azalea::container::ContainerHandleRef, width: usize) {
    for cell in 1..=width * width {
        handle.shift_click(cell);
    }
    bot.wait_ticks(1).await;
}

macro_rules! items {
    ($($k:ident),+) => { Items(vec![$(ItemKind::$k),+]) };
}
pub(crate) use items;

pub fn planks() -> Items {
    Items(azalea::registry::tags::items::PLANKS.clone().into_iter().collect())
}
pub fn logs() -> Items {
    Items(azalea::registry::tags::items::LOGS_THAT_BURN.clone().into_iter().collect())
}
pub fn stone_material() -> Items {
    Items(azalea::registry::tags::items::STONE_TOOL_MATERIALS.clone().into_iter().collect())
}

/// The recipes a run needs.
pub fn recipes() -> Vec<Recipe> {
    let r = |name, output: Items, yields, pattern, key: Vec<(char, Items)>| Recipe { name, output, yields, pattern, key };
    let p = planks;
    let s = || Items::one(ItemKind::Stick);
    let c = stone_material;
    let i = || Items::one(ItemKind::IronIngot);
    vec![
        r("planks", p(), 4, &["L"], vec![('L', logs())]),
        r("stick", s(), 4, &["P", "P"], vec![('P', p())]),
        r("crafting_table", items!(CraftingTable), 1, &["PP", "PP"], vec![('P', p())]),
        r("wooden_pickaxe", items!(WoodenPickaxe), 1, &["PPP", " S ", " S "], vec![('P', p()), ('S', s())]),
        r("wooden_axe", items!(WoodenAxe), 1, &["PP", "PS", " S"], vec![('P', p()), ('S', s())]),
        r("stone_pickaxe", items!(StonePickaxe), 1, &["CCC", " S ", " S "], vec![('C', c()), ('S', s())]),
        r("stone_axe", items!(StoneAxe), 1, &["CC", "CS", " S"], vec![('C', c()), ('S', s())]),
        r("stone_sword", items!(StoneSword), 1, &["C", "C", "S"], vec![('C', c()), ('S', s())]),
        r("stone_shovel", items!(StoneShovel), 1, &["C", "S", "S"], vec![('C', c()), ('S', s())]),
        r("furnace", items!(Furnace), 1, &["CCC", "C C", "CCC"], vec![('C', c())]),
        r("iron_pickaxe", items!(IronPickaxe), 1, &["III", " S ", " S "], vec![('I', i()), ('S', s())]),
        r("iron_sword", items!(IronSword), 1, &["I", "I", "S"], vec![('I', i()), ('S', s())]),
        r("bucket", items!(Bucket), 1, &["I I", " I "], vec![('I', i())]),
        r("flint_and_steel", items!(FlintAndSteel), 1, &["I ", " F"], vec![('I', i()), ('F', items!(Flint))]),
        r("shield", items!(Shield), 1, &["PIP", "PPP", " P "], vec![('P', p()), ('I', i())]),
        r("diamond_pickaxe", items!(DiamondPickaxe), 1, &["DDD", " S ", " S "], vec![('D', items!(Diamond)), ('S', s())]),
        r("blaze_powder", items!(BlazePowder), 2, &["B"], vec![('B', items!(BlazeRod))]),
        r("ender_eye", items!(EnderEye), 1, &["PB"], vec![('P', items!(EnderPearl)), ('B', items!(BlazePowder))]),
        r("white_bed", items!(WhiteBed), 1, &["WWW", "PPP"], vec![('W', items!(WhiteWool)), ('P', p())]),
        r("bow", items!(Bow), 1, &[" TS", "T S", " TS"], vec![('T', s()), ('S', items!(String))]),
        r("arrow", items!(Arrow), 4, &["F", "S", "E"], vec![('F', items!(Flint)), ('S', s()), ('E', items!(Feather))]),
    ]
}

pub fn recipe_for(output: ItemKind) -> Option<Recipe> {
    recipes().into_iter().find(|r| r.output.contains(output))
}
