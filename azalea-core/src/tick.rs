use bevy_ecs::schedule::ScheduleLabel;

/// A Bevy schedule that runs every Minecraft game tick, i.e. every 50ms.
///
/// Many client systems run on this schedule, the most important one being
/// physics.
///
/// This schedule runs either zero or one times after every Bevy `Update`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, ScheduleLabel)]
pub struct GameTick;

/// The server's tick rate in ticks per second, as `f32` bits (`/tick rate`, sent by
/// `ClientboundTickingState`). Vanilla's client follows it; the schedule loop reads it each
/// update, so a server sped up for tests runs its bots at the same speed.
pub static TICK_RATE_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x41a0_0000); // 20.0

pub fn tick_rate() -> f32 {
    f32::from_bits(TICK_RATE_BITS.load(std::sync::atomic::Ordering::Relaxed))
}

pub fn set_tick_rate(rate: f32) {
    let rate = if rate.is_finite() { rate.clamp(1.0, 10_000.0) } else { 20.0 };
    TICK_RATE_BITS.store(rate.to_bits(), std::sync::atomic::Ordering::Relaxed);
}
