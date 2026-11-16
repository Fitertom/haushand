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

/// The player's slots in player-menu numbering (0-45), read from whatever menu is open.
///
/// With a container open azalea updates the player's slots inside the container's menu and
/// the inventory menu only when it closes: counted from the inventory menu, a pickaxe
/// shift-clicked out of a crafting table was not there, and the craft "gave nothing".
pub fn player_slots(bot: &Client) -> Vec<ItemStack> {
    bot.query_self::<&azalea::entity::inventory::Inventory, _>(|inv| {
        let mut slots = inv.inventory_menu.slots();
        if inv.id != 0
            && let Some(open) = &inv.container_menu
        {
            let theirs = open.slots();
            // the container's last 36 slots are the player's main rows then the hotbar,
            // the same order as player-menu slots 9..45
            let start = theirs.len() - 36;
            for i in 0..36 {
                slots[9 + i] = theirs[start + i].clone();
            }
        }
        slots
    })
    .expect("bot has no inventory")
}

/// The player's own menu as last synced (for code that wants a `Menu`).
#[allow(dead_code)]
pub fn player_menu(bot: &Client) -> Menu {
    bot.query_self::<&azalea::entity::inventory::Inventory, _>(|inv| inv.inventory_menu.clone())
        .expect("bot has no inventory")
}

/// Main inventory and hotbar slots of the player menu.
pub const STORAGE: std::ops::Range<usize> = 9..45;
pub const HOTBAR: std::ops::Range<usize> = 36..45;

pub fn count(bot: &Client, items: &Items) -> i32 {
    let slots = player_slots(bot);
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
    let slots = player_slots(bot);
    let mut found: Vec<_> = STORAGE
        .filter(|&i| items.contains(slots[i].kind()) && slots[i].count() > 0)
        .map(|i| (i, slots[i].kind(), slots[i].count()))
        .collect();
    found.sort_by_key(|&(_, _, n)| -n);
    found
}

pub fn free_slots(bot: &Client) -> usize {
    let slots = player_slots(bot);
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
    let slots = player_slots(bot);
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
    if let Some(inv) = crate::craft::open_own_inventory(bot).await {
        inv.click(azalea::inventory::operations::SwapClick {
            source_slot: from as u16,
            target_slot: target,
        });
    }
    bot.wait_ticks(2).await;
    true
}
