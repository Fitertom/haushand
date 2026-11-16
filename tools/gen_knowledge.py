#!/usr/bin/env python3
"""Build data/knowledge.json: how every item of Minecraft Java 26.3 can be obtained.

Everything below is read from the game's own data (the server jar's data pack and the
data generator's reports), so a new game version is one re-run, not a hand edit. The only
hand-written part is MANUAL (interactions the data does not describe), marked as such in
the output.

Usage: python3 tools/gen_knowledge.py [--data DIR] [--reports DIR] [--pumpkin DIR] [--out FILE]

Sources (defaults):
  data pack      /home/user/mcserver/v26.3/d/data/minecraft
                 recipes, loot tables, tags, worldgen, structures (.nbt), trades, fuel providers
  reports        /home/user/azalea/codegen/__cache__/generated-26.3/reports
                 minecraft/components/item/*.json: every item's default components
                 (max_stack_size, minecraft:tool rules, minecraft:cooking_fuel)
  pumpkin        /home/user/azalea/codegen/__cache__/pumpkin-26.3
                 blocks.json: hardness, requires_correct_tool_for_drops, block -> item id
                 (vanilla's blocks.json report carries neither); entities.json: max_health
"""

import argparse
import glob
import gzip
import io
import json
import math
import os
import struct
import sys

NS = "minecraft:"


def mc(name):
    return name if ":" in name else NS + name


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def rel_id(root, path):
    """`<root>/a/b.json` -> `minecraft:a/b`."""
    r = os.path.relpath(path, root)
    return NS + os.path.splitext(r)[0].replace(os.sep, "/")


def files(root, pattern="**/*.json"):
    return sorted(glob.glob(os.path.join(root, pattern), recursive=True))


class Report:
    """Things the generator met and did not turn into a source, so they are listed in the
    output instead of silently vanishing (a dropped recipe type looked like 'unobtainable')."""

    def __init__(self):
        self.d = {}

    def add(self, kind, key, example=None):
        e = self.d.setdefault(kind, {})
        if key not in e:
            e[key] = {"count": 0, "example": example}
        e[key]["count"] += 1

    def out(self):
        return self.d


REPORT = Report()

# ---------------------------------------------------------------------------------------
# Tags


class Tags:
    def __init__(self, root, kind):
        self.raw = {}
        base = os.path.join(root, "tags", kind)
        for f in files(base):
            self.raw[rel_id(base, f)] = load(f).get("values", [])
        self.cache = {}

    def resolve(self, tag, stack=()):
        tag = mc(tag.lstrip("#"))
        if tag in self.cache:
            return self.cache[tag]
        if tag in stack or tag not in self.raw:
            return []
        out = []
        for v in self.raw[tag]:
            # Entries may be {"id": ..., "required": false}; optional ones that name nothing are skipped.
            if isinstance(v, dict):
                v = v.get("id")
            if v is None:
                continue
            if v.startswith("#"):
                out += self.resolve(v, stack + (tag,))
            else:
                out.append(mc(v))
        seen, uniq = set(), []
        for x in out:
            if x not in seen:
                seen.add(x)
                uniq.append(x)
        self.cache[tag] = uniq
        return uniq

    def all(self):
        return {t: self.resolve(t) for t in sorted(self.raw)}


def members(spec, tags):
    """A recipe ingredient / tool rule 'blocks' value -> (list of ids, tag name or None)."""
    if isinstance(spec, list):
        out, tag = [], None
        for s in spec:
            m, t = members(s, tags)
            out += m
            tag = tag or t
        return out, tag
    if isinstance(spec, dict):  # older shape {"item": ...} / {"tag": ...}
        if "tag" in spec:
            return tags.resolve(spec["tag"]), mc(spec["tag"])
        if "item" in spec:
            return [mc(spec["item"])], None
        if "id" in spec:
            return [mc(spec["id"])], None
        return [], None
    if spec.startswith("#"):
        return tags.resolve(spec), mc(spec[1:])
    return [mc(spec)], None


# ---------------------------------------------------------------------------------------
# Number providers (loot counts, rolls, placement counts)


def expect(p):
    """Expected value of a number provider; None when the shape is unknown."""
    if isinstance(p, (int, float)):
        return float(p)
    if not isinstance(p, dict):
        return None
    t = p.get("type", "minecraft:uniform" if "min" in p else "")
    if t in ("minecraft:constant",):
        return float(p.get("value", 0))
    if t == "minecraft:uniform":
        lo = expect(p.get("min", p.get("min_inclusive", 0)))
        hi = expect(p.get("max", p.get("max_inclusive", 0)))
        return (lo + hi) / 2 if lo is not None and hi is not None else None
    if t == "minecraft:binomial":
        n, q = expect(p.get("n")), expect(p.get("p"))
        return n * q if n is not None and q is not None else None
    if t == "minecraft:weighted_list":
        tot = sum(e["weight"] for e in p["distribution"])
        return sum(expect(e["data"]) * e["weight"] for e in p["distribution"]) / tot if tot else 0.0
    if t in ("minecraft:biased_to_bottom", "minecraft:clamped_normal", "minecraft:trapezoid"):
        lo = expect(p.get("min_inclusive", p.get("min", 0)))
        hi = expect(p.get("max_inclusive", p.get("max", 0)))
        if t == "minecraft:biased_to_bottom":
            return lo + (hi - lo) / 4
        return (lo + hi) / 2
    if t == "minecraft:clamped":
        v = expect(p.get("source"))
        if v is None:
            return None
        return max(p.get("min_inclusive", v), min(p.get("max_inclusive", v), v))
    if t == "minecraft:enchantment_level":
        return 1.0
    REPORT.add("number_provider", t, p)
    return None


def max_of(p):
    if isinstance(p, (int, float)):
        return float(p)
    if not isinstance(p, dict):
        return 0.0
    for k in ("max", "max_inclusive", "n", "value"):
        if k in p:
            v = max_of(p[k])
            return v
    if p.get("type") == "minecraft:weighted_list":
        return max(max_of(e["data"]) for e in p["distribution"])
    return expect(p) or 0.0


# ---------------------------------------------------------------------------------------
# Loot tables
#
# A table is evaluated under a context (what tool/enchantment the bot uses, whether a
# player killed the mob) into the EXPECTED count of each item per evaluation (per block
# broken, per kill, per chest, per barter). Conditions that depend on the context are
# exact; random ones contribute their probability; ones the bot cannot steer are assumed
# as noted at each branch.


class Loot:
    def __init__(self, root):
        self.root = root
        self.tables = {}
        base = os.path.join(root, "loot_table")
        for f in files(base):
            self.tables[rel_id(base, f)] = load(f)
        self.predicates = {}
        pbase = os.path.join(root, "predicate")
        for f in files(pbase):
            self.predicates[rel_id(pbase, f)] = load(f)

    # --- conditions -------------------------------------------------------------------

    def cond_list(self, node):
        """26.3 writes a single `condition` (object or predicate reference); older versions a
        `conditions` list. Both are read so a table of either shape works."""
        c = node.get("condition", node.get("conditions"))
        if c is None:
            return []
        return c if isinstance(c, list) else [c]

    def prob(self, c, ctx, where, notes):
        """Probability that condition `c` holds in context `ctx`.
        where: 'entry' (gates an entry or pool) or 'modifier' (gates a count change)."""
        if isinstance(c, str):
            ref = mc(c)
            if ref == "minecraft:tool/can_silk_touch":
                notes.add("silk_touch")
                return 1.0 if ctx.get("silk_touch") else 0.0
            if ref == "minecraft:tool/can_shear":
                notes.add("shears")
                return 1.0 if ctx.get("shears") else 0.0
            if ref in self.predicates:
                return self.prob(self.predicates[ref], ctx, where, notes)
            REPORT.add("loot_condition", "ref:" + ref)
            return 0.0
        t = c.get("type", c.get("condition"))
        if t == "minecraft:survives_explosion":
            return 1.0
        if t == "minecraft:random_chance":
            return float(expect(c.get("chance")) or 0.0)
        if t == "minecraft:random_chance_with_enchanted_bonus":
            notes.add("looting")
            return float(expect(c.get("unenchanted_chance")) or 0.0)
        if t == "minecraft:table_bonus":
            notes.add("fortune")
            return float(c["chances"][0])
        if t == "minecraft:killed_by_player":
            notes.add("player_kill")
            return 1.0 if ctx.get("player_kill", True) else 0.0
        if t == "minecraft:inverted":
            return 1.0 - self.prob(c["term"], ctx, where, notes)
        if t == "minecraft:any_of":
            q = 1.0
            for x in c["terms"]:
                q *= 1.0 - self.prob(x, ctx, where, notes)
            return 1.0 - q
        if t == "minecraft:all_of":
            q = 1.0
            for x in c["terms"]:
                q *= self.prob(x, ctx, where, notes)
            return q
        if t == "minecraft:match_tool":
            pred = json.dumps(c.get("predicate", {}))
            if "silk_touch" in pred:
                notes.add("silk_touch")
                return 1.0 if ctx.get("silk_touch") else 0.0
            if "shears" in pred:
                notes.add("shears")
                return 1.0 if ctx.get("shears") else 0.0
            notes.add("match_tool:" + pred)
            return 0.0
        if t == "minecraft:match_block":
            # Gating an ENTRY a block state is the "drops at all" case (a door's lower half,
            # a grown crop): the bot breaks the half that drops and harvests grown crops.
            # Gating a COUNT it is a bump (a double slab gives 2): the bot takes the base.
            st = c.get("state", {})
            notes.add("state:" + ",".join("%s=%s" % kv for kv in sorted(st.items())))
            return 1.0 if where == "entry" else 0.0
        if t in ("minecraft:entity_properties", "minecraft:damage_source_properties",
                 "minecraft:location_check", "minecraft:weather_check", "minecraft:time_check",
                 "minecraft:entity_scores", "minecraft:value_check", "minecraft:reference",
                 "minecraft:enchantment_active_check"):
            # Special circumstances (mob on fire, killed by a skeleton's arrow, a biome):
            # the bot does not arrange them, so their drops are not counted on.
            if t == "minecraft:entity_properties" and not c.get("predicate"):
                return 1.0
            notes.add(t.split(":")[1])
            return 1.0 if ctx.get("special") else 0.0
        REPORT.add("loot_condition", t, c)
        return 0.0

    def all_prob(self, node, ctx, where, notes):
        p = 1.0
        for c in self.cond_list(node):
            p *= self.prob(c, ctx, where, notes)
        return p

    # --- entries ----------------------------------------------------------------------

    def modifiers(self, node):
        m = node.get("modifier", node.get("functions"))
        if m is None:
            return []
        return m if isinstance(m, list) else [m]

    def item_count(self, entry, ctx, notes):
        """(expected count, max count) of one item entry after its count modifiers."""
        n, mx = 1.0, 1.0
        state_bumps = False
        for f in self.modifiers(entry):
            if isinstance(f, str):
                continue
            t = f.get("type", f.get("function"))
            if t == "minecraft:set_count":
                p = self.all_prob(f, ctx, "modifier", notes)
                v = expect(f.get("count"))
                if v is None:
                    continue
                vmax = max_of(f.get("count"))
                if f.get("add"):
                    state_bumps = state_bumps or bool(self.cond_list(f))
                    n, mx = n + p * v, mx + (vmax if p > 0 else 0)
                else:
                    n, mx = p * v + (1 - p) * n, (vmax if p > 0 else mx)
            elif t == "minecraft:apply_bonus":
                notes.add("fortune")
            elif t in ("minecraft:enchanted_count_increase", "minecraft:looting_enchant"):
                notes.add("looting")
            elif t == "minecraft:limit_count":
                lim = f.get("limit", {})
                if "max" in lim and isinstance(lim["max"], (int, float)):
                    n, mx = min(n, lim["max"]), min(mx, lim["max"])
            elif t == "minecraft:furnace_smelt":
                notes.add("smelted_if_on_fire")
        if state_bumps and n <= 0:
            # A multiface block (glow lichen, sculk vein) counts its faces by state bumps
            # from a base of 0 or -1; with every bump assumed off it would drop nothing,
            # but a block that exists has at least one face.
            n, mx = 1.0, max(mx, 1.0)
        return max(n, 0.0), max(mx, 0.0)

    def entry(self, e, ctx, notes, depth):
        """-> (probability the entry passes, {item: [expected, max]} given it passes)."""
        t = e.get("type", "minecraft:item")
        p = self.all_prob(e, ctx, "entry", notes)
        if p <= 0:
            return 0.0, {}
        if t == "minecraft:item":
            n, mx = self.item_count(e, ctx, notes)
            return p, {mc(e["name"]): [n, mx]}
        if t == "minecraft:empty":
            return p, {}
        if t == "minecraft:tag":
            items = self.tags.resolve(e["name"] if "name" in e else e["items"])
            n, mx = self.item_count(e, ctx, notes)
            if e.get("expand"):
                # Expanded, each member is its own equally weighted choice.
                k = max(len(items), 1)
                return p, {i: [n / k, mx] for i in items}
            return p, {i: [n, mx] for i in items}
        if t == "minecraft:loot_table":
            v = e.get("value", e.get("name"))
            sub = self.eval(v, ctx, notes, depth + 1) if isinstance(v, (str, dict)) else {}
            return p, sub
        if t == "minecraft:alternatives":
            # First child that passes wins: child i is taken with p_i * prod(1 - p_j<i).
            out, rest = {}, 1.0
            for ch in e.get("children", []):
                q, y = self.entry(ch, ctx, notes, depth)
                for k, (n, mx) in y.items():
                    o = out.setdefault(k, [0.0, 0.0])
                    o[0] += rest * q * n
                    o[1] = max(o[1], mx)
                rest *= 1 - q
            got = 1 - rest
            if got <= 0:
                return 0.0, {}
            return p * got, {k: [v[0] / got, v[1]] for k, v in out.items()}
        if t in ("minecraft:group", "minecraft:sequence"):
            out = {}
            for ch in e.get("children", []):
                q, y = self.entry(ch, ctx, notes, depth)
                for k, (n, mx) in y.items():
                    o = out.setdefault(k, [0.0, 0.0])
                    o[0] += q * n
                    o[1] = max(o[1], mx)
                if t == "minecraft:sequence" and q < 1:
                    break
            return p, out
        if t == "minecraft:dynamic":
            notes.add("dynamic:" + str(e.get("name")))
            return p, {}
        REPORT.add("loot_entry", t, e)
        return 0.0, {}

    def eval(self, table, ctx, notes=None, depth=0):
        """Expected {item: [count, max]} of one evaluation of `table`."""
        notes = notes if notes is not None else set()
        if depth > 8:
            return {}
        d = self.tables.get(mc(table)) if isinstance(table, str) else table
        if d is None:
            return {}
        out = {}
        for pool in d.get("pools", []):
            pp = self.all_prob(pool, ctx, "entry", notes)
            if pp <= 0:
                continue
            rolls = expect(pool.get("rolls", 1)) or 0.0
            rmax = max_of(pool.get("rolls", 1))
            res = []
            for e in pool.get("entries", []):
                q, y = self.entry(e, ctx, notes, depth)
                res.append((e.get("weight", 1) * q, y))
            tot = sum(w for w, _ in res)
            if tot <= 0:
                continue
            for w, y in res:
                share = w / tot if len(res) > 1 else min(w, 1.0)
                for k, (n, mx) in y.items():
                    o = out.setdefault(k, [0.0, 0.0])
                    o[0] += pp * rolls * share * n
                    o[1] += rmax * mx
        return out


def rounded(y):
    return {k: [round(v[0], 4), round(v[1], 2)] for k, v in sorted(y.items()) if v[0] > 0}


# ---------------------------------------------------------------------------------------
# NBT (structure templates are gzipped NBT; parsed here so stdlib is enough)


class Nbt:
    def __init__(self, data):
        self.b = io.BytesIO(data)

    def r(self, fmt):
        n = struct.calcsize(fmt)
        return struct.unpack(">" + fmt, self.b.read(n))[0]

    def string(self):
        n = self.r("H")
        return self.b.read(n).decode("utf-8", "replace")

    def payload(self, t):
        if t == 1:
            return self.r("b")
        if t == 2:
            return self.r("h")
        if t == 3:
            return self.r("i")
        if t == 4:
            return self.r("q")
        if t == 5:
            return self.r("f")
        if t == 6:
            return self.r("d")
        if t == 7:
            n = self.r("i")
            return self.b.read(n)
        if t == 8:
            return self.string()
        if t == 9:
            et, n = self.r("b"), self.r("i")
            return [self.payload(et) for _ in range(n)]
        if t == 10:
            out = {}
            while True:
                ct = self.r("b")
                if ct == 0:
                    return out
                name = self.string()
                out[name] = self.payload(ct)
        if t == 11:
            n = self.r("i")
            return list(struct.unpack(">%di" % n, self.b.read(4 * n)))
        if t == 12:
            n = self.r("i")
            return list(struct.unpack(">%dq" % n, self.b.read(8 * n)))
        raise ValueError("nbt tag %d" % t)

    def root(self):
        t = self.r("b")
        self.string()
        return self.payload(t)


def read_nbt(path):
    raw = open(path, "rb").read()
    if raw[:2] == b"\x1f\x8b":
        raw = gzip.decompress(raw)
    return Nbt(raw).root()


# ---------------------------------------------------------------------------------------
# Worldgen


class Anchor:
    def __init__(self, min_y, height):
        self.min_y, self.top = min_y, min_y + height - 1

    def y(self, a):
        if isinstance(a, (int, float)):
            return int(a)
        if "absolute" in a:
            return int(a["absolute"])
        if "above_bottom" in a:
            return self.min_y + int(a["above_bottom"])
        if "below_top" in a:
            return self.top - int(a["below_top"])
        return self.min_y


def height_range(h, anc):
    """-> (lo, hi, peak or None) of a height provider in absolute Y."""
    t = h.get("type", "minecraft:constant" if "absolute" in h or "above_bottom" in h else "")
    if t == "minecraft:constant":
        y = anc.y(h.get("value", h))
        return y, y, y
    lo = anc.y(h.get("min_inclusive", {"above_bottom": 0}))
    hi = anc.y(h.get("max_inclusive", {"below_top": 0}))
    if t == "minecraft:trapezoid":
        # The distribution's ends may lie outside the world (diamonds: -144..16, peak at
        # -64): the peak is the middle of the unclamped range, the range is then clamped.
        peak = (lo + hi) // 2
        return max(lo, anc.min_y), min(hi, anc.top), max(min(peak, anc.top), anc.min_y)
    lo, hi = max(lo, anc.min_y), min(hi, anc.top)
    if t in ("minecraft:biased_to_bottom", "minecraft:very_biased_to_bottom"):
        return lo, hi, lo
    return lo, hi, None


BLOCK_KEYS_SKIP = {
    "target", "predicate", "predicates", "replaceable_blocks", "can_be_placed_on", "valid_blocks",
    "valid_base_block", "can_replace", "allowed_placement", "continue_through", "input_predicate",
    "if_true", "accepted_neighbors", "muddy_roots_in", "can_place_on", "tag", "below_trunk_provider",
    "dirt_provider", "cannot_replace", "replaceable", "allowed_tree_position", "project_through",
    "blocks", "can_grow_through", "placement",
}


class Worldgen:
    def __init__(self, root, block_ids, biome_tags):
        self.root = os.path.join(root, "worldgen")
        self.block_ids = block_ids
        self.placed = {rel_id(os.path.join(self.root, "placed_feature"), f): load(f)
                       for f in files(os.path.join(self.root, "placed_feature"))}
        self.conf = {rel_id(os.path.join(self.root, "feature"), f): load(f)
                     for f in files(os.path.join(self.root, "feature"))}
        self.biomes = {rel_id(os.path.join(self.root, "biome"), f): load(f)
                       for f in files(os.path.join(self.root, "biome"))}
        dims = {}
        for d, tag in (("overworld", "is_overworld"), ("nether", "is_nether"), ("end", "is_end")):
            for b in biome_tags.resolve(NS + tag):
                dims.setdefault(b, d)
        self.biome_dim = dims
        dt = os.path.join(root, "dimension_type")
        self.anchor = {}
        for d, f in (("overworld", "overworld"), ("nether", "the_nether"), ("end", "the_end")):
            j = load(os.path.join(dt, f + ".json"))
            self.anchor[d] = Anchor(j["min_y"], j["height"])
        self.biome_tags = biome_tags

    def feature_biomes(self):
        """placed feature -> sorted biomes that list it in their generation steps."""
        out = {}
        for b, j in self.biomes.items():
            for step in j.get("features", []):
                for f in step if isinstance(step, list) else [step]:
                    out.setdefault(mc(f), set()).add(b)
        return out

    def blocks_of(self, node, mult, out, stack):
        """Walk a configured feature, adding (block -> weight) for every block it places.
        `mult` carries selector chances so a tree picked one time in a hundred counts so."""
        if isinstance(node, str):
            return
        if isinstance(node, list):
            for x in node:
                self.blocks_of(x, mult, out, stack)
            return
        if not isinstance(node, dict):
            return
        t = node.get("type")
        if "feature" in node and "placement" in node and not t:
            self.follow(node["feature"], mult, out, stack, configured=True)
            return
        if t == "minecraft:random_selector":
            used = 0.0
            for e in node.get("features", []):
                c = float(e.get("chance", 0))
                used += c
                self.follow(e["feature"], mult * c, out, stack)
            if "default" in node:
                self.follow(node["default"], mult * max(0.0, 1 - used), out, stack)
            return
        if t == "minecraft:simple_random_selector":
            fs = node.get("features", [])
            fs = fs if isinstance(fs, list) else [fs]
            for f in fs:
                self.follow(f, mult / max(len(fs), 1), out, stack)
            return
        if t == "minecraft:random_boolean_selector":
            self.follow(node["feature_true"], mult / 2, out, stack)
            self.follow(node["feature_false"], mult / 2, out, stack)
            return
        if t == "minecraft:weighted_random_selector":
            es = node.get("features", node.get("entries", []))
            tot = sum(e.get("weight", 1) for e in es) or 1
            for e in es:
                self.follow(e.get("data", e.get("feature")), mult * e.get("weight", 1) / tot, out, stack)
            return
        # How many blocks one placement puts down, roughly: an ore vein its size, a tree its
        # trunk (~5 logs), anything else one. Only a ranking number for the planner.
        per = 1.0
        if t in ("minecraft:ore", "minecraft:scattered_ore"):
            per = float(node.get("size", 1)) * (1 - 0.5 * float(node.get("discard_chance_on_air_exposure", 0)))
        elif t in ("minecraft:tree", "minecraft:fallen_tree"):
            per = 5.0
        for k, v in node.items():
            if k in BLOCK_KEYS_SKIP:
                continue
            if k in ("feature", "default", "features") and isinstance(v, (str, dict, list)):
                if isinstance(v, list):
                    for x in v:
                        self.follow(x.get("feature", x) if isinstance(x, dict) else x, mult, out, stack)
                else:
                    self.follow(v, mult, out, stack)
                continue
            self.collect(v, k, mult * per, out, stack)

    def collect(self, v, key, w, out, stack):
        if isinstance(v, str):
            if v in self.block_ids and key not in ("type",):
                out[v] = out.get(v, 0.0) + w
            return
        if isinstance(v, list):
            for x in v:
                self.collect(x, key, w, out, stack)
            return
        if isinstance(v, dict):
            if v.get("type") in ("minecraft:weighted", "minecraft:weighted_state_provider") and "entries" in v:
                tot = sum(e.get("weight", 1) for e in v["entries"]) or 1
                for e in v["entries"]:
                    self.collect(e.get("data"), key, w * e.get("weight", 1) / tot, out, stack)
                return
            if "feature" in v and ("placement" in v or "chance" in v):
                self.follow(v["feature"], w, out, stack, configured="placement" in v)
                return
            for k, x in v.items():
                if k in BLOCK_KEYS_SKIP:
                    continue
                self.collect(x, k, w, out, stack)

    def follow(self, ref, mult, out, stack, configured=False):
        if isinstance(ref, dict):
            if "feature" in ref and "placement" in ref:
                self.follow(ref["feature"], mult, out, stack, configured=True)
            else:
                self.blocks_of(ref, mult, out, stack)
            return
        if not isinstance(ref, str):
            return
        ref = mc(ref)
        # A bare string in a selector names a PLACED feature; inside {feature, placement} a
        # configured one. Placed wins when both exist and the caller did not say. The guard
        # keys on the kind too: a placed and a configured feature often share a name
        # (trees_plains), and one key for both cut every tree out of the forests.
        if not configured and ref in self.placed:
            if ("P", ref) in stack:
                return
            self.follow(self.placed[ref]["feature"], mult, out, stack + (("P", ref),), configured=True)
        elif ref in self.conf:
            if ("C", ref) in stack:
                return
            self.blocks_of(self.conf[ref], mult, out, stack + (("C", ref),))

    def placement(self, pf, dim):
        """-> dict of per-chunk count, rarity, height range for a placed feature."""
        count, chance, hr, surface = 1.0, 1.0, None, False
        anc = self.anchor.get(dim, self.anchor["overworld"])
        for m in pf.get("placement", []):
            t = m.get("type")
            if t in ("minecraft:count", "minecraft:count_on_every_layer"):
                c = expect(m.get("count"))
                count *= c if c is not None else 1.0
            elif t == "minecraft:rarity_filter":
                chance /= max(float(m.get("chance", 1)), 1.0)
            elif t in ("minecraft:noise_based_count", "minecraft:noise_threshold_count"):
                vals = [m.get(k) for k in ("noise_to_count_ratio", "below_noise", "above_noise") if isinstance(m.get(k), (int, float))]
                count *= max(sum(vals) / len(vals), 1.0) if vals else 1.0
            elif t == "minecraft:height_range":
                hr = height_range(m["height"], anc)
            elif t == "minecraft:heightmap":
                surface = True
        return count, chance, hr, surface

    def found(self):
        """block -> list of generation records from placed features."""
        fb = self.feature_biomes()
        out = {}
        for pid in sorted(self.placed):
            biomes = sorted(fb.get(pid, ()))
            if not biomes:
                continue  # placed only through another feature: counted there.
            dims = sorted({self.biome_dim.get(b, "overworld") for b in biomes})
            pf = self.placed[pid]
            blocks = {}
            self.follow(pf["feature"], 1.0, blocks, (("P", pid),), configured=True)
            if not blocks:
                continue
            ftype = None
            conf = self.conf.get(mc(pf["feature"])) if isinstance(pf["feature"], str) else pf["feature"]
            if isinstance(conf, dict):
                ftype = conf.get("type")
            count, chance, hr, surface = self.placement(pf, dims[0])
            for b, w in blocks.items():
                rec = {
                    "feature": pid,
                    "feature_type": ftype,
                    "dimensions": dims,
                    "biome_count": len(biomes),
                    "biomes": biomes if len(biomes) <= 12 else biomes[:12] + ["..."],
                    "per_chunk": round(count * chance, 4),
                    "blocks_per_chunk": round(count * chance * w, 4),
                    "surface": surface,
                }
                if hr:
                    rec["y_min"], rec["y_max"] = hr[0], hr[1]
                    if hr[2] is not None:
                        rec["y_peak"] = hr[2]
                out.setdefault(b, []).append(rec)
        return out

    def terrain(self):
        """block -> {dimension: {"biomes": [...] (empty: any biome), "rare": bool}} for blocks
        the noise settings and surface rules lay as terrain. `biomes` comes from enclosing
        `biome` conditions (badlands terracotta, desert sand); `rare` from noise thresholds and
        ore veins (a raw iron block is terrain, but only inside a vein, 2 % of it)."""
        out = {}
        mr = os.path.join(self.root, "material_rule")

        def put(b, dim, biomes, rare):
            e = out.setdefault(b, {}).setdefault(dim, {"any": False, "biomes": set(), "rare": True})
            if biomes is None:
                e["any"] = True
            else:
                e["biomes"] |= set(biomes)
            e["rare"] = e["rare"] and rare

        def walk(v, dim, seen, biomes, rare):
            if isinstance(v, str):
                if v in self.block_ids:
                    put(v, dim, biomes, rare)
                p = os.path.join(mr, v.split(":")[-1] + ".json")
                if v.startswith(NS) and os.path.exists(p) and v not in seen:
                    walk(load(p), dim, seen | {v}, biomes, rare)
            elif isinstance(v, list):
                for x in v:
                    walk(x, dim, seen, biomes, rare)
            elif isinstance(v, dict):
                if v.get("type") == "minecraft:condition":
                    c = v.get("if_true")
                    ct = c.get("type") if isinstance(c, dict) else None
                    b2, r2 = biomes, rare
                    if ct == "minecraft:biome":
                        bi = c.get("biome_is")
                        b2 = [mc(x) for x in (bi if isinstance(bi, list) else [bi])]
                    elif ct == "minecraft:noise_threshold":
                        r2 = True
                    walk(v.get("then_run"), dim, seen, b2, r2)
                    return
                if v.get("type") == "minecraft:ore_vein":
                    rare = True
                for k, x in v.items():
                    if k in ("if_true", "noise", "noise_router", "density", "richness", "filler_gap"):
                        continue
                    walk(x, dim, seen, biomes, rare)

        for name, dim in (("overworld", "overworld"), ("nether", "nether"), ("end", "end")):
            j = load(os.path.join(self.root, "noise_settings", name + ".json"))
            walk(j.get("default_block"), dim, set(), None, False)
            walk(j.get("default_fluid"), dim, set(), None, False)
            walk(j.get("material_rule", j.get("surface_rule")), dim, set(), None, False)
        return {b: {d: {"biomes": [] if e["any"] else sorted(e["biomes"]), "rare": e["rare"]}
                    for d, e in sorted(ds.items())} for b, ds in out.items()}

    def spawns(self):
        """entity -> list of {biome, dimension, category, weight, category_weight, count}."""
        out = {}
        for b, j in sorted(self.biomes.items()):
            attr = j.get("attributes", {}).get("minecraft:gameplay/natural_mob_spawns")
            sp = None
            if attr:
                sp = attr.get("argument", attr).get("spawns_by_category")
            elif "spawners" in j:  # pre-26 shape
                sp = j["spawners"]
            if not sp:
                continue
            for cat, lst in sp.items():
                tot = sum(e.get("weight", 1) for e in lst)
                for e in lst:
                    out.setdefault(mc(e["type"]), []).append({
                        "biome": b,
                        "dimension": self.biome_dim.get(b, "overworld"),
                        "category": cat,
                        "weight": e.get("weight", 1),
                        "category_weight": tot,
                        "count": round(expect(e.get("count", 1)) or 1, 2),
                    })
        return out


# ---------------------------------------------------------------------------------------
# Structures


class Structures:
    def __init__(self, root, biome_tags, biome_dim, block_ids):
        wg = os.path.join(root, "worldgen")
        self.defs = {rel_id(os.path.join(wg, "structure"), f): load(f)
                     for f in files(os.path.join(wg, "structure"))}
        self.sets = {}
        for f in files(os.path.join(wg, "structure_set")):
            j = load(f)
            for s in j.get("structures", []):
                self.sets[mc(s["structure"])] = {"set": rel_id(os.path.join(wg, "structure_set"), f), **j["placement"]}
        self.biome_tags = biome_tags
        self.biome_dim = biome_dim
        self.block_ids = block_ids
        self.tdir = os.path.join(root, "structure")

    def dims(self, sid):
        b = self.defs[sid].get("biomes", [])
        bs, _ = members(b, self.biome_tags) if b else ([], None)
        return sorted({self.biome_dim.get(x, "overworld") for x in bs}) or ["overworld"]

    def owners(self, template):
        """Which structures a template belongs to. Jigsaw structures by their start pool's
        folder (village/plains -> village_plains), the rest by the template's top folder
        matching the structure's name (igloo, shipwreck...)."""
        parts = template.split("/")
        out = []
        for sid, d in self.defs.items():
            sp = d.get("start_pool")
            name = sid.split(":")[1]
            if sp:
                pp = sp.split(":")[1].split("/")
                key = pp[:2] if pp[0] == "village" else pp[:1]
                if parts[:len(key)] == key:
                    out.append(sid)
            elif name.startswith(parts[0]) or parts[0].startswith(name) or \
                    (parts[0] == "nether_fossils" and name == "nether_fossil") or \
                    (parts[0] == "woodland_mansion" and name == "mansion") or \
                    (parts[0] == "underwater_ruin" and name.startswith("ocean_ruin")):
                out.append(sid)
        return sorted(out)

    def scan(self):
        """-> (block -> {structure: expected blocks in one structure}, loot table -> structures).

        Expected count: the block's count summed over the structure's templates, over the
        number of its templates (the mean piece), times the pieces one structure has (a jigsaw
        structure's `size`, else 1). Rough, but it tells a ruined portal's ~10 obsidian
        from a single diamond block somewhere in an ancient city."""
        counts, ntempl, loots = {}, {}, {}
        skip = {NS + x for x in ("air", "structure_void", "jigsaw", "structure_block", "cave_air", "void_air")}
        for f in files(self.tdir, "**/*.nbt"):
            t = os.path.relpath(f, self.tdir)[:-4].replace(os.sep, "/")
            owners = self.owners(t)
            if not owners:
                continue
            try:
                n = read_nbt(f)
            except Exception as e:  # a corrupt template must not stop the whole build
                REPORT.add("structure_nbt", t, str(e))
                continue
            for o in owners:
                ntempl[o] = ntempl.get(o, 0) + 1
            pals = n.get("palette") or (n.get("palettes") or [[]])[0]
            names = [st.get("Name", st.get("id")) for st in pals]  # 26.3 writes `id`, older `Name`
            per = {}
            for bl in n.get("blocks", []):
                si = bl.get("state")
                if isinstance(si, int) and 0 <= si < len(names):
                    b = names[si]
                    if b in self.block_ids and b not in skip:
                        per[b] = per.get(b, 0) + 1
                nb = bl.get("nbt")
                if isinstance(nb, dict) and isinstance(nb.get("LootTable"), str):
                    for o in owners:
                        loots.setdefault(mc(nb["LootTable"]), set()).add(o)
            for b, c in per.items():
                for o in owners:
                    counts.setdefault(b, {}).setdefault(o, 0)
                    counts[b][o] += c
        blocks = {}
        for b, by in counts.items():
            for o, c in by.items():
                pieces = max(int(self.defs[o].get("size", 1)), 1) if self.defs[o].get("type") == "minecraft:jigsaw" else 1
                blocks.setdefault(b, {})[o] = round(c / ntempl[o] * pieces, 3)
        return blocks, {k: sorted(v) for k, v in loots.items()}

    def info(self):
        out = {}
        for sid in sorted(self.defs):
            s = self.sets.get(sid, {})
            out[sid] = {
                "type": self.defs[sid].get("type"),
                "dimensions": self.dims(sid),
                "set": s.get("set"),
                "placement": s.get("type"),
                "spacing": s.get("spacing"),
                "separation": s.get("separation"),
                "distance": s.get("distance"),
                "count": s.get("count"),
                # A share of the cells only get one (buried treasure: 1 %), and only in its biomes.
                "frequency": s.get("frequency", 1.0),
                # Underground structures (buried treasure, mineshafts, trial chambers) are not
                # seen from the surface; the planner searches longer for them.
                "step": self.defs[sid].get("step"),
                "biome_count": len(members(self.defs[sid].get("biomes", []), self.biome_tags)[0]) if self.defs[sid].get("biomes") else None,
                "spawn_overrides": sorted({mc(e["type"]) for cat in self.defs[sid].get("spawn_overrides", {}).values()
                                           for e in cat.get("spawns", [])}),
            }
        return out


# Chest tables whose structure places its chests in code, not in a template's NBT: their
# names do not say the structure's id, so these few are named here (hand-written, small).
CHEST_ALIASES = {
    "nether_bridge": ["minecraft:fortress"],
    "jungle_temple": ["minecraft:jungle_pyramid"],
    "woodland_mansion": ["minecraft:mansion"],
    "underwater_ruin": ["minecraft:ocean_ruin_cold", "minecraft:ocean_ruin_warm"],
    "simple_dungeon": ["feature:minecraft:monster_room"],
    "spawn_bonus_chest": ["bonus_chest"],
    "abandoned_mineshaft": ["minecraft:mineshaft", "minecraft:mineshaft_mesa"],
    "stronghold": ["minecraft:stronghold"],
}


# ---------------------------------------------------------------------------------------
# Hand-written: interactions the data does not describe. Keep short and honest.

MANUAL = [
    {"id": "fill_water_bucket", "result": "minecraft:water_bucket", "count": 1,
     "inputs": {"minecraft:bucket": 1}, "tools": [], "seconds": 10, "dimension": "overworld",
     "note": "Right-click any water source; water is everywhere in the overworld."},
    {"id": "fill_lava_bucket", "result": "minecraft:lava_bucket", "count": 1,
     "inputs": {"minecraft:bucket": 1}, "tools": [], "seconds": 90, "dimension": "overworld",
     "note": "Lava pools (lake_lava features) on the surface or underground; easier in the nether."},
    {"id": "obsidian_from_lava_pool", "result": "minecraft:obsidian", "count": 1,
     "inputs": {}, "tools": ["minecraft:water_bucket"], "mine_block": "minecraft:obsidian",
     "seconds": 8, "setup_seconds": 60, "dimension": "overworld",
     "note": "Pour water over a lava source, mine the obsidian (diamond pickaxe or better)."},
    {"id": "obsidian_from_lava_bucket", "result": "minecraft:obsidian", "count": 1,
     "inputs": {"minecraft:lava_bucket": 1}, "returns": {"minecraft:bucket": 1},
     "tools": ["minecraft:water_bucket"], "mine_block": "minecraft:obsidian", "seconds": 12,
     "dimension": "overworld", "note": "Place lava from a bucket, pour water on it, mine."},
    {"id": "milk_cow", "result": "minecraft:milk_bucket", "count": 1,
     "inputs": {"minecraft:bucket": 1}, "tools": [], "entity": "minecraft:cow", "seconds": 5,
     "note": "Use a bucket on a cow (or goat)."},
    {"id": "sign_book", "result": "minecraft:written_book", "count": 1,
     "inputs": {"minecraft:writable_book": 1}, "tools": [], "seconds": 5,
     "note": "Sign a book and quill."},
    {"id": "fill_glass_bottle", "result": "minecraft:potion#minecraft:water", "count": 1,
     "inputs": {"minecraft:glass_bottle": 1}, "tools": [], "seconds": 10, "dimension": "overworld",
     "note": "Water bottle (a potion item with the water potion component)."},
    {"id": "summon_wither", "result": "minecraft:nether_star", "count": 1,
     "inputs": {"minecraft:soul_sand": 4, "minecraft:wither_skeleton_skull": 3}, "tools": [],
     "seconds": 300, "note": "Build and kill the wither; its loot table gives the nether star."},
]

# Getting into another dimension is a structure the bot builds, not an item: what it costs
# is listed so the planner can plan it like any other step.
DIMENSION_ACCESS = {
    "nether": {"needs": {"minecraft:obsidian": 10}, "tools": ["minecraft:flint_and_steel"], "seconds": 60,
               "note": "Build a 4x5 obsidian frame (10 without corners) and light it."},
    "end": {"needs": {"minecraft:ender_eye": 12}, "tools": [], "seconds": 60,
            "find_structure": "minecraft:stronghold",
            "note": "Find a stronghold (throw eyes, ~2 used up), fill the portal frame (12 eyes worst case; "
                    "each frame block already holds an eye with p=0.1)."},
}


# ---------------------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="/home/user/mcserver/v26.3/d/data/minecraft")
    ap.add_argument("--reports", default="/home/user/azalea/codegen/__cache__/generated-26.3/reports")
    ap.add_argument("--pumpkin", default="/home/user/azalea/codegen/__cache__/pumpkin-26.3")
    ap.add_argument("--out", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "knowledge.json"))
    a = ap.parse_args()
    root = a.data

    item_tags = Tags(root, "item")
    block_tags = Tags(root, "block")
    biome_tags = Tags(root, os.path.join("worldgen", "biome"))
    entity_tags = Tags(root, "entity_type")

    # --- items -------------------------------------------------------------------------
    comp_dir = os.path.join(a.reports, "minecraft", "components", "item")
    items = {}
    item_comp = {}
    for f in files(comp_dir):
        iid = NS + os.path.basename(f)[:-5]
        c = load(f).get("components", {})
        item_comp[iid] = c
        items[iid] = {"stack": c.get("minecraft:max_stack_size", 64)}

    # --- fuel (component minecraft:cooking_fuel, burn time from context_int_provider) --
    def provider(v, kind, seen=()):
        if isinstance(v, (int, float)):
            return v
        if isinstance(v, str):
            p = os.path.join(root, kind, v.split(":")[1] + ".json")
            if v in seen or not os.path.exists(p):
                return None
            return provider(load(p), kind, seen + (v,))
        t = v.get("type")
        if t == "minecraft:div":
            l, r = provider(v["left"], kind, seen), provider(v["right"], kind, seen)
            return l / r if l is not None and r else None
        if t == "minecraft:mul":
            l, r = provider(v["left"], kind, seen), provider(v["right"], kind, seen)
            return l * r if l is not None and r is not None else None
        if t == "minecraft:conditional":
            # A plain furnace: the fast_cooking condition (smoker / blast furnace) is false.
            return provider(v["on_false"], kind, seen)
        if t == "minecraft:constant":
            return provider(v["value"], kind, seen)
        REPORT.add("context_provider", t, v)
        return None

    fuel = {}
    for iid, c in item_comp.items():
        cf = c.get("minecraft:cooking_fuel")
        if cf:
            ticks = provider(cf.get("burn_time"), "context_int_provider")
            if ticks:
                fuel[iid] = int(ticks)

    # --- blocks ------------------------------------------------------------------------
    pk_items = load(os.path.join(a.pumpkin, "items.json"))
    pk_item_by_num = {v["id"]: NS + k for k, v in pk_items.items()}
    pk_blocks = load(os.path.join(a.pumpkin, "blocks.json"))["blocks"]
    vanilla_blocks = set(load(os.path.join(a.reports, "blocks.json")))
    block_ids = set()
    blocks = {}
    for b in pk_blocks:
        bid = NS + b["name"]
        block_ids.add(bid)
        if bid not in vanilla_blocks:
            REPORT.add("pumpkin_block_not_in_vanilla", bid)
        it = pk_item_by_num.get(b.get("item_id"))
        blocks[bid] = {
            "hardness": b.get("hardness"),
            "requires_tool": bool(b.get("requires_correct_tool_for_drops")),
            "item": it if it and it != "minecraft:air" else None,
        }
    for bid in vanilla_blocks - block_ids:
        REPORT.add("vanilla_block_missing_hardness", bid)

    # --- tools ------------------------------------------------------------------------
    TIERS = ["wooden", "golden", "stone", "copper", "iron", "diamond", "netherite"]
    tools = {}
    for iid, c in sorted(item_comp.items()):
        t = c.get("minecraft:tool")
        if not t:
            continue
        rules = []
        for r in t.get("rules", []):
            bs, tag = members(r["blocks"], block_tags)
            rules.append((set(bs), r.get("speed"), r.get("correct_for_drops")))
        name = iid.split(":")[1]
        kind = name.split("_")[-1]
        tier = name.rsplit("_", 1)[0] if "_" in name else None
        tools[iid] = {
            "kind": kind,
            "tier": tier if tier in TIERS else None,
            "default_speed": t.get("default_mining_speed", 1.0),
            "durability": c.get("minecraft:max_damage"),
            "_rules": rules,
        }

    def break_seconds(hardness, speed, harvest):
        if hardness is None or hardness < 0:
            return None
        if hardness == 0:
            return 0.0
        dmg = speed / hardness / (30.0 if harvest else 100.0)
        if dmg >= 1:
            return 0.0
        return math.ceil(1 / dmg) / 20.0

    for bid, b in blocks.items():
        h, req = b["hardness"], b["requires_tool"]
        b["hand_seconds"] = break_seconds(h, 1.0, not req)
        tl = {}
        for tid, t in tools.items():
            speed, correct = None, None
            for bs, s, cfd in t["_rules"]:
                if bid in bs:
                    if speed is None and s is not None:
                        speed = s
                    if correct is None and cfd is not None:
                        correct = cfd
            speed = speed if speed is not None else t["default_speed"]
            correct = bool(correct)
            if speed > 1.0 or correct:
                harvest = correct or not req
                tl[tid] = {"speed": min(speed, 1e9), "correct": correct,
                           "seconds": break_seconds(h, speed, harvest)}
        b["tools"] = dict(sorted(tl.items()))
    for t in tools.values():
        del t["_rules"]

    # --- loot tables -------------------------------------------------------------------
    loot = Loot(root)
    loot.tags = item_tags
    base_ctx = {"player_kill": True}
    for bid, b in blocks.items():
        tid = "minecraft:blocks/" + bid.split(":")[1]
        if tid not in loot.tables:
            b["drops"] = {}
            continue
        notes = set()
        plain = rounded(loot.eval(tid, base_ctx, notes))
        silk = rounded(loot.eval(tid, dict(base_ctx, silk_touch=True), set()))
        shears = rounded(loot.eval(tid, dict(base_ctx, shears=True), set()))
        d = {"plain": plain}
        if silk != plain:
            d["silk_touch"] = silk
        if shears != plain:
            d["shears"] = shears
        b["drops"] = d
        b["drop_notes"] = sorted(n for n in notes if not n.startswith("state:") or "age" in n)

    # Entities: entities/<id>[/<variant>].json
    mobs = {}
    health = {}
    try:
        for k, v in load(os.path.join(a.pumpkin, "entities.json")).items():
            health[NS + k] = v.get("max_health")
    except FileNotFoundError:
        pass
    for tid in sorted(loot.tables):
        if not tid.startswith("minecraft:entities/"):
            continue
        parts = tid.split("/")
        eid = NS + parts[1]
        variant = parts[2] if len(parts) > 2 else None
        notes = set()
        y = rounded(loot.eval(tid, base_ctx, notes))
        y_np = rounded(loot.eval(tid, {"player_kill": False}, set()))
        # Drops behind circumstances the bot does not arrange (a creeper killed by a
        # skeleton's arrow gives a disc), kept apart so they are known but not counted on.
        y_sp = rounded(loot.eval(tid, dict(base_ctx, special=True), set()))
        m = mobs.setdefault(eid, {"drops": {}, "health": health.get(eid)})
        key = variant or "default"
        m["drops"][key] = {"player": y}
        if y_np != y:
            m["drops"][key]["not_player"] = y_np
        if y_sp != y:
            m["drops"][key]["special"] = y_sp
        if notes:
            m.setdefault("drop_notes", [])
            m["drop_notes"] = sorted(set(m["drop_notes"]) | notes)

    # Every other loot table: chests, gameplay (bartering, fishing, cat gifts...), shearing,
    # archaeology, harvest... grouped by its top folder.
    other = {}
    for tid in sorted(loot.tables):
        top = tid.split(":")[1].split("/")[0]
        if top in ("blocks", "entities"):
            continue
        notes = set()
        y = rounded(loot.eval(tid, base_ctx, notes))
        rec = {"kind": top, "yield": y}
        if notes:
            rec["notes"] = sorted(notes)
        other[tid] = rec

    # --- worldgen ------------------------------------------------------------------------
    wg = Worldgen(root, block_ids, biome_tags)
    found = wg.found()
    terrain = wg.terrain()
    spawns = wg.spawns()
    st = Structures(root, biome_tags, wg.biome_dim, block_ids)
    st_blocks, st_loot = st.scan()
    structures = st.info()
    for bid, b in blocks.items():
        if bid in found:
            b["found"] = sorted(found[bid], key=lambda r: (-r["blocks_per_chunk"], r["feature"]))
        if bid in terrain:
            b["terrain"] = terrain[bid]
        if bid in st_blocks:
            b["structures"] = dict(sorted(st_blocks[bid].items()))

    # Structures' own mob spawns (fortress blazes, monument guardians...).
    for sid, s in structures.items():
        for e in s["spawn_overrides"]:
            spawns.setdefault(e, []).append({"structure": sid, "dimension": s["dimensions"][0]})
    for eid, sp in spawns.items():
        m = mobs.setdefault(eid, {"drops": {}, "health": health.get(eid)})
        dims = sorted({x["dimension"] for x in sp})
        # Per dimension: an enderman is the only monster of the end (share 1.0) but a rare one
        # in the overworld; one number for both sent the planner to the overworld at 1.0.
        natural = {}
        for d in dims:
            xs = [x for x in sp if "weight" in x and x["dimension"] == d]
            if xs:
                natural[d] = {
                    "best_share": round(max(x["weight"] / x["category_weight"] for x in xs), 4),
                    "biome_count": len({x["biome"] for x in xs}),
                    "night_only": all(x["category"] == "monster" for x in xs),
                }
        m["spawn"] = {
            "dimensions": dims,
            "natural": natural,
            "categories": sorted({x["category"] for x in sp if "category" in x}),
            "structures": sorted({x["structure"] for x in sp if "structure" in x}),
            "biomes": sorted({x["biome"] for x in sp if "biome" in x})[:12],
        }

    # Chest tables -> structures: from template NBT first, the name second.
    for tid, rec in other.items():
        if rec["kind"] not in ("chests", "barrels", "archaeology", "pots", "spawners", "dispensers"):
            continue
        ss = set(st_loot.get(tid, []))
        how = "template" if ss else None
        if not ss:
            name = tid.split(":")[1].split("/", 1)[1]
            head = name.split("/")[0]
            for k, v in CHEST_ALIASES.items():
                if head.startswith(k):
                    ss |= set(v)
            if not ss:
                for sid in structures:
                    sn = sid.split(":")[1]
                    if head.startswith(sn) or sn.startswith(head.split("_")[0]) and len(head.split("_")[0]) > 3:
                        ss.add(sid)
            how = "name" if ss else None
        rec["structures"] = sorted(ss)
        if how:
            rec["structures_by"] = how

    # --- recipes -------------------------------------------------------------------------
    STATION = {
        "minecraft:smelting": "minecraft:furnace", "minecraft:blasting": "minecraft:blast_furnace",
        "minecraft:smoking": "minecraft:smoker", "minecraft:campfire_cooking": "minecraft:campfire",
        "minecraft:stonecutting": "minecraft:stonecutter", "minecraft:smithing_transform": "minecraft:smithing_table",
    }
    NOT_ACQUISITION = {  # they change an item already held, never make a new one
        "minecraft:crafting_special_bannerduplicate", "minecraft:crafting_special_firework_star_fade",
        "minecraft:crafting_special_mapextending", "minecraft:crafting_special_repairitem",
        "minecraft:crafting_special_shielddecoration", "minecraft:crafting_dye",
        "minecraft:smithing_trim", "minecraft:crafting_special_bookcloning",
    }
    recipes = []
    rdir = os.path.join(root, "recipe")
    handled = {}
    for f in files(rdir):
        rid = rel_id(rdir, f)
        d = load(f)
        t = d["type"]
        ings = []

        def ing(spec, n=1):
            ms, tag = members(spec, item_tags)
            for x in ings:
                if x["items"] == ms:
                    x["count"] += n
                    return
            e = {"items": ms, "count": n}
            if tag:
                e["tag"] = tag
            ings.append(e)

        res = d.get("result") or {}
        rec = {"id": rid, "type": t}
        if t == "minecraft:crafting_shaped":
            pat = d["pattern"]
            for row in pat:
                for ch in row:
                    if ch != " ":
                        ing(d["key"][ch])
            w, h = max(len(r) for r in pat), len(pat)
            rec["station"] = None if w <= 2 and h <= 2 else "minecraft:crafting_table"
            rec["pattern"] = pat
        elif t == "minecraft:crafting_shapeless":
            for x in d["ingredients"]:
                ing(x)
            rec["station"] = None if len(d["ingredients"]) <= 4 else "minecraft:crafting_table"
        elif t == "minecraft:crafting_transmute":
            # material_count may be a range ({min, max}: map cloning takes 1..8 blank maps);
            # the least it takes is what one craft needs.
            mcnt = d.get("material_count", 1)
            mcnt = int(mcnt.get("min", 1)) if isinstance(mcnt, dict) else int(mcnt)
            ing(d["input"])
            ing(d["material"], mcnt)
            rec["station"] = None if 1 + mcnt <= 4 else "minecraft:crafting_table"
            rec["note"] = "transmute: keeps the input's components"
            if d.get("add_material_count_to_result"):
                res = dict(res, count=res.get("count", 1) + mcnt)
        elif t == "minecraft:crafting_imbue":
            ing(d["material"], 8)
            ing(d["source"])
            rec["station"] = "minecraft:crafting_table"
            rec["note"] = "imbue: the result takes the source's potion contents"
        elif t == "minecraft:crafting_decorated_pot":
            for k in ("back", "left", "right", "front"):
                ing(d[k])
            rec["station"] = None
        elif t == "minecraft:crafting_special_firework_rocket":
            ing(d["shell"])
            ing(d["fuel"])
            rec["station"] = None
            rec["note"] = "flight duration 1 (one gunpowder); stars optional"
        elif t == "minecraft:crafting_special_firework_star":
            ing(d["fuel"])
            ing(d["dye"])
            rec["station"] = None
            rec["note"] = "plain star; shape/trail/twinkle optional"
        elif t in STATION:
            if t == "minecraft:smithing_transform":
                ing(d["template"])
                ing(d["base"])
                ing(d["addition"])
            else:
                ing(d["ingredient"])
            rec["station"] = STATION[t]
            if "cookingtime" in d:
                rec["cook_ticks"] = d["cookingtime"]
            elif t in ("minecraft:smelting", "minecraft:blasting", "minecraft:smoking", "minecraft:campfire_cooking"):
                rec["cook_ticks"] = {"minecraft:smelting": 200, "minecraft:blasting": 100,
                                     "minecraft:smoking": 100, "minecraft:campfire_cooking": 600}[t]
            if "experience" in d:
                rec["xp"] = d["experience"]
        elif t == "minecraft:brewing":
            # Potions differ only by a component, so each kind is named `<item>#<potion>`
            # ("minecraft:potion#minecraft:awkward"); a plain item id would make every brew
            # a potion -> potion loop. One reagent brews three bottles.
            def potion_id(spec):
                pc = (spec.get("potion_contents") or {})
                pot = pc.get("potions", pc.get("potion"))
                if isinstance(pot, list):
                    pot = pot[0] if pot else None
                item = mc(spec.get("item", spec.get("id")))
                return item + "#" + mc(pot) if isinstance(pot, str) and not pot.startswith("#") else item
            ings.append({"items": [potion_id(d["input"])], "count": 3})
            rg = d["reagent"]
            ing(rg.get("item", rg.get("id")) if isinstance(rg, dict) else rg)
            out_c = (d["output"].get("components") or {}).get("minecraft:potion_contents", {})
            pot = out_c.get("potion")
            res = {"id": mc(d["output"]["id"]) + ("#" + mc(pot) if pot else ""), "count": 3}
            rec["station"] = "minecraft:brewing_stand"
            rec["cook_ticks"] = 400
            rec["note"] = "brewing stand fuel: one blaze powder per 20 brews"
        elif t in NOT_ACQUISITION:
            REPORT.add("recipe_type_not_acquisition", t, rid)
            continue
        else:
            REPORT.add("recipe_type_unhandled", t, rid)
            continue
        if not res.get("id"):
            REPORT.add("recipe_without_result", t, rid)
            continue
        rec["result"] = {"item": mc(res["id"]), "count": res.get("count", 1)}
        if res.get("components"):
            rec["result"]["components"] = sorted(res["components"])
        rec["ingredients"] = ings
        # A recipe that needs its own result is a modification (dyeing), not a way to get
        # it. Only an ingredient whose EVERY option is the result counts: a red bundle
        # from "#bundles" may start from a plain bundle, so the tag merely includes itself.
        if any(x["items"] and all(i == rec["result"]["item"] for i in x["items"]) for x in ings):
            rec["self_input"] = True
        recipes.append(rec)
        handled[t] = handled.get(t, 0) + 1

    # --- villager trades -------------------------------------------------------------------
    trades = []
    vdir = os.path.join(root, "villager_trade")
    for f in files(vdir):
        tid = rel_id(vdir, f)
        d = load(f)
        parts = tid.split(":")[1].split("/")
        prof = parts[0]
        level = int(parts[1]) if len(parts) > 2 and parts[1].isdigit() else None
        wants = {}
        for k in ("wants", "additional_wants"):
            if k in d:
                wants[mc(d[k]["id"])] = wants.get(mc(d[k]["id"]), 0) + d[k].get("count", 1)
        g = d.get("gives", {})
        rec = {"id": tid, "profession": prof, "level": level, "wants": wants,
               "gives": {"item": mc(g["id"]), "count": g.get("count", 1)}}
        if "given_item_modifier" in d:
            rec["modified"] = True
        if "merchant_predicate" in d:
            rec["conditional"] = True
        trades.append(rec)

    # --- per-item index ----------------------------------------------------------------
    def src(i, kind):
        items.setdefault(i, {"stack": 64})
        s = items[i].setdefault("sources", [])
        if kind not in s:
            s.append(kind)

    for r in recipes:
        if not r.get("self_input"):
            src(r["result"]["item"].split("#")[0], r["type"].split(":")[1])
    for bid, b in blocks.items():
        for ctx, y in b["drops"].items():
            for i in y:
                src(i, "mine" if ctx == "plain" else "mine_" + ctx)
    for eid, m in mobs.items():
        for v in m["drops"].values():
            for ys in v.values():
                for i in ys:
                    src(i, "mob")
    for tid, r in other.items():
        for i in r["yield"]:
            src(i, "loot_" + r["kind"])
    for t in trades:
        src(t["gives"]["item"], "trade")
    for m in MANUAL:
        src(m["result"].split("#")[0], "manual")
    for i in items.values():
        if "sources" in i:
            i["sources"].sort()
    no_source = sorted(i for i, v in items.items() if not v.get("sources") and i != "minecraft:air")

    out = {
        "meta": {
            "version": "26.3",
            "generator": "tools/gen_knowledge.py",
            "sources": {
                "recipes, loot tables, tags, worldgen, structures, trades, fuel providers": root,
                "item components (stack, tool rules, cooking_fuel)": comp_dir,
                "block hardness, requires_correct_tool_for_drops, block->item": os.path.join(a.pumpkin, "blocks.json"),
                "mob max health": os.path.join(a.pumpkin, "entities.json"),
            },
            "loot_yield": "expected count per evaluation (block broken / kill / chest / barter / catch) and max",
            "contexts": "blocks: plain = correct tool, no enchantment; silk_touch; shears. mobs: player = killed by player",
        },
        "items": dict(sorted(items.items())),
        "item_tags": item_tags.all(),
        "tools": tools,
        "fuel": dict(sorted(fuel.items())),
        "blocks": dict(sorted(blocks.items())),
        "mobs": dict(sorted(mobs.items())),
        "loot_tables": other,
        "structures": structures,
        "recipes": sorted(recipes, key=lambda r: r["id"]),
        "trades": trades,
        "manual": {"note": "HAND-WRITTEN: interactions not described by the game's data",
                   "interactions": MANUAL, "dimension_access": DIMENSION_ACCESS,
                   "chest_aliases": CHEST_ALIASES},
        "report": {
            "recipe_types_handled": dict(sorted(handled.items())),
            "unhandled": REPORT.out(),
            "items_without_source": no_source,
        },
    }
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(out, f, sort_keys=True, indent=1, ensure_ascii=False)
        f.write("\n")
    print("wrote %s: %d items, %d blocks, %d recipes, %d mobs, %d other loot tables, %d trades, %d bytes"
          % (a.out, len(items), len(blocks), len(recipes), len(mobs), len(other), len(trades), os.path.getsize(a.out)))
    print("items without any source: %d" % len(no_source))
    for k, v in REPORT.out().items():
        print("  %s: %s" % (k, ", ".join("%s(%d)" % (x, y["count"]) for x, y in sorted(v.items()))[:400]))


if __name__ == "__main__":
    main()
