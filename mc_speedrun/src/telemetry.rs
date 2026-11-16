//! A run's log for the report: one JSON object a line in `runs/<start>.jsonl`.
//!
//! Time is the game's: ticks since the bot joined (`ticks_connected`), so runs on a server
//! sped up for testing compare with runs at normal speed. Wall-clock seconds go along for
//! the record.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::{Mutex, OnceLock},
    time::Instant,
};

use azalea::{
    Client,
    protocol::packets::game::ClientboundGamePacket,
};
use serde_json::{Value, json};

struct Log {
    file: File,
    started: Instant,
}

static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

/// Opens a new run log. Later calls do nothing: one log a process.
pub fn start() -> std::io::Result<String> {
    std::fs::create_dir_all("runs")?;
    let name = format!(
        "runs/{}.jsonl",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    );
    let file = OpenOptions::new().create(true).append(true).open(&name)?;
    let _ = LOG.set(Mutex::new(Log { file, started: Instant::now() }));
    Ok(name)
}

/// Writes one record: `kind` plus `fields`, stamped with the game tick and wall seconds.
pub fn record(bot: &Client, kind: &str, fields: Value) {
    let Some(log) = LOG.get() else { return };
    let mut log = log.lock().unwrap();
    let mut obj = json!({
        "t": bot.ticks_connected(),
        "wall": log.started.elapsed().as_secs_f64(),
        "kind": kind,
    });
    if let (Value::Object(o), Value::Object(f)) = (&mut obj, fields) {
        o.extend(f);
    }
    let _ = writeln!(log.file, "{obj}");
}

/// Every second of game time: where we are and how we are.
pub fn sample(bot: &Client) {
    if bot.ticks_connected() % 20 != 0 {
        return;
    }
    let pos = bot.position().unwrap_or_default();
    let health = bot.query_self::<&azalea::entity::metadata::Health, _>(|h| h.0).unwrap_or(0.0);
    let food = bot.hunger().map(|h| h.food).unwrap_or(0);
    let dim = bot.world_name().map(|w| w.to_string()).unwrap_or_default();
    record(bot, "pos", json!({"x": pos.x, "y": pos.y, "z": pos.z, "dim": dim, "hp": health, "food": food}));
}

/// Advancements done, from the server's progress packets: the run's milestones as the game
/// itself counts them.
pub fn packet(bot: &Client, packet: &ClientboundGamePacket) {
    if let ClientboundGamePacket::UpdateAdvancements(p) = packet {
        for (id, progress) in &p.progress {
            let done = !progress.is_empty() && progress.values().all(|c| c.date.is_some());
            // recipe unlocks are advancements too; they are noise in a run's timeline
            if done && !id.to_string().contains("recipes/") {
                record(bot, "advancement", json!({"id": id.to_string()}));
            }
        }
    }
}
