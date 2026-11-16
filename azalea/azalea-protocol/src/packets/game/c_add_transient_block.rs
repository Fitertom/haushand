use azalea_block::BlockState;
use azalea_buf::AzBuf;
use azalea_core::position::BlockPos;
use azalea_protocol_macros::ClientboundGamePacket;

/// A block shown for a moment where there is none in the world.
#[derive(AzBuf, Clone, Debug, PartialEq, ClientboundGamePacket)]
pub struct ClientboundAddTransientBlock {
    pub pos: BlockPos,
    pub block_state: BlockState,
}
