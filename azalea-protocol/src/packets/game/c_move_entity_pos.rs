use std::io::{self, Cursor, Write};

use azalea_buf::{AzBuf, AzBufVar, BufReadError};
use azalea_core::{delta::PositionDelta8, entity_id::MinecraftEntityId};
use azalea_protocol_macros::ClientboundGamePacket;

use crate::common::movements::read_vec_delta;

/// Since 26.3 the on-ground flag and the number of steps share one VarInt (bit 0 on the
/// ground, the rest the steps), and a stepped delta is a chain; `delta` is its sum.
#[derive(ClientboundGamePacket, Clone, Debug, PartialEq)]
pub struct ClientboundMoveEntityPos {
    pub entity_id: MinecraftEntityId,
    pub delta: PositionDelta8,
    pub on_ground: bool,
}
impl AzBuf for ClientboundMoveEntityPos {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let entity_id = MinecraftEntityId::azalea_read_var(buf)?;
        let props = u32::azalea_read_var(buf)?;
        let delta = read_vec_delta(buf, props >> 1)?;
        Ok(Self { entity_id, delta, on_ground: props & 1 != 0 })
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        self.entity_id.azalea_write_var(buf)?;
        (self.on_ground as u32).azalea_write_var(buf)?;
        self.delta.azalea_write(buf)
    }
}
