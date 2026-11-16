# Status (2026-11-16, session 2)

## Session 2 summary (lead)

Layout now: repo `Fitertom/haushand`, branch `claude/mc-speedrun-azalea-handoff-o87msf`,
`mc_speedrun/` and `azalea/` side by side (the bot finds `../azalea/azalea`).

Owner's rules (strict): no Azalea interaction helpers — look and packets are ours; nothing
that looks like cheating — every click/dig/hit only on what the eyes see (own raycast),
in reach, after looking at it.

- wood→stone→iron end to end on peaceful: 7025 ticks on seed -613993438115203091.
- `click.rs`: own look (yaw/pitch into LookDirection, sent with the tick's movement packet),
  own raycast vs outline shapes, `sight`/`sees`, `use_item_on` with the server's checks.
- `dig.rs`: own block breaking (seen face, progress counted by the game's rule, server-confirmed).
- `place.rs`: seen faces only; `pillar_up`/`pillar_down`.
- Stations (table, furnace) are carried: picked up after use, placed where needed.
- `portal.rs`: stage `portal` (needs 10 obsidian + flint and steel): site, frame with
  cobblestone corners built from a 3-high pillar, lit, entered — verified, in the nether.
- `nether.rs`: stages `survey`, `fortress`: spiral search for nether bricks, legs walk,
  fortress reached and its blaze spawner found on the test seed. The final walk to the
  spawner spun forever (fixed in `walk_to`, NOT re-run).
- Azalea fix: `open_container_at` no longer takes a stale ContainerSetContent as the open.
- Pathfinder: `world::stop_path` (a force stop cancelled a goto sent right after it);
  `goto_timeout` gives up after 100 ticks without moving.
- dev: `fresh_world.sh [seed] [difficulty] [tickrate]`, MC_PORT/MC_RUN/RCON_PORT/TAG for
  several servers; bot flags `--exit`, `--delay <ticks>`, `--stage a,b,c`.
- `tools/report.py`: HTML report of a run log (see TESTING.md).
- Merged at the end, NOT tested together: `work/combat` (combat.rs, survival.rs: fight,
  eat, hazards, death → resume; idle guard in main.rs) and `work/mining` (mine.rs:
  staircase, strip mining, obsidian casting, stages descend/diamonds/obsidian — last
  commit is the stopped agent's WIP). Their own STATUS notes are below if they wrote any.
  First thing next: build, run `--go` on the test seed, then each new stage alone.

Next: blazes (combat at the spawner), pearls (endermen / bartering), eyes, stronghold,
dragon.

## Before session 2

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
- Telemetry log written per run (`runs/*.jsonl`); report page: `python3 tools/report.py`.

Tested only in **peaceful, permanent day** (set via RCON) — no combat yet.

## In progress when handed off

- A full timed run `--go` on a fresh world (seed -613993438115203091) failed in `wood`: the
  crafting table was made after the pickaxe's planks and ate them. Fixed in `obtain.rs`
  (table first) but NOT re-run yet — rerun `--go` on a fresh world first (TESTING.md).
  Telemetry of that run worked (`runs/*.jsonl`: stages, `story/root` at tick 3812).

## Next, in order

1. [x] **Report page** (`tools/report.py`, see TESTING.md) from `runs/*.jsonl`: splits per stage in game ticks/IGT, top-down path
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
