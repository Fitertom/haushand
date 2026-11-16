//! What the game says about getting every item, and a planner over it.
//!
//! `data/knowledge.json` is written by `tools/gen_knowledge.py` from the 26.3 server's own
//! data (recipes, loot tables, tags, worldgen, structures, trades, item components). This
//! module reads it and plans: `Knowledge::plan(item, count, have)` gives the cheapest tree of
//! steps (craft, smelt, mine with a tool, kill, loot, barter, trade, a hand-written
//! interaction) from what is held, by an estimate of seconds.
//!
//! Item ids are strings ("minecraft:diamond") so nothing here depends on Azalea. Potions are
//! `<item>#<potion>` ("minecraft:potion#minecraft:awkward"): they differ only by a component.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use serde::Deserialize;

/// Expected count per evaluation (one block broken, one kill, one chest...) and the most.
pub type Yield = BTreeMap<String, [f64; 2]>;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemInfo {
    #[serde(default)]
    pub stack: u32,
    /// Kinds of sources (craft, mine, mob, loot_chests...): a summary for people.
    #[serde(default)]
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolInfo {
    pub kind: String,
    pub tier: Option<String>,
    pub default_speed: f64,
    pub durability: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolUse {
    pub speed: f64,
    /// The tool's `correct_for_drops`: without it a block that requires a tool drops nothing.
    pub correct: bool,
    /// Break time in seconds by the vanilla formula; None for unbreakable blocks.
    pub seconds: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Found {
    pub feature: String,
    pub feature_type: Option<String>,
    pub dimensions: Vec<String>,
    pub biome_count: u32,
    #[serde(default)]
    pub biomes: Vec<String>,
    pub per_chunk: f64,
    /// Expected blocks of this kind a chunk of a listing biome gets from the feature.
    pub blocks_per_chunk: f64,
    pub surface: bool,
    pub y_min: Option<i32>,
    pub y_max: Option<i32>,
    pub y_peak: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerrainAt {
    /// Empty: every biome of the dimension.
    #[serde(default)]
    pub biomes: Vec<String>,
    /// Only where a noise or an ore vein puts it (a raw iron block, a badlands band).
    #[serde(default)]
    pub rare: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlockInfo {
    pub hardness: Option<f64>,
    pub requires_tool: bool,
    pub item: Option<String>,
    pub hand_seconds: Option<f64>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolUse>,
    /// Context ("plain", "silk_touch", "shears") -> expected drops.
    #[serde(default)]
    pub drops: BTreeMap<String, Yield>,
    #[serde(default)]
    pub drop_notes: Vec<String>,
    #[serde(default)]
    pub found: Vec<Found>,
    /// Dimensions where the terrain itself is made of it (stone, netherrack, badlands
    /// terracotta...), with the biomes that have it.
    #[serde(default)]
    pub terrain: BTreeMap<String, TerrainAt>,
    /// Structures whose templates hold it, with the blocks one structure holds on average.
    #[serde(default)]
    pub structures: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MobDrops {
    #[serde(default)]
    pub player: Yield,
    pub not_player: Option<Yield>,
    pub special: Option<Yield>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NaturalSpawn {
    /// The mob's best share of its spawn category's weight in one biome of the dimension.
    pub best_share: f64,
    pub biome_count: u32,
    pub night_only: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Spawn {
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub natural: BTreeMap<String, NaturalSpawn>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub structures: Vec<String>,
    #[serde(default)]
    pub biomes: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MobInfo {
    #[serde(default)]
    pub drops: BTreeMap<String, MobDrops>,
    pub health: Option<f64>,
    #[serde(default)]
    pub drop_notes: Vec<String>,
    pub spawn: Option<Spawn>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LootTable {
    pub kind: String,
    #[serde(rename = "yield", default)]
    pub yields: Yield,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub structures: Vec<String>,
    pub structures_by: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Structure {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub dimensions: Vec<String>,
    pub set: Option<String>,
    pub placement: Option<String>,
    pub spacing: Option<f64>,
    pub separation: Option<f64>,
    pub distance: Option<f64>,
    pub count: Option<f64>,
    /// Share of the spacing cells that hold one (buried treasure: 0.01).
    pub frequency: Option<f64>,
    pub biome_count: Option<u32>,
    /// Generation step: "underground_structures" are not seen from the surface.
    pub step: Option<String>,
    #[serde(default)]
    pub spawn_overrides: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecipeResult {
    pub item: String,
    pub count: u32,
    #[serde(default)]
    pub components: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ingredient {
    /// Any one of these fills the slot(s); a tag already resolved.
    pub items: Vec<String>,
    pub count: u32,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Recipe {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// None: fits the inventory's 2x2 grid.
    pub station: Option<String>,
    pub result: RecipeResult,
    pub ingredients: Vec<Ingredient>,
    pub cook_ticks: Option<f64>,
    pub xp: Option<f64>,
    #[serde(default)]
    pub pattern: Vec<String>,
    pub note: Option<String>,
    #[serde(default)]
    pub self_input: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TradeGives {
    pub item: String,
    pub count: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Trade {
    pub id: String,
    pub profession: String,
    pub level: Option<u32>,
    pub wants: BTreeMap<String, u32>,
    pub gives: TradeGives,
    #[serde(default)]
    pub modified: bool,
    #[serde(default)]
    pub conditional: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Interaction {
    pub id: String,
    pub result: String,
    pub count: u32,
    #[serde(default)]
    pub inputs: BTreeMap<String, u32>,
    #[serde(default)]
    pub returns: BTreeMap<String, u32>,
    #[serde(default)]
    pub tools: Vec<String>,
    pub seconds: f64,
    pub setup_seconds: Option<f64>,
    pub dimension: Option<String>,
    pub mine_block: Option<String>,
    pub entity: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DimAccess {
    #[serde(default)]
    pub needs: BTreeMap<String, u32>,
    #[serde(default)]
    pub tools: Vec<String>,
    pub seconds: f64,
    pub find_structure: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Manual {
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub interactions: Vec<Interaction>,
    #[serde(default)]
    pub dimension_access: BTreeMap<String, DimAccess>,
    #[serde(default)]
    pub chest_aliases: BTreeMap<String, Vec<String>>,
}

/// Everything `data/knowledge.json` holds.
#[derive(Debug, Clone, Deserialize)]
pub struct Knowledge {
    #[serde(default)]
    pub items: BTreeMap<String, ItemInfo>,
    #[serde(default)]
    pub item_tags: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolInfo>,
    /// Item -> burn time in ticks in a furnace.
    #[serde(default)]
    pub fuel: BTreeMap<String, f64>,
    #[serde(default)]
    pub blocks: BTreeMap<String, BlockInfo>,
    #[serde(default)]
    pub mobs: BTreeMap<String, MobInfo>,
    #[serde(default)]
    pub loot_tables: BTreeMap<String, LootTable>,
    #[serde(default)]
    pub structures: BTreeMap<String, Structure>,
    #[serde(default)]
    pub recipes: Vec<Recipe>,
    #[serde(default)]
    pub trades: Vec<Trade>,
    #[serde(default)]
    pub manual: Manual,
    #[serde(default)]
    pub report: serde_json::Value,
    #[serde(skip)]
    index: Index,
}

// ---------------------------------------------------------------------------------------
// Cost model. Every number is seconds of the bot's time, and only has to RANK alternatives
// sensibly; each says why it is what it is.

/// One craft: open the grid, click the ingredients in, take the result — a few window
/// clicks with a server round trip each.
const CRAFT_SECONDS: f64 = 1.5;
/// Walking to the dropped item and to the next block of a vein or tree.
const PICKUP_SECONDS: f64 = 0.5;
/// A tool or station is used again and again; in the per-unit ESTIMATES it is charged a
/// sixteenth, or the first cobblestone would look as dear as the pickaxe and the planner
/// would pick a worse source for it. The tree itself charges a missing tool in full once.
const TOOL_AMORTIZE: f64 = 16.0;
/// Ores and pools are mined in batches: the estimate charges one descent to 8 units.
const BATCH: f64 = 8.0;
/// Sprinting is 5.6 m/s, walking 4.3; the bot jumps, swims and goes round things.
const WALK_SPEED: f64 = 4.3;
/// A route is never straight and the target is not known in advance (search, not travel).
const ROUTE_FACTOR: f64 = 1.5;
/// Crossing a chunk (16 blocks) on the surface, looking at what lies on it.
const SURFACE_SECONDS_PER_CHUNK: f64 = 4.0;
/// Strip mining exposes ~8 blocks of volume per metre dug (the 1x2 tunnel and its walls) at
/// ~0.6 s a metre with a stone or iron pickaxe: ~13 blocks looked at per second.
const STRIP_BLOCKS_PER_SECOND: f64 = 13.0;
/// Descending a staircase: one block down needs ~2 blocks dug and a step.
const DEPTH_SECONDS_PER_BLOCK: f64 = 0.6;
/// The bot starts near sea level.
const START_Y: f64 = 64.0;
/// Below this deepslate gives way to bedrock layers.
const LOWEST_DIG_Y: f64 = -58.0;
/// The shortest search: the block is under the bot's feet (stone, dirt, netherrack).
const MIN_SEARCH_SECONDS: f64 = 0.5;
/// Past an hour a source is effectively "go elsewhere": capping keeps sums finite.
const MAX_SEARCH_SECONDS: f64 = 3600.0;
/// A biome list this long covers most of a dimension; fewer biomes must be found first.
const WIDESPREAD_BIOMES: f64 = 30.0;
/// A terrain block only some noise lays (a raw iron block in an ore vein, a badlands band):
/// there, but not under every step.
const RARE_TERRAIN_SECONDS: f64 = 20.0;
/// Crossing one chunk of the wrong biome while looking for the right one.
const BIOME_SEARCH_SECONDS: f64 = 30.0;
/// In a biome where a mob spawns, a mob of its category comes into view about this often.
const MOB_ENCOUNTER_SECONDS: f64 = 20.0;
/// Monsters spawn only in the dark: by day the bot waits or goes underground.
const NIGHT_FACTOR: f64 = 2.0;
/// Hearts the bot takes off a mob per second: between a fist (1/s) and an iron sword (~9/s).
const DAMAGE_PER_SECOND: f64 = 5.0;
/// Looking through a chest once standing at it.
const CHEST_SECONDS: f64 = 15.0;
/// The bot does not know where a structure is: it explores until one comes into view, which
/// is a spiral over the area, not a straight walk to the nearest.
const STRUCTURE_SEARCH_FACTOR: f64 = 3.0;
/// An underground structure (buried treasure, a mineshaft) shows only where a cave or a
/// tunnel happens to open into it.
const HIDDEN_FACTOR: f64 = 4.0;
/// A structure the data gives no spacing for (a dungeon, the spawn's bonus chest).
const UNKNOWN_STRUCTURE_SECONDS: f64 = 600.0;
/// A piglin examines a gold ingot for 6 s before it throws something.
const BARTER_SECONDS: f64 = 6.0;
/// Finding piglins once in the nether.
const FIND_PIGLIN_SECONDS: f64 = 60.0;
/// A fishing bite comes after 5-30 s, plus casting and reeling.
const FISH_SECONDS: f64 = 25.0;
const SHEAR_SECONDS: f64 = 4.0;
const TRADE_SECONDS: f64 = 20.0;
/// Raising a villager one level means trading with it many times.
const VILLAGER_LEVEL_SECONDS: f64 = 300.0;
/// A wandering trader appears every 20-60 minutes near the player.
const WANDERING_TRADER_SECONDS: f64 = 1800.0;
/// A source whose plan came out over SLACK x the next source's estimate + SLACK_SECONDS is
/// compared with that next one: estimates within half again are noise, not a reason to
/// double the search.
const SLACK: f64 = 1.5;
const SLACK_SECONDS: f64 = 30.0;
/// Calls of `obtain` one plan may make: a guard against a combinatorial walk.
const NODE_BUDGET: usize = 20000;

const INF: f64 = f64::INFINITY;

fn ceil_div(n: i64, per: f64) -> i64 {
    if per <= 0.0 { i64::MAX / 4 } else { ((n as f64) / per - 1e-9).ceil().max(1.0) as i64 }
}

// ---------------------------------------------------------------------------------------
// Index: every way to get each item.

#[derive(Debug, Clone)]
enum Src {
    Recipe(usize),
    /// Break a world block; `shears` when the drop needs shears.
    Mine { block: String, shears: bool, per: f64 },
    Kill { mob: String, per: f64 },
    Chest { table: String, per: f64 },
    Barter { per: f64 },
    Fish { per: f64 },
    Shear { mob: String, per: f64 },
    Trade(usize),
    Manual(usize),
}

#[derive(Debug, Clone, Default)]
struct Index {
    by_item: HashMap<String, Vec<Src>>,
    est: HashMap<String, f64>,
}

/// One step of a plan. `cost` includes the children.
#[derive(Debug, Clone)]
pub struct Step {
    pub action: Action,
    pub item: String,
    pub count: i64,
    /// Seconds of this step alone.
    pub own_cost: f64,
    /// Seconds of the whole subtree.
    pub cost: f64,
    pub children: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Already in the inventory.
    Have,
    /// A tool or station already held / placed: used, not consumed.
    UseHeld,
    Craft { recipe: String, station: Option<String>, crafts: i64 },
    /// Smelting, blasting, smoking, campfire, brewing: anything with a cook time.
    Cook { recipe: String, station: String, batches: i64 },
    Stonecut { recipe: String, crafts: i64 },
    Smith { recipe: String, crafts: i64 },
    Mine { block: String, tool: Option<String>, blocks: i64, dimension: String },
    Kill { mob: String, kills: i64 },
    Loot { table: String, structure: String, chests: i64 },
    Barter { barters: i64 },
    Fish { catches: i64 },
    Shear { mob: String, times: i64 },
    Trade { trade: String, times: i64 },
    Manual { id: String, times: i64 },
    /// Building and entering a portal to another dimension.
    Travel { dimension: String },
    /// Fuel for a furnace step.
    Fuel,
    Unobtainable { reason: String },
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub root: Step,
    pub total_cost: f64,
    /// Items the plan could not get, with why.
    pub unobtainable: Vec<(String, String)>,
    /// What the inventory holds after the plan: surplus of crafts, tools made.
    pub leftover: BTreeMap<String, i64>,
}

impl Plan {
    pub fn ok(&self) -> bool {
        self.unobtainable.is_empty()
    }

    /// The plan as an indented tree, a line a step.
    pub fn render(&self) -> String {
        let mut s = String::new();
        render_step(&self.root, 0, &mut s);
        if !self.unobtainable.is_empty() {
            let _ = writeln!(s, "unobtainable:");
            for (i, why) in &self.unobtainable {
                let _ = writeln!(s, "  {i}: {why}");
            }
        }
        s
    }
}

fn short(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

fn fmt_secs(s: f64) -> String {
    if !s.is_finite() { "inf".into() } else if s >= 3600.0 { format!("{:.1}h", s / 3600.0) } else if s >= 60.0 { format!("{:.1}m", s / 60.0) } else { format!("{s:.1}s") }
}

fn render_step(st: &Step, depth: usize, out: &mut String) {
    let what = match &st.action {
        Action::Have => "have".to_string(),
        Action::UseHeld => "use held".to_string(),
        Action::Craft { recipe, station, crafts } => format!(
            "craft x{crafts} [{}]{}",
            short(recipe),
            station.as_ref().map(|s| format!(" at {}", short(s))).unwrap_or_default()
        ),
        Action::Cook { recipe, station, batches } => format!("cook x{batches} in {} [{}]", short(station), short(recipe)),
        Action::Stonecut { recipe, crafts } => format!("stonecut x{crafts} [{}]", short(recipe)),
        Action::Smith { recipe, crafts } => format!("smith x{crafts} [{}]", short(recipe)),
        Action::Mine { block, tool, blocks, dimension } => format!(
            "mine {blocks} x {} with {} ({dimension})",
            short(block),
            tool.as_deref().map(short).unwrap_or("hand")
        ),
        Action::Kill { mob, kills } => format!("kill {kills} x {}", short(mob)),
        Action::Loot { table, structure, chests } => format!("loot {chests} chest(s) {} in {}", short(table), short(structure)),
        Action::Barter { barters } => format!("barter {barters} gold with piglins"),
        Action::Fish { catches } => format!("fish {catches} times"),
        Action::Shear { mob, times } => format!("shear {} {times} times", short(mob)),
        Action::Trade { trade, times } => format!("trade x{times} [{}]", short(trade)),
        Action::Manual { id, times } => format!("do x{times} [{id}] (hand-written)"),
        Action::Travel { dimension } => format!("go to the {dimension}"),
        Action::Fuel => "fuel".to_string(),
        Action::Unobtainable { reason } => format!("UNOBTAINABLE: {reason}"),
    };
    let _ = writeln!(
        out,
        "{:indent$}{} x{} <- {}  [{} total, {} own]",
        "",
        short(&st.item),
        st.count,
        what,
        fmt_secs(st.cost),
        fmt_secs(st.own_cost),
        indent = depth * 2
    );
    for c in &st.children {
        render_step(c, depth + 1, out);
    }
}

impl Knowledge {
    /// Reads `data/knowledge.json` (or any file the generator wrote).
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Knowledge, String> {
        let text = std::fs::read_to_string(path.as_ref()).map_err(|e| format!("{}: {e}", path.as_ref().display()))?;
        Knowledge::from_json_str(&text)
    }

    /// From the file's text: `Knowledge::from_json_str(include_str!("../data/knowledge.json"))`.
    pub fn from_json_str(text: &str) -> Result<Knowledge, String> {
        let mut k: Knowledge = serde_json::from_str(text).map_err(|e| e.to_string())?;
        k.build_index();
        Ok(k)
    }

    /// Members of an item tag ("minecraft:planks" or "#minecraft:planks").
    pub fn tag(&self, tag: &str) -> &[String] {
        let t = tag.trim_start_matches('#');
        self.item_tags.get(t).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Tools whose `correct_for_drops` holds for the block, fastest first.
    pub fn harvest_tools(&self, block: &str) -> Vec<(&str, f64)> {
        let Some(b) = self.blocks.get(block) else { return vec![] };
        let mut v: Vec<(&str, f64)> = b.tools.iter().filter(|(_, u)| u.correct).map(|(t, u)| (t.as_str(), u.seconds.unwrap_or(INF))).collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(b.0)));
        v
    }

    /// Seconds to break `block` with `tool` (None: by hand), and whether it drops.
    pub fn break_time(&self, block: &str, tool: Option<&str>) -> Option<(f64, bool)> {
        let b = self.blocks.get(block)?;
        match tool.and_then(|t| b.tools.get(t)) {
            Some(u) => Some((u.seconds?, u.correct || !b.requires_tool)),
            None => Some((b.hand_seconds?, !b.requires_tool)),
        }
    }

    /// The best tool of `have` for `block`: one that makes it drop, the fastest.
    pub fn best_held_tool<'a>(&self, block: &str, have: impl IntoIterator<Item = &'a String>) -> Option<String> {
        let b = self.blocks.get(block)?;
        have.into_iter()
            .filter_map(|t| b.tools.get(t).map(|u| (t, u)))
            .filter(|(_, u)| u.correct || !b.requires_tool)
            .filter(|(_, u)| u.seconds.is_some())
            .min_by(|a, b| a.1.seconds.unwrap().total_cmp(&b.1.seconds.unwrap()).then(a.0.cmp(b.0)))
            .map(|(t, _)| t.clone())
    }

    /// Estimated seconds per unit with nothing held; infinite when no source reaches it.
    pub fn estimate(&self, item: &str) -> f64 {
        self.index.est.get(item).copied().unwrap_or(INF)
    }

    /// Items with no way to get them in survival by this data (spawn eggs, command blocks...).
    pub fn unobtainable_items(&self) -> Vec<String> {
        self.items.keys().filter(|i| *i != "minecraft:air" && !self.estimate(i).is_finite()).cloned().collect()
    }

    fn build_index(&mut self) {
        let mut by: HashMap<String, Vec<Src>> = HashMap::new();
        for (i, r) in self.recipes.iter().enumerate() {
            if !r.self_input {
                by.entry(r.result.item.clone()).or_default().push(Src::Recipe(i));
            }
        }
        for (bid, b) in &self.blocks {
            // Only blocks the world generates can be mined for drops; a crafted block the
            // bot would place first is no source of anything.
            if b.found.is_empty() && b.terrain.is_empty() && b.structures.is_empty() {
                continue;
            }
            for (ctx, shears) in [("plain", false), ("shears", true)] {
                if let Some(y) = b.drops.get(ctx) {
                    for (item, v) in y {
                        if v[0] > 0.0 {
                            by.entry(item.clone()).or_default().push(Src::Mine { block: bid.clone(), shears, per: v[0] });
                        }
                    }
                }
            }
        }
        for (mid, m) in &self.mobs {
            if m.spawn.is_none() {
                continue;
            }
            let mut best: BTreeMap<&String, f64> = BTreeMap::new();
            for d in m.drops.values() {
                for (item, v) in &d.player {
                    let e = best.entry(item).or_insert(0.0);
                    *e = e.max(v[0]);
                }
            }
            for (item, per) in best {
                if per > 0.0 {
                    by.entry(item.clone()).or_default().push(Src::Kill { mob: mid.clone(), per });
                }
            }
        }
        for (tid, t) in &self.loot_tables {
            let path = tid.strip_prefix("minecraft:").unwrap_or(tid);
            for (item, v) in &t.yields {
                if v[0] <= 0.0 {
                    continue;
                }
                let src = match t.kind.as_str() {
                    "chests" if !t.structures.is_empty() => Some(Src::Chest { table: tid.clone(), per: v[0] }),
                    "gameplay" if path == "gameplay/piglin_bartering" => Some(Src::Barter { per: v[0] }),
                    "gameplay" if path == "gameplay/fishing" => Some(Src::Fish { per: v[0] }),
                    "shearing" => {
                        let mob = format!("minecraft:{}", path.split('/').nth(1).unwrap_or(""));
                        self.mobs.get(&mob).filter(|m| m.spawn.is_some()).map(|_| Src::Shear { mob, per: v[0] })
                    }
                    _ => None,
                };
                if let Some(s) = src {
                    by.entry(item.clone()).or_default().push(s);
                }
            }
        }
        for (i, t) in self.trades.iter().enumerate() {
            if !t.conditional {
                by.entry(t.gives.item.clone()).or_default().push(Src::Trade(i));
            }
        }
        for (i, m) in self.manual.interactions.iter().enumerate() {
            by.entry(m.result.clone()).or_default().push(Src::Manual(i));
        }
        self.index.by_item = by;
        self.compute_estimates();
    }

    // --- estimates ------------------------------------------------------------------------

    /// Per-unit cost of every item with nothing held, by relaxation (Bellman-Ford): a cycle
    /// (planks <-> logs, ingots <-> blocks) never lowers a cost, so it converges, and an item
    /// reachable only through a cycle stays infinite.
    fn compute_estimates(&mut self) {
        let items: Vec<String> = self.index.by_item.keys().cloned().collect();
        let mut est: HashMap<String, f64> = HashMap::new();
        for _ in 0..200 {
            let mut changed = false;
            for it in &items {
                let best = self.index.by_item[it]
                    .iter()
                    .map(|s| self.src_unit_est(s, it, &est))
                    .fold(INF, f64::min);
                let old = est.get(it).copied().unwrap_or(INF);
                if best < old - 1e-9 {
                    est.insert(it.clone(), best);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        self.index.est = est;
    }

    fn e(est: &HashMap<String, f64>, item: &str) -> f64 {
        est.get(item).copied().unwrap_or(INF)
    }

    fn cheapest_option(est: &HashMap<String, f64>, ing: &Ingredient) -> f64 {
        ing.items.iter().map(|i| Self::e(est, i)).fold(INF, f64::min)
    }

    /// Fuel per furnace-equivalent tick: the cheapest burnable's cost over its burn time.
    fn fuel_cost_per_tick(&self, est: &HashMap<String, f64>) -> f64 {
        self.fuel.iter().map(|(i, ticks)| Self::e(est, i) / ticks.max(1.0)).fold(INF, f64::min)
    }

    /// Ticks of furnace fuel a cooking step burns: a blast furnace and a smoker cook twice as
    /// fast and burn twice as fast, so per item they burn what a furnace does.
    fn fuel_ticks(r: &Recipe) -> f64 {
        match r.kind.as_str() {
            "minecraft:smelting" => r.cook_ticks.unwrap_or(200.0),
            "minecraft:blasting" | "minecraft:smoking" => 2.0 * r.cook_ticks.unwrap_or(100.0),
            _ => 0.0,
        }
    }

    fn dim_est(&self, dim: &str, est: &HashMap<String, f64>) -> f64 {
        if dim == "overworld" {
            return 0.0;
        }
        let Some(a) = self.manual.dimension_access.get(dim) else { return INF };
        let mut c = a.seconds;
        for (i, n) in &a.needs {
            c += *n as f64 * Self::e(est, i);
        }
        for t in &a.tools {
            c += Self::e(est, t);
        }
        if let Some(s) = &a.find_structure {
            c += self.structure_find(s);
        }
        c
    }

    /// Seconds to find a structure of a kind: one per spacing x spacing chunks at a random
    /// place, so the nearest is about half a cell off. Rarer by every share that thins them:
    /// structures sharing a set (taken as 1/n of it), cells left empty (`frequency`), biomes
    /// they may stand in. A density thinned by q puts the nearest 1/sqrt(q) further away.
    pub fn structure_find(&self, sid: &str) -> f64 {
        let Some(s) = self.structures.get(sid) else { return UNKNOWN_STRUCTURE_SECONDS };
        let blocks = match s.placement.as_deref() {
            Some("minecraft:random_spread") => {
                let share = s.set.as_ref().map(|set| self.structures.values().filter(|o| o.set.as_ref() == Some(set)).count().max(1) as f64).unwrap_or(1.0);
                let freq = s.frequency.unwrap_or(1.0).clamp(1e-4, 1.0);
                let coverage = s.biome_count.map(|n| (n as f64 / WIDESPREAD_BIOMES).clamp(0.05, 1.0)).unwrap_or(1.0);
                s.spacing.unwrap_or(32.0).max(1.0) * 16.0 * 0.5 * (share / freq / coverage).sqrt()
            }
            // Strongholds: the first ring is about 4 x `distance` chunks from the origin, and
            // eyes of ender point the way, so it is a walk, not a search.
            Some("minecraft:concentric_rings") => return 4.0 * s.distance.unwrap_or(32.0) * 16.0 / WALK_SPEED * ROUTE_FACTOR,
            _ => return UNKNOWN_STRUCTURE_SECONDS,
        };
        let hidden = if s.step.as_deref() == Some("underground_structures") { HIDDEN_FACTOR } else { 1.0 };
        blocks / WALK_SPEED * ROUTE_FACTOR * STRUCTURE_SEARCH_FACTOR * hidden
    }

    fn structure_dim(&self, sid: &str) -> String {
        self.structures.get(sid).and_then(|s| s.dimensions.first().cloned()).unwrap_or_else(|| "overworld".into())
    }

    /// Where to dig a block: (search seconds per block, one-time seconds, dimension), the
    /// cheapest way for `n` blocks.
    /// `dim_cost` is what getting into a dimension costs the caller: gravel lies in the
    /// nether too, and without it a tie sent the plan through a portal for flint.
    fn block_search(&self, block: &str, n: f64, dim_cost: &dyn Fn(&str) -> f64) -> Option<(f64, f64, String)> {
        let b = self.blocks.get(block)?;
        let mut best: Option<(f64, f64, String, f64)> = None;
        let mut consider = |per: f64, once: f64, dim: String| {
            let per = per.clamp(MIN_SEARCH_SECONDS, MAX_SEARCH_SECONDS);
            let total = per * n + once + dim_cost(&dim);
            if best.as_ref().map_or(true, |x| total < x.3) {
                best = Some((per, once, dim, total));
            }
        };
        for (d, t) in &b.terrain {
            let once = if t.biomes.is_empty() { 0.0 } else { Self::biome_search(t.biomes.len() as f64) };
            consider(if t.rare { RARE_TERRAIN_SECONDS } else { MIN_SEARCH_SECONDS }, once, d.clone());
        }
        // Biomes that have the block by any feature: oak logs come from a dozen per-biome tree
        // features of one biome each, and counted one by one every forest looked rare.
        let mut biomes_by_dim: HashMap<&str, u32> = HashMap::new();
        for f in &b.found {
            for d in &f.dimensions {
                *biomes_by_dim.entry(d.as_str()).or_insert(0) += f.biome_count;
            }
        }
        for f in &b.found {
            // A block found in few biomes needs its biome found first.
            let listed = f.dimensions.iter().map(|d| biomes_by_dim.get(d.as_str()).copied().unwrap_or(0)).max().unwrap_or(f.biome_count);
            let biome = Self::biome_search(listed as f64);
            let d = f.blocks_per_chunk;
            if d <= 0.0 {
                continue;
            }
            let dim = if f.dimensions.iter().any(|x| x == "overworld") { "overworld".to_string() } else { f.dimensions.first().cloned().unwrap_or_default() };
            match (f.surface, f.y_min, f.y_max) {
                (false, Some(lo), Some(hi)) => {
                    // Hidden in rock: found by strip mining at the best height. A trapezoid's
                    // peak holds about twice its mean density.
                    let span = (hi - lo).max(1) as f64;
                    let peak = if f.y_peak.is_some() { 2.0 } else { 1.0 };
                    let per = 256.0 * span / (STRIP_BLOCKS_PER_SECOND * d * peak);
                    let target = f.y_peak.map(|y| y as f64).unwrap_or((lo + hi) as f64 / 2.0).max(LOWEST_DIG_Y);
                    let once = if dim == "overworld" { (START_Y - target).max(0.0) * DEPTH_SECONDS_PER_BLOCK } else { 0.0 };
                    consider(per, once + biome, dim);
                }
                _ => consider(SURFACE_SECONDS_PER_CHUNK / d, biome, dim),
            }
        }
        for (sid, per_structure) in &b.structures {
            if *per_structure > 0.0 {
                // The first structure is found once; past what one holds, another each time.
                let find = self.structure_find(sid);
                consider(find / per_structure, find, self.structure_dim(sid));
            }
        }
        best.map(|(p, o, d, _)| (p, o, d))
    }

    /// Finding one of `listed` biomes: a biome in one of thirty takes crossing ~30 chunks'
    /// worth of others.
    fn biome_search(listed: f64) -> f64 {
        let coverage = (listed / WIDESPREAD_BIOMES).clamp(1.0 / WIDESPREAD_BIOMES, 1.0);
        (1.0 / coverage - 1.0) * BIOME_SEARCH_SECONDS
    }

    /// Seconds to meet one mob of a kind, and the dimension it is met in.
    /// `n` meetings are wanted; `dim_cost` is what reaching a dimension costs the caller.
    fn mob_find(&self, mob: &str, n: f64, dim_cost: &dyn Fn(&str) -> f64) -> Option<(f64, String)> {
        let sp = self.mobs.get(mob)?.spawn.as_ref()?;
        let mut best: Option<(f64, String, f64)> = None;
        for (dim, ns) in &sp.natural {
            if ns.best_share <= 0.0 {
                continue;
            }
            let coverage = (ns.biome_count as f64 / WIDESPREAD_BIOMES).clamp(0.05, 1.0);
            let s = MOB_ENCOUNTER_SECONDS / ns.best_share / coverage * if ns.night_only { NIGHT_FACTOR } else { 1.0 };
            let s = s.min(MAX_SEARCH_SECONDS);
            let total = s * n + dim_cost(dim);
            if best.as_ref().map_or(true, |x| total < x.2) {
                best = Some((s, dim.clone(), total));
            }
        }
        for sid in &sp.structures {
            // The structure is found once; its spawner area then gives mobs steadily.
            let find = self.structure_find(sid);
            let d = self.structure_dim(sid);
            let total = find + MOB_ENCOUNTER_SECONDS * n + dim_cost(&d);
            if best.as_ref().map_or(true, |x| total < x.2) {
                best = Some((find / n.max(1.0) + MOB_ENCOUNTER_SECONDS, d, total));
            }
        }
        best.map(|(s, d, _)| (s, d))
    }

    fn fight(&self, mob: &str) -> f64 {
        self.mobs.get(mob).and_then(|m| m.health).unwrap_or(20.0) / DAMAGE_PER_SECOND
    }

    /// The cheapest way to break a block `n` times in the estimates: (seconds, tool share).
    fn mine_tool_est(&self, block: &str, shears: bool, n: f64, est: &HashMap<String, f64>) -> f64 {
        let Some(b) = self.blocks.get(block) else { return INF };
        let mut best = INF;
        if !shears && !b.requires_tool {
            if let Some(h) = b.hand_seconds {
                best = h * n;
            }
        }
        for (t, u) in &b.tools {
            let fits = if shears { t == "minecraft:shears" } else { u.correct || !b.requires_tool };
            if fits {
                if let Some(s) = u.seconds {
                    best = best.min(s * n + Self::e(est, t) / TOOL_AMORTIZE);
                }
            }
        }
        if shears && !b.tools.contains_key("minecraft:shears") {
            // Shears that are no faster than a hand on it: still needed for the drop.
            best = best.min(b.hand_seconds.unwrap_or(INF) * n + Self::e(est, "minecraft:shears") / TOOL_AMORTIZE);
        }
        best
    }

    fn src_unit_est(&self, s: &Src, item: &str, est: &HashMap<String, f64>) -> f64 {
        match s {
            Src::Recipe(i) => {
                let r = &self.recipes[*i];
                let mut c = 0.0;
                for ing in &r.ingredients {
                    c += ing.count as f64 * Self::cheapest_option(est, ing);
                }
                let station = r.station.as_deref().map(|st| Self::e(est, st) / TOOL_AMORTIZE).unwrap_or(0.0);
                let action = match r.cook_ticks {
                    Some(t) => t / 20.0 + missing_cost(Self::fuel_ticks(r), self.fuel_cost_per_tick(est)),
                    None => CRAFT_SECONDS,
                };
                (c + action) / r.result.count.max(1) as f64 + station
            }
            Src::Mine { block, shears, per } => {
                let Some((search, once, dim)) = self.block_search(block, BATCH, &|d| self.dim_est(d, est)) else { return INF };
                let tool = self.mine_tool_est(block, *shears, 1.0, est);
                (search + PICKUP_SECONDS + tool + once / BATCH) / per + self.dim_est(&dim, est)
            }
            Src::Kill { mob, per } => {
                let Some((find, dim)) = self.mob_find(mob, 1.0, &|d| self.dim_est(d, est)) else { return INF };
                (find + self.fight(mob)) / per + self.dim_est(&dim, est)
            }
            Src::Chest { table, per } => {
                let t = &self.loot_tables[table];
                t.structures
                    .iter()
                    .map(|sid| (self.structure_find(sid) + CHEST_SECONDS) / per + self.dim_est(&self.structure_dim(sid), est))
                    .fold(INF, f64::min)
            }
            Src::Barter { per } => {
                (BARTER_SECONDS + Self::e(est, "minecraft:gold_ingot")) / per + FIND_PIGLIN_SECONDS / BATCH + self.dim_est("nether", est)
            }
            Src::Fish { per } => FISH_SECONDS / per + Self::e(est, "minecraft:fishing_rod") / TOOL_AMORTIZE,
            Src::Shear { mob, per } => {
                let Some((find, dim)) = self.mob_find(mob, 1.0, &|d| self.dim_est(d, est)) else { return INF };
                (SHEAR_SECONDS + find) / per + Self::e(est, "minecraft:shears") / TOOL_AMORTIZE + self.dim_est(&dim, est)
            }
            Src::Trade(i) => {
                let t = &self.trades[*i];
                let mut c = TRADE_SECONDS;
                for (w, n) in &t.wants {
                    c += *n as f64 * Self::e(est, w);
                }
                (c + self.villager_find(t) / BATCH) / t.gives.count.max(1) as f64
            }
            Src::Manual(i) => {
                let m = &self.manual.interactions[*i];
                let _ = item;
                let mut c = m.seconds + m.setup_seconds.unwrap_or(0.0) / BATCH;
                for (w, n) in &m.inputs {
                    c += *n as f64 * Self::e(est, w);
                }
                for t in &m.tools {
                    c += Self::e(est, t) / TOOL_AMORTIZE;
                }
                if let Some(b) = &m.mine_block {
                    c += self.mine_tool_est(b, false, 1.0, est);
                }
                if let Some(e) = &m.entity {
                    c += self.mob_find(e, 1.0, &|d| self.dim_est(d, est)).map(|x| x.0).unwrap_or(INF);
                }
                c / m.count.max(1) as f64 + m.dimension.as_deref().map(|d| self.dim_est(d, est)).unwrap_or(0.0)
            }
        }
    }

    /// A villager of the trade's profession and level: a village, plus levelling it.
    fn villager_find(&self, t: &Trade) -> f64 {
        if t.profession == "wandering_trader" {
            return WANDERING_TRADER_SECONDS;
        }
        let village = self.structures.keys().filter(|s| s.starts_with("minecraft:village_")).map(|s| self.structure_find(s)).fold(INF, f64::min);
        village + (t.level.unwrap_or(1).saturating_sub(1)) as f64 * VILLAGER_LEVEL_SECONDS
    }

    // --- planning -------------------------------------------------------------------------

    /// The cheapest tree of steps to hold `count` of `item`, starting from `have`.
    pub fn plan<K: AsRef<str>>(&self, item: &str, count: i32, have: &HashMap<K, i32>) -> Plan {
        let mut p = Planner {
            k: self,
            inv: have.iter().filter(|(_, n)| **n > 0).map(|(k, n)| (k.as_ref().to_string(), *n as i64)).collect(),
            placed: BTreeSet::new(),
            dims: ["overworld".to_string()].into_iter().collect(),
            visited: BTreeSet::new(),
            stack: Vec::new(),
            nodes: 0,
        };
        let root = p.obtain(item, count.max(0) as i64);
        let mut unobtainable = Vec::new();
        collect_unobtainable(&root, &mut unobtainable);
        Plan { total_cost: root.cost, root, unobtainable, leftover: p.inv.into_iter().filter(|(_, n)| *n > 0).collect() }
    }
}

fn collect_unobtainable(s: &Step, out: &mut Vec<(String, String)>) {
    if let Action::Unobtainable { reason } = &s.action {
        if !out.iter().any(|(i, r)| i == &s.item && r == reason) {
            out.push((s.item.clone(), reason.clone()));
        }
    }
    for c in &s.children {
        collect_unobtainable(c, out);
    }
}

fn failed(s: &Step) -> bool {
    matches!(s.action, Action::Unobtainable { .. }) || s.children.iter().any(failed)
}

#[derive(Clone)]
struct Saved {
    inv: BTreeMap<String, i64>,
    placed: BTreeSet<String>,
    dims: BTreeSet<String>,
    visited: BTreeSet<String>,
}

struct Planner<'a> {
    k: &'a Knowledge,
    /// What is held now, as the plan goes: consumed by steps, grown by their surplus.
    inv: BTreeMap<String, i64>,
    /// Stations already made and set down (a crafting table is not used up).
    placed: BTreeSet<String>,
    /// Dimensions the plan has already opened a way to.
    dims: BTreeSet<String>,
    /// Blocks whose place (the biome, the depth, the structure) the plan already went to.
    visited: BTreeSet<String>,
    /// Items being expanded: a source that needs one of them again is a cycle.
    stack: Vec<String>,
    nodes: usize,
}

impl<'a> Planner<'a> {
    fn save(&self) -> Saved {
        Saved { inv: self.inv.clone(), placed: self.placed.clone(), dims: self.dims.clone(), visited: self.visited.clone() }
    }

    fn restore(&mut self, s: Saved) {
        self.inv = s.inv;
        self.placed = s.placed;
        self.dims = s.dims;
        self.visited = s.visited;
    }

    /// Reaching a dimension from here: nothing once the plan has a way in.
    fn dim_cost(&self, d: &str) -> f64 {
        if self.dims.contains(d) { 0.0 } else { self.k.dim_est(d, &self.k.index.est) }
    }

    fn held(&self, item: &str) -> i64 {
        self.inv.get(item).copied().unwrap_or(0)
    }

    fn add(&mut self, item: &str, n: i64) {
        if n > 0 {
            *self.inv.entry(item.to_string()).or_insert(0) += n;
        }
    }

    fn take(&mut self, item: &str, n: i64) -> i64 {
        let h = self.held(item);
        let t = h.min(n);
        if t > 0 {
            self.inv.insert(item.to_string(), h - t);
        }
        t
    }

    fn est(&self, item: &str) -> f64 {
        self.k.estimate(item)
    }

    fn fail(item: &str, count: i64, reason: impl Into<String>) -> Step {
        Step { action: Action::Unobtainable { reason: reason.into() }, item: item.to_string(), count, own_cost: 0.0, cost: INF, children: vec![] }
    }

    fn leaf(action: Action, item: &str, count: i64) -> Step {
        Step { action, item: item.to_string(), count, own_cost: 0.0, cost: 0.0, children: vec![] }
    }

    /// Hold `count` more of `item`: from the inventory first, the rest by the cheapest source.
    fn obtain(&mut self, item: &str, count: i64) -> Step {
        self.nodes += 1;
        if self.nodes > NODE_BUDGET {
            return Self::fail(item, count, "search budget exhausted");
        }
        let from_inv = self.take(item, count);
        let need = count - from_inv;
        if need <= 0 {
            return Self::leaf(Action::Have, item, count);
        }
        if self.stack.iter().any(|s| s == item) {
            return Self::fail(item, need, "cycle: needed to make itself");
        }
        if self.stack.len() > 48 {
            return Self::fail(item, need, "too deep");
        }
        let srcs = self.k.index.by_item.get(item).cloned().unwrap_or_default();
        if srcs.is_empty() {
            return Self::fail(item, need, "no source in survival data");
        }
        let mut ranked: Vec<(f64, usize)> = srcs.iter().enumerate().map(|(i, s)| (self.rank(s, item, need), i)).filter(|(c, _)| c.is_finite()).collect();
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        if ranked.is_empty() {
            return Self::fail(item, need, "every source needs something unobtainable");
        }
        self.stack.push(item.to_string());
        // The estimates assume nothing is on the way; inside a plan a source can turn out
        // dearer (its furnace is the very thing being made) or fail (a cycle through a tool).
        // So the next source is expanded too when the best came out well over the next one's
        // rank, and the cheaper kept. A handful of tries keeps the walk from exploding.
        let mut best: Option<(Step, Saved)> = None;
        let mut first_fail = None;
        let start = self.save();
        for (idx, &(_, i)) in ranked.iter().enumerate().take(6) {
            if let Some((b, _)) = &best {
                if b.cost <= ranked[idx].0 * SLACK + SLACK_SECONDS {
                    break;
                }
            }
            self.restore(start.clone());
            let st = self.expand(&srcs[i], item, need);
            if failed(&st) {
                first_fail.get_or_insert(st);
                continue;
            }
            if best.as_ref().map_or(true, |(b, _)| st.cost < b.cost) {
                best = Some((st, self.save()));
            }
        }
        self.stack.pop();
        let st = match best {
            Some((st, after)) => {
                self.restore(after);
                st
            }
            None => {
                self.restore(start);
                first_fail.unwrap()
            }
        };
        if from_inv > 0 {
            let have = Self::leaf(Action::Have, item, from_inv);
            let cost = st.cost;
            return Step { action: Action::Have, item: item.to_string(), count, own_cost: 0.0, cost, children: vec![have, st] };
        }
        st
    }

    /// Estimated seconds for `n` of `item` by `s`, with what is held counted as free.
    fn rank(&self, s: &Src, item: &str, n: i64) -> f64 {
        let k = self.k;
        let est = &k.index.est;
        let need = |it: &str, cnt: f64| -> f64 { missing_cost(cnt - self.held(it) as f64, self.est(it)) };
        let dim = |d: &str| if self.dims.contains(d) { 0.0 } else { k.dim_est(d, est) };
        let tool = |t: &str| {
            if self.held(t) > 0 {
                0.0
            } else if self.stack.iter().any(|s| s == t) {
                INF
            } else {
                self.est(t)
            }
        };
        match s {
            Src::Recipe(i) => {
                let r = &k.recipes[*i];
                if r.ingredients.iter().any(|ing| ing.items.iter().all(|o| self.stack.iter().any(|s| s == o))) {
                    return INF;
                }
                let crafts = ceil_div(n, r.result.count as f64) as f64;
                let mut c = 0.0;
                for ing in &r.ingredients {
                    let want = ing.count as f64 * crafts;
                    c += ing.items.iter().map(|o| need(o, want)).fold(INF, f64::min);
                }
                if let Some(st) = &r.station {
                    if self.stack.iter().any(|s| s == st) && !self.placed.contains(st) && self.held(st) == 0 {
                        return INF;
                    }
                    if !self.placed.contains(st) {
                        c += tool(st);
                    }
                }
                c + match r.cook_ticks {
                    Some(t) => crafts * (t / 20.0) + missing_cost(crafts * Knowledge::fuel_ticks(r), k.fuel_cost_per_tick(est)),
                    None => crafts * CRAFT_SECONDS,
                }
            }
            Src::Mine { block, shears, per } => {
                let blocks = ceil_div(n, *per) as f64;
                let Some((search, once, d)) = k.block_search(block, blocks, &dim) else { return INF };
                let (_, t) = self.pick_tool(block, *shears, blocks);
                let once = if self.visited.contains(block) { 0.0 } else { once };
                once + blocks * (search + PICKUP_SECONDS) + t + dim(&d)
            }
            Src::Kill { mob, per } => {
                let Some((find, d)) = k.mob_find(mob, ceil_div(n, *per) as f64, &dim) else { return INF };
                ceil_div(n, *per) as f64 * (find + k.fight(mob)) + dim(&d)
            }
            Src::Chest { table, per } => {
                let t = &k.loot_tables[table];
                let chests = ceil_div(n, *per) as f64;
                t.structures.iter().map(|sid| chests * (k.structure_find(sid) + CHEST_SECONDS) + dim(&k.structure_dim(sid))).fold(INF, f64::min)
            }
            Src::Barter { per } => {
                let b = ceil_div(n, *per) as f64;
                FIND_PIGLIN_SECONDS + b * BARTER_SECONDS + need("minecraft:gold_ingot", b) + dim("nether")
            }
            Src::Fish { per } => ceil_div(n, *per) as f64 * FISH_SECONDS + tool("minecraft:fishing_rod"),
            Src::Shear { mob, per } => {
                let Some((find, d)) = k.mob_find(mob, ceil_div(n, *per) as f64, &dim) else { return INF };
                ceil_div(n, *per) as f64 * (SHEAR_SECONDS + find) + tool("minecraft:shears") + dim(&d)
            }
            Src::Trade(i) => {
                let t = &k.trades[*i];
                let times = ceil_div(n, t.gives.count as f64) as f64;
                let mut c = k.villager_find(t) + times * TRADE_SECONDS;
                for (w, m) in &t.wants {
                    c += need(w, *m as f64 * times);
                }
                c
            }
            Src::Manual(i) => {
                let m = &k.manual.interactions[*i];
                let times = ceil_div(n, m.count as f64) as f64;
                let mut c = m.setup_seconds.unwrap_or(0.0) + times * m.seconds;
                for (w, cnt) in &m.inputs {
                    c += need(w, *cnt as f64 * times);
                }
                for t in &m.tools {
                    c += tool(t);
                }
                if let Some(b) = &m.mine_block {
                    c += self.pick_tool(b, false, times).1;
                }
                if let Some(e) = &m.entity {
                    c += k.mob_find(e, 1.0, &dim).map(|x| x.0).unwrap_or(INF);
                }
                let _ = item;
                c + m.dimension.as_deref().map(dim).unwrap_or(0.0)
            }
        }
    }

    /// The tool to break `block` `n` times with, and its cost: breaking time plus, for a tool
    /// not held, its estimate. A held tool that works is free, so a diamond pickaxe in the
    /// inventory is used for stone rather than a wooden one being made.
    fn pick_tool(&self, block: &str, shears: bool, n: f64) -> (Option<String>, f64) {
        self.tool_options(block, shears, n).into_iter().next().unwrap_or((None, INF))
    }

    /// Every way to break `block` `n` times (None: by hand), cheapest first.
    fn tool_options(&self, block: &str, shears: bool, n: f64) -> Vec<(Option<String>, f64)> {
        let k = self.k;
        let Some(b) = k.blocks.get(block) else { return vec![] };
        let mut out: Vec<(Option<String>, f64)> = Vec::new();
        if !shears && !b.requires_tool {
            if let Some(h) = b.hand_seconds {
                out.push((None, h * n));
            }
        }
        let mut cands: Vec<(&String, f64)> = Vec::new();
        for (t, u) in &b.tools {
            let fits = if shears { t == "minecraft:shears" } else { u.correct || !b.requires_tool };
            if fits {
                if let Some(s) = u.seconds {
                    cands.push((t, s));
                }
            }
        }
        if shears && !b.tools.contains_key("minecraft:shears") {
            // Shears no faster than a hand on it: still needed for the drop.
            if let Some(t) = k.tools.keys().find(|t| *t == "minecraft:shears") {
                cands.push((t, b.hand_seconds.unwrap_or(INF)));
            }
        }
        for (t, s) in cands {
            // A tool being made right now cannot be used to make itself.
            let acquire = if self.held(t) > 0 { 0.0 } else if self.stack.iter().any(|x| x == t) { INF } else { self.est(t) };
            let c = s * n + acquire;
            if c.is_finite() {
                out.push((Some(t.clone()), c));
            }
        }
        out.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        out
    }

    /// Settle the tool for breaking `block` `n` times: the cheapest by estimate, the next
    /// tried too when that one's plan came out far dearer (as `obtain` does for sources).
    fn choose_tool(&mut self, block: &str, shears: bool, n: f64) -> Result<(Option<String>, Option<Step>), String> {
        let opts = self.tool_options(block, shears, n);
        if opts.is_empty() {
            return Err(format!("no tool can harvest {block}"));
        }
        let start = self.save();
        let mut best: Option<(f64, Option<String>, Option<Step>, Saved)> = None;
        for (idx, (tool, _)) in opts.iter().enumerate().take(4) {
            if let Some((c, ..)) = &best {
                if *c <= opts[idx].1 * SLACK + SLACK_SECONDS {
                    break;
                }
            }
            self.restore(start.clone());
            let brk = self.k.break_time(block, tool.as_deref()).map(|x| x.0).unwrap_or(INF) * n;
            let (cost, step) = match tool {
                None => (brk, None),
                Some(t) => {
                    let st = self.ensure_tool(t);
                    if failed(&st) {
                        continue;
                    }
                    (brk + st.cost, Some(st))
                }
            };
            if best.as_ref().map_or(true, |(c, ..)| cost < *c) {
                best = Some((cost, tool.clone(), step, self.save()));
            }
        }
        match best {
            Some((_, tool, step, after)) => {
                self.restore(after);
                Ok((tool, step))
            }
            None => {
                self.restore(start);
                Err(format!("no tool for {block} can be made here"))
            }
        }
    }

    /// A tool kept after use: held -> used; else obtained and put back as held.
    fn ensure_tool(&mut self, tool: &str) -> Step {
        if self.held(tool) > 0 {
            return Self::leaf(Action::UseHeld, tool, 1);
        }
        let st = self.obtain(tool, 1);
        if !failed(&st) {
            self.add(tool, 1);
        }
        st
    }

    /// A station set down once and used for the rest of the plan.
    fn ensure_station(&mut self, station: &str) -> Option<Step> {
        if self.placed.contains(station) {
            return None;
        }
        let st = if self.held(station) > 0 { Self::leaf(Action::UseHeld, station, 1) } else { self.obtain(station, 1) };
        if !failed(&st) {
            self.placed.insert(station.to_string());
        }
        Some(st)
    }

    /// The way into a dimension, built once.
    fn ensure_dim(&mut self, dim: &str) -> Option<Step> {
        if self.dims.contains(dim) {
            return None;
        }
        let k = self.k;
        let Some(a) = k.manual.dimension_access.get(dim) else {
            return Some(Self::fail(dim, 1, "no known way into this dimension"));
        };
        // Building the way may itself need the other dimension (the end's eyes need blaze
        // rods from the nether): that is planned inside, before this one is marked open.
        let mut children = Vec::new();
        for (i, n) in &a.needs {
            children.push(self.obtain(i, *n as i64));
        }
        for t in &a.tools {
            children.push(self.ensure_tool(t));
        }
        let mut own = a.seconds;
        if let Some(s) = &a.find_structure {
            own += k.structure_find(s);
        }
        self.dims.insert(dim.to_string());
        Some(sum(Action::Travel { dimension: dim.to_string() }, &format!("dimension:{dim}"), 1, own, children))
    }

    fn pick_option<'b>(&self, ing: &'b Ingredient, want: i64) -> &'b str {
        // Enough of one option held: use it; else the cheapest to get, not on the stack.
        if let Some(o) = ing.items.iter().filter(|o| self.held(o) >= want).max_by_key(|o| self.held(o)) {
            return o;
        }
        ing.items
            .iter()
            .filter(|o| !self.stack.iter().any(|s| s == *o))
            .min_by(|a, b| {
                let ca = missing_cost((want - self.held(a)) as f64, self.est(a));
                let cb = missing_cost((want - self.held(b)) as f64, self.est(b));
                ca.total_cmp(&cb).then(a.cmp(b))
            })
            .map(|s| s.as_str())
            .unwrap_or(ing.items.first().map(|s| s.as_str()).unwrap_or(""))
    }

    fn expand(&mut self, s: &Src, item: &str, n: i64) -> Step {
        let k = self.k;
        match s {
            Src::Recipe(i) => {
                let r = &k.recipes[*i];
                let crafts = ceil_div(n, r.result.count as f64);
                let mut children = Vec::new();
                if let Some(st) = &r.station {
                    if let Some(c) = self.ensure_station(st) {
                        children.push(c);
                    }
                }
                for ing in &r.ingredients {
                    let want = ing.count as i64 * crafts;
                    let opt = self.pick_option(ing, want).to_string();
                    children.push(self.obtain(&opt, want));
                }
                let (action, own) = match (r.kind.as_str(), r.cook_ticks) {
                    ("minecraft:stonecutting", _) => (Action::Stonecut { recipe: r.id.clone(), crafts }, crafts as f64 * CRAFT_SECONDS),
                    ("minecraft:smithing_transform", _) => (Action::Smith { recipe: r.id.clone(), crafts }, crafts as f64 * CRAFT_SECONDS),
                    (_, Some(t)) => {
                        let ft = Knowledge::fuel_ticks(r) * crafts as f64;
                        if ft > 0.0 {
                            children.push(self.fuel(ft));
                        }
                        (Action::Cook { recipe: r.id.clone(), station: r.station.clone().unwrap_or_default(), batches: crafts }, crafts as f64 * t / 20.0)
                    }
                    _ => (Action::Craft { recipe: r.id.clone(), station: r.station.clone(), crafts }, crafts as f64 * CRAFT_SECONDS),
                };
                self.add(item, crafts * r.result.count as i64 - n);
                sum(action, item, n, own, children)
            }
            Src::Mine { block, shears, per } => {
                let blocks = ceil_div(n, *per);
                let Some((search, once, dim)) = k.block_search(block, blocks as f64, &|d| self.dim_cost(d)) else { return Self::fail(item, n, "block not generated") };
                let mut children = Vec::new();
                if let Some(t) = self.ensure_dim(&dim) {
                    children.push(t);
                }
                let tool = match self.choose_tool(block, *shears, blocks as f64) {
                    Ok((tool, step)) => {
                        children.extend(step);
                        tool
                    }
                    Err(why) => return Self::fail(item, n, why),
                };
                let brk = k.break_time(block, tool.as_deref()).map(|x| x.0).unwrap_or(INF);
                // The descent or the biome found for this block is paid once a plan: the
                // second log comes from the same forest.
                let once = if self.visited.insert(block.clone()) { once } else { 0.0 };
                let own = once + blocks as f64 * (search + PICKUP_SECONDS + brk);
                let got = ((blocks as f64) * per).floor() as i64;
                self.add(item, got - n);
                sum(Action::Mine { block: block.clone(), tool, blocks, dimension: dim }, item, n, own, children)
            }
            Src::Kill { mob, per } => {
                let kills = ceil_div(n, *per);
                let Some((find, dim)) = k.mob_find(mob, kills as f64, &|d| self.dim_cost(d)) else { return Self::fail(item, n, "mob does not spawn") };
                let children = self.ensure_dim(&dim).into_iter().collect();
                sum(Action::Kill { mob: mob.clone(), kills }, item, n, kills as f64 * (find + k.fight(mob)), children)
            }
            Src::Chest { table, per } => {
                let t = &k.loot_tables[table];
                let chests = ceil_div(n, *per);
                let Some(sid) = t.structures.iter().min_by(|a, b| {
                    let ca = k.structure_find(a) + if self.dims.contains(&k.structure_dim(a)) { 0.0 } else { k.dim_est(&k.structure_dim(a), &k.index.est) };
                    let cb = k.structure_find(b) + if self.dims.contains(&k.structure_dim(b)) { 0.0 } else { k.dim_est(&k.structure_dim(b), &k.index.est) };
                    ca.total_cmp(&cb).then(a.cmp(b))
                }) else {
                    return Self::fail(item, n, "chest of no known structure");
                };
                let children = self.ensure_dim(&k.structure_dim(sid)).into_iter().collect();
                let own = chests as f64 * (k.structure_find(sid) + CHEST_SECONDS);
                sum(Action::Loot { table: table.clone(), structure: sid.clone(), chests }, item, n, own, children)
            }
            Src::Barter { per } => {
                let barters = ceil_div(n, *per);
                let mut children: Vec<Step> = self.ensure_dim("nether").into_iter().collect();
                children.push(self.obtain("minecraft:gold_ingot", barters));
                sum(Action::Barter { barters }, item, n, FIND_PIGLIN_SECONDS + barters as f64 * BARTER_SECONDS, children)
            }
            Src::Fish { per } => {
                let catches = ceil_div(n, *per);
                let children = vec![self.ensure_tool("minecraft:fishing_rod")];
                sum(Action::Fish { catches }, item, n, catches as f64 * FISH_SECONDS, children)
            }
            Src::Shear { mob, per } => {
                let times = ceil_div(n, *per);
                let Some((find, dim)) = k.mob_find(mob, times as f64, &|d| self.dim_cost(d)) else { return Self::fail(item, n, "mob does not spawn") };
                let mut children: Vec<Step> = self.ensure_dim(&dim).into_iter().collect();
                children.push(self.ensure_tool("minecraft:shears"));
                sum(Action::Shear { mob: mob.clone(), times }, item, n, times as f64 * (SHEAR_SECONDS + find), children)
            }
            Src::Trade(i) => {
                let t = &k.trades[*i];
                let times = ceil_div(n, t.gives.count as f64);
                let mut children = Vec::new();
                for (w, m) in &t.wants {
                    children.push(self.obtain(w, *m as i64 * times));
                }
                self.add(item, times * t.gives.count as i64 - n);
                let own = k.villager_find(t) + times as f64 * TRADE_SECONDS;
                sum(Action::Trade { trade: t.id.clone(), times }, item, n, own, children)
            }
            Src::Manual(i) => {
                let m = &k.manual.interactions[*i];
                let times = ceil_div(n, m.count as f64);
                let mut children = Vec::new();
                if let Some(d) = &m.dimension {
                    if let Some(t) = self.ensure_dim(d) {
                        children.push(t);
                    }
                }
                for (w, cnt) in &m.inputs {
                    children.push(self.obtain(w, *cnt as i64 * times));
                }
                for t in &m.tools {
                    children.push(self.ensure_tool(t));
                }
                let mut own = m.setup_seconds.unwrap_or(0.0) + times as f64 * m.seconds;
                if let Some(b) = &m.mine_block {
                    let tool = match self.choose_tool(b, false, times as f64) {
                        Ok((tool, step)) => {
                            children.extend(step);
                            tool
                        }
                        Err(why) => return Self::fail(item, n, why),
                    };
                    own += times as f64 * k.break_time(b, tool.as_deref()).map(|x| x.0).unwrap_or(INF);
                }
                if let Some(e) = &m.entity {
                    own += k.mob_find(e, 1.0, &|d| self.dim_cost(d)).map(|x| x.0).unwrap_or(INF);
                }
                for (r, cnt) in &m.returns {
                    self.add(r, *cnt as i64 * times);
                }
                self.add(item, times * m.count as i64 - n);
                sum(Action::Manual { id: m.id.clone(), times }, item, n, own, children)
            }
        }
    }

    /// Fuel for `ticks` of furnace burning: a held fuel that suffices, else the cheapest.
    fn fuel(&mut self, ticks: f64) -> Step {
        let k = self.k;
        let mut best: Option<(&String, i64, f64)> = None;
        for (f, burn) in &k.fuel {
            if self.stack.iter().any(|s| s == f) {
                continue;
            }
            let units = (ticks / burn.max(1.0)).ceil() as i64;
            let c = missing_cost((units - self.held(f)) as f64, self.est(f));
            if c.is_finite() && best.map_or(true, |(bf, _, bc)| c < bc || (c == bc && f < bf)) {
                best = Some((f, units, c));
            }
        }
        let Some((f, units, _)) = best else { return Self::fail("fuel", 1, "no fuel obtainable") };
        let child = self.obtain(f, units);
        sum(Action::Fuel, f, units, 0.0, vec![child])
    }
}

/// What the part not held costs; nothing when all is held, even for an item with no source
/// (0 x infinity would be NaN and rank a held unobtainable item as impossible).
fn missing_cost(missing: f64, unit: f64) -> f64 {
    if missing <= 0.0 { 0.0 } else { missing * unit }
}

fn sum(action: Action, item: &str, count: i64, own: f64, children: Vec<Step>) -> Step {
    let cost = own + children.iter().map(|c| c.cost).sum::<f64>();
    Step { action, item: item.to_string(), count, own_cost: own, cost, children }
}

// ---------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn k() -> &'static Knowledge {
        static K: OnceLock<Knowledge> = OnceLock::new();
        K.get_or_init(|| Knowledge::load(concat!(env!("CARGO_MANIFEST_DIR"), "/data/knowledge.json")).expect("run tools/gen_knowledge.py first"))
    }

    fn inv(pairs: &[(&str, i32)]) -> HashMap<String, i32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn find<'a>(s: &'a Step, f: &dyn Fn(&Step) -> bool) -> Option<&'a Step> {
        if f(s) {
            return Some(s);
        }
        s.children.iter().find_map(|c| find(c, f))
    }

    #[test]
    fn tag_resolution() {
        let planks = k().tag("#minecraft:planks");
        assert!(planks.contains(&"minecraft:oak_planks".to_string()));
        assert!(planks.contains(&"minecraft:crimson_planks".to_string()));
        // A tag of tags is flattened: #logs holds #oak_logs' members.
        assert!(k().tag("minecraft:logs").contains(&"minecraft:stripped_oak_wood".to_string()));
        // The stick recipe keeps the tag's name next to its members.
        let stick = k().recipes.iter().find(|r| r.id == "minecraft:stick").unwrap();
        assert_eq!(stick.ingredients[0].tag.as_deref(), Some("minecraft:planks"));
        assert!(stick.ingredients[0].items.contains(&"minecraft:oak_planks".to_string()));
    }

    #[test]
    fn shaped_recipe() {
        let r = k().recipes.iter().find(|r| r.id == "minecraft:diamond_pickaxe").unwrap();
        assert_eq!(r.kind, "minecraft:crafting_shaped");
        assert_eq!(r.station.as_deref(), Some("minecraft:crafting_table"));
        assert_eq!(r.result.count, 1);
        let dia = r.ingredients.iter().find(|i| i.items.contains(&"minecraft:diamond".to_string())).unwrap();
        assert_eq!(dia.count, 3);
        let stick = r.ingredients.iter().find(|i| i.items == ["minecraft:stick"]).unwrap();
        assert_eq!(stick.count, 2);
        // The crafting table itself fits the 2x2 grid.
        let t = k().recipes.iter().find(|r| r.id == "minecraft:crafting_table").unwrap();
        assert_eq!(t.station, None);
    }

    #[test]
    fn diamond_ore_needs_iron_or_better() {
        let tools: Vec<&str> = k().harvest_tools("minecraft:diamond_ore").into_iter().map(|t| t.0).collect();
        for t in ["minecraft:iron_pickaxe", "minecraft:diamond_pickaxe", "minecraft:netherite_pickaxe"] {
            assert!(tools.contains(&t), "{t} should harvest diamond ore: {tools:?}");
        }
        for t in ["minecraft:wooden_pickaxe", "minecraft:stone_pickaxe", "minecraft:golden_pickaxe", "minecraft:copper_pickaxe"] {
            assert!(!tools.contains(&t), "{t} must not harvest diamond ore");
        }
        assert_eq!(k().break_time("minecraft:diamond_ore", None).map(|x| x.1), Some(false));
        // The plan for a diamond makes an iron pickaxe (or better) on the way.
        let p = k().plan("minecraft:diamond", 1, &inv(&[]));
        let mine = find(&p.root, &|s| matches!(&s.action, Action::Mine { block, .. } if block.contains("diamond_ore"))).expect("mines diamond ore");
        let Action::Mine { tool, .. } = &mine.action else { unreachable!() };
        assert!(tools.contains(&tool.as_deref().unwrap()), "{tool:?}");
    }

    #[test]
    fn smelting_step() {
        let p = k().plan("minecraft:iron_ingot", 3, &inv(&[("minecraft:raw_iron", 3), ("minecraft:furnace", 1), ("minecraft:coal", 1)]));
        assert!(p.ok(), "{}", p.render());
        let cook = find(&p.root, &|s| matches!(s.action, Action::Cook { .. })).expect("a cooking step");
        let Action::Cook { batches, .. } = &cook.action else { unreachable!() };
        assert_eq!(*batches, 3);
        assert!(find(&p.root, &|s| matches!(s.action, Action::Mine { .. })).is_none(), "raw iron was held:\n{}", p.render());
        assert!(find(&p.root, &|s| s.action == Action::Fuel && s.item == "minecraft:coal").is_some(), "{}", p.render());
    }

    #[test]
    fn cycle_guard() {
        // Ingots <-> blocks <-> nuggets, planks <-> nothing: the real data plans in finite time.
        let p = k().plan("minecraft:iron_block", 1, &inv(&[]));
        assert!(p.ok(), "{}", p.render());
        // Two items made only of each other are unobtainable, not a stack overflow.
        let json = r#"{"items": {"t:a": {"stack": 64}, "t:b": {"stack": 64}},
            "recipes": [
              {"id": "t:a", "type": "minecraft:crafting_shapeless", "station": null,
               "result": {"item": "t:a", "count": 1}, "ingredients": [{"items": ["t:b"], "count": 1}]},
              {"id": "t:b", "type": "minecraft:crafting_shapeless", "station": null,
               "result": {"item": "t:b", "count": 1}, "ingredients": [{"items": ["t:a"], "count": 1}]}]}"#;
        let tk = Knowledge::from_json_str(json).unwrap();
        assert!(!tk.estimate("t:a").is_finite());
        let p = tk.plan("t:a", 1, &inv(&[]));
        assert!(!p.ok());
        // With one b held, a is one craft away.
        let p = tk.plan("t:a", 1, &inv(&[("t:b", 1)]));
        assert!(p.ok(), "{}", p.render());
    }

    #[test]
    fn uses_held_better_pickaxe() {
        let p = k().plan("minecraft:cobblestone", 10, &inv(&[("minecraft:diamond_pickaxe", 1)]));
        assert!(p.ok(), "{}", p.render());
        let mine = find(&p.root, &|s| matches!(s.action, Action::Mine { .. })).expect("mines");
        let Action::Mine { tool, .. } = &mine.action else { unreachable!() };
        assert_eq!(tool.as_deref(), Some("minecraft:diamond_pickaxe"));
        assert!(find(&p.root, &|s| matches!(&s.action, Action::Craft { .. })).is_none(), "nothing to craft:\n{}", p.render());
    }

    #[test]
    fn creative_items_are_unobtainable() {
        let p = k().plan("minecraft:command_block", 1, &inv(&[]));
        assert!(!p.ok());
        assert!(k().unobtainable_items().contains(&"minecraft:bedrock".to_string()));
    }
}
