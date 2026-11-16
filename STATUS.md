# Status (2026-11-16)

## Works (tested live on a local 26.3 server at 60 TPS)

- Join, stay connected, no packet errors, no kicks.
- Bot sees the world correctly (block comparison with the server: 20/20), walks with
  position matching the server's to the hundredth.
- Server tick rate followed by the client (`/tick rate 60` → bot runs 600 ticks / 10 s).
- **Stage `wood`**: find trees (explores if none), mine logs, planks, sticks, crafting table
  (placed), wooden pickaxe.
- **Stage `stone`**: cobblestone, stone pickaxe/axe/sword, spare cobble, coal (optional).
  ~24 s real at ×3.
- **Stage `iron`**: iron ore underground, furnace crafted + placed, smelting, iron pickaxe,
  bucket, iron sword. ~64 s real at ×3.
- Knowledge base + planner (agent-built): `cargo run --example plan -- minecraft:pink_terracotta 4`,
  7 unit tests pass (`cargo test --lib`).
- Telemetry log written per run (`runs/*.jsonl`); report page NOT written yet.

Tested only in **peaceful, permanent day** (set via RCON) — no combat yet.

## In progress when handed off

- A full timed run `--go` (wood→stone→iron) on a fresh world with telemetry was started; its
  result was not read. Rerun it (TESTING.md).

## Next, in order

1. **Report page** from `runs/*.jsonl`: splits per stage in game ticks/IGT, top-down path
   per dimension, hp/food over time, advancements timeline. Owner asked for this.
2. **Wire the planner** (`knowledge.rs`) into `obtain.rs`/`craft.rs`, replacing the hand
   tables; map item id strings ↔ Azalea `ItemKind`.
3. **Survival**: eat (hunt/cook), fight mobs (sword, shield), avoid lava/falls, handle death
   (respawn, resume — stages are idempotent). Then test on easy/normal and with day/night.
4. **Nether**: obsidian (bucket + lava pool, or diamond pickaxe), portal frame building
   (needs block placing by plan — `place.rs` is the base), flint and steel, enter.
   Pathfinder places no blocks: bridging/pillaring must be added (Azalea has none).
5. Fortress (blaze rods ×6–7), ender pearls (barter with piglins or endermen) → eyes of ender.
6. Stronghold by eye throws (triangulate), dig down, fill portal.
7. Dragon: beds (wool) or bow+crystals, melee on perch.
8. Speed: release build, fewer waits, parallel planning.

## Known issues

- Paths to far or hillside trees sometimes time out; logs high in a crown are skipped
  (`height_above_ground`), whole tree not yet blacklisted on failure.
- `obtain` picks mining sources by a hand table (e.g. stone also asks for deepslate);
  the planner should replace this.
- `azalea::pathfinder` warns "channel lagged" and "No best node found" sometimes.
- Debug build only so far.
