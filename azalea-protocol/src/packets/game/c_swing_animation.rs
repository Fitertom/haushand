use azalea_buf::AzBuf;
use azalea_core::entity_id::MinecraftEntityId;
use azalea_inventory::components::AttackAnimation;
use azalea_protocol_macros::ClientboundGamePacket;

use crate::packets::game::s_interact::InteractionHand;

#[derive(AzBuf, Clone, Debug, PartialEq, ClientboundGamePacket)]
pub struct ClientboundSwingAnimation {
    #[var]
    pub entity_id: MinecraftEntityId,
    pub hand: InteractionHand,
    /// Sent with the same codec as the item components' swing animation.
    pub animation: AttackAnimation,
}
