use azalea_buf::AzBuf;
use azalea_protocol_macros::ServerboundGamePacket;

/// A left click, since 26.3; it took the place of `ServerboundSwing` and carries nothing.
#[derive(AzBuf, Clone, Debug, PartialEq, ServerboundGamePacket)]
pub struct ServerboundPunch;
