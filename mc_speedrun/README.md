# Speedrun bot (Minecraft 26.3)

Start with HANDOFF.md (project, status, testing, Azalea port).

- `mc_speedrun/` — the bot (Rust, nightly).
- `azalea/` — Azalea ported to Minecraft 26.3 (protocol 777): new blocks/items/registries,
  13 new item components, the new entity movement encoding (VecDelta / PositionPath),
  AcceptTeleportation with position, Punch instead of Swing, advancements positions,
  particles, explosion, optional game mode as VarInt.

Build: put both folders side by side, then `cd mc_speedrun && cargo build --release`.

Server: Minecraft 26.3, `online-mode=false` in server.properties.
Run: `target/release/speedrun_bot --server localhost:25565 --owner <your nick>`.
Chat commands: `!go` whole run, `!stage wood|stone|iron`, `!stop`, `!inv`, `!pos`.

Status: not yet tested on a live server.
