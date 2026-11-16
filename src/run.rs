//! The run, stage by stage. Each stage is a goal on the inventory or the world, so running
//! it again after a failure (or a death) goes on from what is already there.

use azalea::{Client, registry::builtin::ItemKind};
use eyre::{Result, bail};
use tracing::info;

use crate::{
    craft::{self, items},
    inv::Items,
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
    info!("stage {name}");
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
            obtain(bot, Items::one(ItemKind::IronPickaxe), 1).await?;
            obtain(bot, Items::one(ItemKind::Bucket), 1).await?;
            obtain(bot, Items::one(ItemKind::IronSword), 1).await?;
            hotbar_tools(bot).await;
        }
        other => bail!("unknown stage {other}; stages: {}", STAGES.join(", ")),
    }
    Ok(())
}
