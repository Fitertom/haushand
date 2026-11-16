# Testing against a local server

Everything here ran in a Linux cloud container (4 cores, 15 GB RAM).

## Server (Minecraft 26.3)

- Needs Java 25: `apt-get install -y openjdk-25-jre-headless` →
  `/usr/lib/jvm/java-25-openjdk-amd64/bin/java`.
- Server jar: version manifest `https://piston-meta.mojang.com/mc/game/version_manifest_v2.json`
  → 26.3 → `downloads.server.url`. Put it in `/home/user/mcserver/run/server.jar`.
- `eula.txt`: `eula=true` (the owner allowed running a local test server).
- `server.properties` (the server rewrites it; edit after first start):
  `online-mode=false`, `white-list=false` (26.3 turns the whitelist ON by default and kicks
  "You are not white-listed"), `enable-rcon=true`, `rcon.password=testpass`,
  `rcon.port=25575`, `difficulty=peaceful` for mechanics tests.
- Start detached (a tool's background job dies after its time limit):
  `cd /home/user/mcserver/run && setsid nohup java -Xmx3G -jar server.jar nogui > server.log 2>&1 < /dev/null &`
- After start, via RCON (`dev/rcon.py "<cmd>" ...`):
  `tick rate 60` (×3 speed; the bot follows), `difficulty peaceful`, `time set day`,
  `gamerule advance_time false` (26.3 name; `doDaylightCycle` no longer exists).
  Useful: `give SpeedBot <item> <n>`, `clear SpeedBot`, `data get entity SpeedBot Pos`,
  `execute if block x y z <block>` (only while the bot keeps the chunk loaded), `tick query`.
- Fresh world: stop the server, rename `world`, start again.

## Bot

- Connect by `127.0.0.1:25565`, not `localhost` (resolves to IPv6, not available here).
- `dev/run_bot_test.sh <seconds> [--stage wood|stone|iron|walk|probe|place|crafttest|tps] [--go]`
  copies the project to `/home/user/bot_build` (without the knowledge files, so another
  agent's half-written code never blocks a test), builds, runs for N seconds, and prints
  errors by kind, the bot's INFO lines and the server log tail. Full log:
  `/tmp/claude-0/bot.clean.log`. `TAILN=40` shows more lines; `RUST_LOG=...` overrides.
- The deadlock detector prints a backtrace when the client freezes (symptom: server log
  "lost connection: Timed out" a minute after joining).

## Pitfalls already hit (do not repeat)

- **Lock order**: never hold Azalea's world lock or a component guard
  (`bot.component::<T>()`) while calling something that takes the ECS lock
  (`bot.position()`, other components). Read what you need first, drop, then lock.
- Azalea's `nearest_entities_by` deadlocks (reads Position while holding the ECS lock); use
  our `world::drops_near`.
- With a container open, the player's slots update in the **container menu**; the
  inventory menu syncs on close. Count via `inv::player_slots`.
- At 60 TPS, a fixed "wait N ticks" is 3× shorter in real time: wait for the server's
  answer instead (menu state id stable, result slot filled, block changed).
- A click on short grass replaces it: the placed block lands one lower. `place.rs` handles
  replaceable blocks.
