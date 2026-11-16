use azalea_buf::AzBuf;
use azalea_protocol_macros::ClientboundConfigPacket;
use azalea_registry::identifier::Identifier;

#[derive(AzBuf, Clone, Debug, PartialEq, ClientboundConfigPacket)]
pub struct ClientboundPostEffects {
    pub post_effects: Vec<Identifier>,
}
