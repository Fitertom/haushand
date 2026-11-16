# Speedrun bot — handoff

Read this first. Then `STATUS.md` (where it stands), `TESTING.md` (how to run it against a
local server), `../azalea/PORT_26.3.md` (what was changed in Azalea for Minecraft 26.3).

## What it is

A bot, in Rust, that plays Minecraft Java **26.3** (latest release, protocol 777) on the
owner's own server and tries to **beat the game (kill the Ender Dragon) as fast as
possible**, fully on its own. Time to write it is not limited; progress is measured by stages
and by game ticks (IGT).

- The owner talks Russian. Code, comments, logs, commits: English. Comments say WHY.
- The bot uses **Azalea** (Rust Minecraft bot library). Azalea upstream supports 26.2 at
  best, so this project carries **its own port of Azalea to 26.3** in `../azalea`
  (in this repository next to `mc_speedrun/`; based on upstream `azalea-rs/azalea` main
  `0d38e0d`, see the root `HANDOFF.md`).
  The bot depends on it by path: `azalea = { path = "../azalea/azalea" }`.
- Rust **nightly** (Azalea needs it); `rust-toolchain.toml` says `nightly`. Built and tested
  with `rustc 1.102.0-nightly (2026-11-13)`.

## Layout

| Path | What |
|---|---|
| `src/main.rs` | Args (`--server host:port --name Bot --owner Nick --go --stage <name>`), event handler, chat commands (`!go`, `!stage x`, `!stop`, `!inv`, `!pos`), deadlock detector thread. |
| `src/run.rs` | The run as stages: `wood`, `stone`, `iron` (the `--go` run), then `descend`, `diamonds`, `obsidian`, `portal`, `survey`, `fortress` (run with `--stage a,b,c`). Test stages: `food`, `fight`, `crowd`, `eat`, `hunt`, `lava`, `die`, `guard`, `map`, `walk`, `pathto`, `probe`, `place`, `crafttest`, `tps`, `stand`. Each stage is idempotent: a goal on the inventory or the world. |
| `src/click.rs` | Our own look (yaw/pitch sent with the tick's movement), raycast against outline shapes, `sees`, `use_item_on`: every click only on a face the eyes see, in reach. |
| `src/dig.rs` | Our own block breaking: a seen face, progress by the game's rule, server-confirmed. |
| `src/combat.rs` | Mobs near us (`mobs_near`, `mob`), `fight` (aim, charged hits, creeper/skeleton handling, kill = health 0 or entity gone), `defend` (fight or run), `hunt`, `goto_guarded` (every walk stops to fight). |
| `src/survival.rs` | `tend`: escape hazards (lava, fire), defend, eat, get food; death → respawn → resume the stage. |
| `src/mine.rs` | Underground: staircase down to y -54, strip mining for seen ores, obsidian cast from lava with buckets, portal room. Slow so far (see STATUS). |
| `src/portal.rs` | Nether portal: site, frame (corners from cobblestone, 3-high pillar), light, enter. |
| `src/nether.rs` | Fortress search (spiral of legs), walking in legs, blaze spawner search. |
| `src/obtain.rs` | `obtain(items, n)`: get N of an item by mining (with the right pickaxe tier), smelting (furnace, fuel) or crafting (ingredients first, recursively). Hand-written source table — to be replaced by the planner in `knowledge.rs`. |
| `src/craft.rs` | Crafting by clicking ingredients into the 2×2 or crafting-table grid; waits for the server's answer between clicks (`settle_menu`) and for the result to appear. Hand-written recipe list. |
| `src/place.rs` | Placing a block by the real outline shapes (slabs, bamboo offsets, replaceable grass), checks it won't be inside the bot, aims and verifies the hit. |
| `src/world.rs` | Find blocks, explore when none in view, reach (path with distance-scaled timeout), mine, pick up drops, wait for chunks. |
| `src/inv.rs` | Inventory reads in player-menu numbering **from whatever menu is open** (see pitfalls), hold an item, slot mapping. |
| `src/telemetry.rs` | Run log `runs/<unix>.jsonl`: position/hp/food each 20 ticks, stage start/end, advancements done, deaths. Time = game ticks. |
| `src/knowledge.rs`, `src/lib.rs`, `data/knowledge.json`, `tools/gen_knowledge.py`, `examples/plan.rs` | Knowledge base of how to obtain every item, generated from the 26.3 game data, and a cost-based planner. Not yet wired into `obtain.rs`. See `KNOWLEDGE.md`. |
| `dev/` | Test tooling: `run_bot_test.sh`, `rcon.py`, protocol diff scripts, `tg_send.sh`. |

## Owner-specific rules

- Archives for the owner go to Telegram **only** via `dev/tg_send.sh`, which is fixed to the
  owner's chat (@Oshk1) and checks the username before sending. Never send to any other
  chat the bot knows. The bot token is NOT in this archive (it lived in `/home/user/.tg_token`);
  ask the owner for it, and suggest revoking the old one.
- The owner may interrupt often and ask short questions; answer in Russian, briefly.
