use azalea_buf::AzBuf;
use azalea_core::position::Vec3;
use azalea_protocol_macros::ServerboundGamePacket;

/// Since 26.3 the client says where it took the teleport to and how it is turned.
#[derive(AzBuf, Clone, Debug, PartialEq, ServerboundGamePacket)]
pub struct ServerboundAcceptTeleportation {
    #[var]
    pub id: u32,
    pub pos: Vec3,
    pub y_rot: f32,
    pub x_rot: f32,
}
