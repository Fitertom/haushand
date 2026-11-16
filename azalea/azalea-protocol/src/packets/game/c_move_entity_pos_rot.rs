use azalea_buf::AzBuf;
use azalea_core::{delta::PositionDelta8, entity_id::MinecraftEntityId};
use azalea_entity::LookDirection;
use azalea_protocol_macros::ClientboundGamePacket;

/// This packet is sent by the server when an entity moves less then 8 blocks.
/// See [`ClientboundMoveEntityPos`](super::c_move_entity_pos::ClientboundMoveEntityPos) for
/// the 26.3 properties VarInt and stepped delta.
#[derive(ClientboundGamePacket, Clone, Debug, PartialEq)]
pub struct ClientboundMoveEntityPosRot {
    pub entity_id: MinecraftEntityId,
    pub delta: PositionDelta8,
    pub look_direction: CompactLookDirection,
    pub on_ground: bool,
}
impl AzBuf for ClientboundMoveEntityPosRot {
    fn azalea_read(buf: &mut std::io::Cursor<&[u8]>) -> Result<Self, azalea_buf::BufReadError> {
        use azalea_buf::AzBufVar;
        let entity_id = MinecraftEntityId::azalea_read_var(buf)?;
        let props = u32::azalea_read_var(buf)?;
        let delta = crate::common::movements::read_vec_delta(buf, props >> 1)?;
        let look_direction = CompactLookDirection::azalea_read(buf)?;
        Ok(Self { entity_id, delta, look_direction, on_ground: props & 1 != 0 })
    }
    fn azalea_write(&self, buf: &mut impl std::io::Write) -> std::io::Result<()> {
        use azalea_buf::AzBufVar;
        self.entity_id.azalea_write_var(buf)?;
        (self.on_ground as u32).azalea_write_var(buf)?;
        self.delta.azalea_write(buf)?;
        self.look_direction.azalea_write(buf)
    }
}

#[derive(AzBuf, Clone, Copy, Debug, PartialEq, Default)]
pub struct CompactLookDirection {
    pub y_rot: i8,
    pub x_rot: i8,
}

impl From<CompactLookDirection> for LookDirection {
    fn from(l: CompactLookDirection) -> Self {
        LookDirection::new(
            (l.y_rot as i32 * 360) as f32 / 256.,
            (l.x_rot as i32 * 360) as f32 / 256.,
        )
    }
}
