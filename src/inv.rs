//! Inventory queries over the player's own menu.
//!
//! Slot numbers here are the PLAYER menu's (0 craft result, 1-4 craft grid, 5-8 armour,
//! 9-35 main, 36-44 hotbar, 45 offhand). A container menu shifts the player's slots, so
//! anything clicking inside an open container maps through `player_slot_in`.

use azalea::{
    Client,
    inventory::{ItemStack, Menu},
    registry::builtin::ItemKind,
};

/// A set of interchangeable items: any planks, any log, coal or charcoal.
#[derive(Clone, Debug)]
pub struct Items(pub Vec<ItemKind>);

impl Items {
    pub fn one(kind: ItemKind) -> Self {
        Self(vec![kind])
    }
    pub fn contains(&self, kind: ItemKind) -> bool {
        self.0.contains(&kind)
    }
}

impl From<ItemKind> for Items {
    fn from(kind: ItemKind) -> Self {
        Self::one(kind)
    }
}

pub fn player_menu(bot: &Client) -> Menu {
    bot.query_self::<&azalea::entity::inventory::Inventory, _>(|inv| inv.inventory_menu.clone())
        .expect("bot has no inventory")
}

/// Main inventory and hotbar slots of the player menu.
pub const STORAGE: std::ops::Range<usize> = 9..45;
pub const HOTBAR: std::ops::Range<usize> = 36..45;

pub fn count(bot: &Client, items: &Items) -> i32 {
    let menu = player_menu(bot);
    let slots = menu.slots();
    STORAGE
        .chain(45..46)
        .filter_map(|i| slots.get(i))
        .filter(|s| items.contains(s.kind()))
        .map(ItemStack::count)
        .sum()
}

pub fn count_kind(bot: &Client, kind: ItemKind) -> i32 {
    count(bot, &Items::one(kind))
}

/// Player-menu slots holding any of `items`, largest stack first.
pub fn slots_of(bot: &Client, items: &Items) -> Vec<(usize, ItemKind, i32)> {
    let menu = player_menu(bot);
    let slots = menu.slots();
    let mut found: Vec<_> = STORAGE
        .filter(|&i| items.contains(slots[i].kind()) && slots[i].count() > 0)
        .map(|i| (i, slots[i].kind(), slots[i].count()))
        .collect();
    found.sort_by_key(|&(_, _, n)| -n);
    found
}

pub fn free_slots(bot: &Client) -> usize {
    let menu = player_menu(bot);
    let slots = menu.slots();
    STORAGE.filter(|&i| slots[i].is_empty()).count()
}

/// A player-menu slot as numbered inside a container menu of `container_slots` own slots
/// (a crafting table has 10, a furnace 3): the player's main rows follow the container's.
pub fn player_slot_in(container_slots: usize, player_slot: usize) -> usize {
    debug_assert!(STORAGE.contains(&player_slot));
    container_slots + (player_slot - 9)
}

/// Puts an item of `items` into the hotbar and selects it. Returns false when there is none.
pub async fn hold(bot: &Client, items: &Items) -> bool {
    let menu = player_menu(bot);
    let slots = menu.slots();
    if let Some(i) = HOTBAR.clone().find(|&i| items.contains(slots[i].kind())) {
        bot.set_selected_hotbar_slot((i - 36) as u8);
        bot.wait_updates(1).await;
        return true;
    }
    let Some(&(from, _, _)) = slots_of(bot, items).first() else {
        return false;
    };
    // Swap it into the selected hotbar slot: a hotbar-swap click moves it in one packet,
    // whatever was there goes back where the item came from.
    let target = bot.selected_hotbar_slot().unwrap_or(0);
    if let Ok(Some(inv)) = bot.open_inventory() {
        inv.click(azalea::inventory::operations::SwapClick {
            source_slot: from as u16,
            target_slot: target,
        });
    }
    bot.wait_ticks(2).await;
    true
}
