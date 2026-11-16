# Knowledge base and planner

Built by a helper agent; not yet used by the bot.

- `tools/gen_knowledge.py` (stdlib Python) reads the 26.3 data pack
  (`server.jar` → `META-INF/versions/26.3/server-26.3.jar` → `data/minecraft/`), the data
  generator reports (`--reports`: items with default components incl. `minecraft:tool`
  rules), and the Pumpkin extractor's `blocks.json` (hardness, requires-correct-tool) and
  `entities.json` (mob health). Writes `data/knowledge.json` (~2.9 MB, deterministic).
  Paths are hard-coded to `/home/user/mcserver/v26.3/d/...` and
  `/home/user/azalea/codegen/__cache__/...`: regenerate those (see TESTING.md, PORT_26.3.md)
  or edit the paths.
- Holds: all recipes (crafting incl. transmute, cooking, stonecutting, smithing, brewing)
  with tags resolved; block drops (plain / silk touch / shears) and which tools harvest them
  with break times; ore and terrain generation (Y ranges, counts, biomes, dimension);
  structures and their chest loot; mob drops and spawns (26.3 biome
  `gameplay/natural_mob_spawns`); bartering, fishing, trades; a small hand-written section
  (buckets, obsidian from lava, entering the nether/end).
- `src/knowledge.rs`: `Knowledge::load(path)` / `from_json_str`, and
  `plan(item, count, &have) -> Plan` — cheapest tree in estimated seconds, tools planned
  recursively, held tools reused, cycles guarded. Cost constants are rough guesses
  (commented); tune them with real run logs.
- `cargo run --example plan -- minecraft:diamond_pickaxe 1`; `cargo test --lib` (7 tests).
- Item ids are strings (`"minecraft:diamond"`); map to Azalea `ItemKind` when wiring in.
