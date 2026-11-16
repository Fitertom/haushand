use std::io::{self, Cursor, Write};

use azalea_buf::{AzBuf, BufReadError};
use azalea_protocol_macros::ClientboundConfigPacket;
use azalea_registry::identifier::Identifier;
use simdnbt::owned::{NbtCompound, NbtTag};

#[derive(ClientboundConfigPacket, Clone, Debug, PartialEq)]
pub struct ClientboundRegistryData {
    pub registry_id: Identifier,
    pub entries: Vec<(Identifier, Option<NbtCompound>)>,
}

/// Since 26.3 some synced registries (number providers) send an entry as a bare tag, not a
/// compound; such a tag is kept under the key `value`, so one odd registry does not lose
/// the whole packet (and with it the dimension types).
impl AzBuf for ClientboundRegistryData {
    fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
        let registry_id = Identifier::azalea_read(buf)?;
        let entries = Vec::<(Identifier, Option<NbtTag>)>::azalea_read(buf)?
            .into_iter()
            .map(|(id, tag)| {
                let compound = tag.map(|tag| match tag {
                    NbtTag::Compound(c) => c,
                    other => NbtCompound::from_values(vec![("value".into(), other)]),
                });
                (id, compound)
            })
            .collect();
        Ok(Self { registry_id, entries })
    }
    fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
        self.registry_id.azalea_write(buf)?;
        self.entries.azalea_write(buf)
    }
}
