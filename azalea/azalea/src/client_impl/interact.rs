use azalea_client::interact::{
    BlockStatePredictionHandler, EntityInteractEvent, StartUseItemEvent, pick::HitResultComponent,
};
use azalea_core::{hit_result::HitResult, position::BlockPos};
use azalea_entity::LookDirection;
use azalea_protocol::packets::game::{s_interact::InteractionHand, s_use_item::ServerboundUseItem};
use bevy_ecs::entity::Entity;

use crate::{Client, client_impl::error::AzaleaResult};

impl Client {
    /// Returns the current [`HitResult`], which is the block or entity in the
    /// client's crosshair.
    pub fn hit_result(&self) -> AzaleaResult<HitResult> {
        Ok((**self.component::<HitResultComponent>()?).clone())
    }

    /// Right-click a block.
    ///
    /// The behavior of this depends on the target block,
    /// and it'll either place the block you're holding in your hand or use the
    /// block you clicked (like toggling a lever).
    ///
    /// Note that this may trigger anticheats as it doesn't take into account
    /// whether you're actually looking at the block.
    pub fn block_interact(&self, position: BlockPos) {
        self.ecs.write().write_message(StartUseItemEvent {
            entity: self.entity,
            hand: InteractionHand::MainHand,
            force_block: Some(position),
        });
    }

    /// Right-click an entity.
    ///
    /// This can click through walls, which may trigger anticheats. If that
    /// behavior isn't desired, consider using [`Client::start_use_item`]
    /// instead.
    pub fn entity_interact(&self, entity: Entity) {
        self.ecs.write().trigger(EntityInteractEvent {
            client: self.entity,
            target: entity,
            location: None,
        });
    }

    /// Right-click the currently held item.
    ///
    /// If the item is consumable, then it'll act as if right-click was held
    /// until the item finishes being consumed. You can use this to eat food.
    ///
    /// If we're looking at a block or entity, then it will be clicked. Also see
    /// [`Client::block_interact`] and [`Client::entity_interact`].
    pub fn start_use_item(&self) {
        self.ecs.write().write_message(StartUseItemEvent {
            entity: self.entity,
            hand: InteractionHand::MainHand,
            force_block: None,
        });
    }

    /// Use the held item "in the air" (`ServerboundUseItem`) facing `direction`,
    /// whatever block is in the crosshair.
    ///
    /// [`Client::start_use_item`] sends only `ServerboundUseItemOn` when a
    /// block is in view, and relies on the server's answer to that; the
    /// vanilla client also sends `UseItem` when the block interaction passed.
    /// Items that raycast themselves (buckets, ender eyes, boats) only act on
    /// `UseItem`, so they need this. The server takes the rotation from the
    /// packet, so no separate look is needed first.
    pub fn use_item_facing(&self, direction: LookDirection) -> AzaleaResult<()> {
        let seq = {
            let mut ecs = self.ecs.write();
            let mut handler = ecs
                .get_mut::<BlockStatePredictionHandler>(self.entity)
                .ok_or(crate::client_impl::error::MissingComponentError {
                    entity_description: "Client",
                    entity: self.entity,
                    component: "BlockStatePredictionHandler",
                })?;
            handler.start_predicting()
        };
        self.write_packet(ServerboundUseItem {
            hand: InteractionHand::MainHand,
            seq,
            y_rot: direction.y_rot(),
            x_rot: direction.x_rot(),
        });
        Ok(())
    }
}
