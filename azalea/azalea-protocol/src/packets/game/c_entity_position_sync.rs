use azalea_buf::AzBuf;
use azalea_core::entity_id::MinecraftEntityId;
use azalea_protocol_macros::ClientboundGamePacket;

use crate::common::movements::PositionPath;

/// Since 26.3 a path and the rotation, no velocity.
#[derive(AzBuf, ClientboundGamePacket, Clone, Debug, PartialEq)]
pub struct ClientboundEntityPositionSync {
    #[var]
    pub id: MinecraftEntityId,
    pub position: PositionPath,
    pub y_rot: f32,
    pub x_rot: f32,
    pub on_ground: bool,
}
