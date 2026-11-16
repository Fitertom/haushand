//! Breaking a block, done by the bot itself: the tool, the look, the timing, the packets.
//!
//! As a player does it: the best tool in hand, the crosshair on a face the eyes see (our own
//! ray, nothing in between, in reach), held there while the block breaks. The vanilla
//! server breaks a block when it gets `StopDestroyBlock` and its own count of the progress
//! since `StartDestroyBlock` is at least 0.7 of the block (at once, on the start, for a
//! block broken in one tick); the bot counts the progress itself tick by tick with the
//! game's rule (tool, on ground, in water, effects), sends the stop once the block is done,
//! and takes it as broken only when the server's block update says so.

use azalea::{
    BlockPos, Client,
    block::BlockState,
    core::direction::Direction,
    entity::{ActiveEffects, Attributes, FluidOnEyes, Physics, inventory::Inventory, mining::get_mine_progress},
    interact::BlockStatePredictionHandler,
    protocol::packets::game::s_player_action::{Action, ServerboundPlayerAction},
    registry::builtin::BlockKind,
};
use eyre::{Result, bail};
use tracing::debug;

use crate::{
    click,
    world::{block_at, world_lock},
};

fn seq(bot: &Client) -> u32 {
    let mut ecs = bot.ecs.write();
    ecs.get_mut::<BlockStatePredictionHandler>(bot.entity).map(|mut h| h.start_predicting()).unwrap_or(1)
}

fn action(bot: &Client, action: Action, pos: BlockPos, face: Direction) {
    let seq = seq(bot);
    bot.write_packet(ServerboundPlayerAction { action, pos, direction: face, seq });
}

/// Progress a tick on `state` with the item in hotbar slot `slot`, as the game counts it.
fn progress_with(bot: &Client, state: BlockState, slot: usize) -> f32 {
    let block = state.to_trait();
    bot.query_self::<(&Inventory, &FluidOnEyes, &Physics, &Attributes, &ActiveEffects), _>(|(inv, fluid, physics, attrs, effects)| {
        let item = inv.inventory_menu.slot(36 + slot).cloned().unwrap_or_default();
        get_mine_progress(block, &item, fluid, physics, attrs, effects)
    })
    .unwrap_or(0.0)
}

/// The hotbar slot that breaks `state` fastest, preferring one that also gets its drop.
fn best_slot(bot: &Client, state: BlockState) -> usize {
    // the held one unless another is strictly faster: grass breaks at once with anything,
    // and swapping the sword away for it costs the fight its charge
    let held = bot.selected_hotbar_slot().unwrap_or(0) as usize;
    let mut best = (held, progress_with(bot, state, held));
    for slot in 0..9 {
        let p = progress_with(bot, state, slot);
        if p > best.1 {
            best = (slot, p);
        }
    }
    best.0
}

/// Whether the block can be broken at all by a player: not air, not a fluid, not bedrock,
/// a portal frame or another block with no destroy time.
pub fn breakable(state: BlockState) -> bool {
    let block = state.to_trait();
    !state.is_air()
        && !matches!(BlockKind::from(state), BlockKind::Water | BlockKind::Lava | BlockKind::BubbleColumn)
        && block.behavior().destroy_time >= 0.0
}

/// Breaks the block at `pos`, which must be in reach. Returns when the server has it as air
/// (or what replaced it, as water flowing in).
pub async fn dig(bot: &Client, pos: BlockPos) -> Result<()> {
    let Some(state) = world_lock(bot).read().get_block_state(pos) else {
        bail!("{pos} is not loaded");
    };
    if state.is_air() {
        return Ok(());
    }
    if !breakable(state) {
        bail!("{pos} ({:?}) cannot be broken", BlockKind::from(state));
    }
    if !click::height_ok(bot, pos.y) {
        bail!("{pos} is outside the world's height");
    }
    let Some((face, point)) = click::sight(bot, pos) else {
        bail!("{pos} is not in sight");
    };

    let slot = best_slot(bot, state);
    if bot.selected_hotbar_slot().unwrap_or(0) as usize != slot {
        bot.set_selected_hotbar_slot(slot as u8);
    }
    click::look(bot, point).await; // the tick it waits also sends the slot change
    if !click::sees(bot, pos, face, point) {
        bail!("{pos} went out of sight");
    }

    let kind = BlockKind::from(state);
    action(bot, Action::StartDestroyBlock, pos, face);
    let mut done = progress_with(bot, state, slot);
    let mut ticks = 1u32;
    if done < 1.0 {
        // one tick of progress each game tick, as the server counts it; the stop once done
        let limit = (1.0 / done.max(1e-4)).ceil() as u32 + 40;
        while done < 1.0 {
            bot.wait_ticks(1).await;
            ticks += 1;
            if block_at(bot, pos) != state {
                break; // gone already (another hand, water, a fall)
            }
            // the crosshair must stay on it, as a player's does (a step, a mob in between)
            if !click::sees(bot, pos, face, point) || ticks > limit {
                action(bot, Action::AbortDestroyBlock, pos, face);
                bail!("lost {pos} while breaking it ({ticks} ticks)");
            }
            done += progress_with(bot, state, slot);
        }
        if block_at(bot, pos) == state {
            action(bot, Action::StopDestroyBlock, pos, face);
        }
    }
    // the server's word: the block update
    for _ in 0..30 {
        if block_at(bot, pos) != state {
            debug!("broke {kind:?} at {pos} in {ticks} ticks");
            return Ok(());
        }
        bot.wait_ticks(1).await;
    }
    action(bot, Action::AbortDestroyBlock, pos, face);
    bail!("the server did not break {kind:?} at {pos} after {ticks} ticks")
}
