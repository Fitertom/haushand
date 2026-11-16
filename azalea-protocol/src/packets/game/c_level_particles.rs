use azalea_buf::AzBuf;
use azalea_core::position::Vec3;
use azalea_entity::particle::Particle;
use azalea_protocol_macros::ClientboundGamePacket;

#[derive(AzBuf, ClientboundGamePacket, Clone, Debug, PartialEq)]
pub struct ClientboundLevelParticles {
    pub particle: Particle,
    pub override_limiter: bool,
    pub always_show: bool,
    pub pos: Vec3,
    pub x_dist: f32,
    pub y_dist: f32,
    pub z_dist: f32,
    pub x_max_speed: f32,
    pub y_max_speed: f32,
    pub z_max_speed: f32,
    #[var]
    pub count: u32,
    pub randomization: ParticleRandomization,
}

#[derive(AzBuf, Clone, Copy, Debug, PartialEq, Default)]
pub enum ParticleRandomization {
    #[default]
    Default,
    Alternative,
    AlternativeWithSpeed,
}
