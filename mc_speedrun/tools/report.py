#!/usr/bin/env python3
"""Run report: one self-contained HTML page from a telemetry log (runs/<unix>.jsonl).

Usage: python3 tools/report.py [runs/<file>.jsonl] [-o out.html]
Default input: the newest log in ./runs; output: next to it, with .html.
Time is IGT (game ticks, 20 per second); real time is the `wall` field.
Stdlib only, no external requests; the page works offline.
"""
import argparse
import glob
import html
import json
import math
import os
import sys

TPS = 20.0


# ---------- parsing ----------

def load(path):
    """Returns (events, skipped_lines). Bad / half-written lines are skipped."""
    events, skipped = [], 0
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    o = json.loads(line)
                except ValueError:
                    skipped += 1
                    continue
                if not isinstance(o, dict) or not isinstance(o.get("t"), (int, float)):
                    skipped += 1
                    continue
                o.setdefault("kind", "?")
                o["wall"] = o["wall"] if isinstance(o.get("wall"), (int, float)) else 0.0
                events.append(o)
    except OSError as e:
        sys.exit(f"cannot read {path}: {e}")
    return events, skipped


def num(v, d=0.0):
    return v if isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) else d


def fmt_igt(ticks):
    s = ticks / TPS
    m = int(s // 60)
    return f"{m:02d}:{s - 60 * m:05.2f}"


def fmt_real(sec):
    m = int(sec // 60)
    return f"{m:02d}:{sec - 60 * m:05.2f}"


def fmt_delta(ticks):
    s = ticks / TPS
    return ("+" if s >= 0 else "-") + f"{abs(s):.2f}s"


def analyse(events):
    """Stages (one row per start; retries are separate rows), samples, deaths, rest."""
    stages, open_by_name = [], {}
    pos, deaths, advs, others = [], [], [], []
    for e in events:
        k = e["kind"]
        if k == "stage_start":
            st = {"name": str(e.get("stage", "?")), "t0": e["t"], "w0": e["wall"],
                  "t1": None, "w1": None, "ok": None, "error": None}
            stages.append(st)
            open_by_name[st["name"]] = st
        elif k == "stage_end":
            name = str(e.get("stage", "?"))
            st = open_by_name.pop(name, None)
            if st is None:  # end without a start (log began mid-run)
                st = {"name": name, "t0": None, "w0": None}
                stages.append(st)
            st.update(t1=e["t"], w1=e["wall"], ok=bool(e.get("ok")), error=e.get("error"))
        elif k == "pos":
            if all(isinstance(e.get(a), (int, float)) for a in "xyz"):
                pos.append(e)
        elif k == "death":
            deaths.append(e)
        elif k == "advancement":
            advs.append(e)
        else:
            others.append(e)
    return stages, pos, deaths, advs, others


def best_previous(this_path, dirname):
    """Best ok duration (ticks) per stage among the other logs: name -> (ticks, file)."""
    best = {}
    for p in glob.glob(os.path.join(dirname, "*.jsonl")):
        if os.path.abspath(p) == os.path.abspath(this_path):
            continue
        ev, _ = load(p)
        for st in analyse(ev)[0]:
            if st["ok"] and st["t0"] is not None and st["t1"] is not None:
                d = st["t1"] - st["t0"]
                if st["name"] not in best or d < best[st["name"]][0]:
                    best[st["name"]] = (d, os.path.basename(p))
    return best


# ---------- svg helpers ----------

def esc(s):
    return html.escape(str(s), quote=True)


def nice_ticks(lo, hi, n=5):
    if hi <= lo:
        hi = lo + 1
    raw = (hi - lo) / n
    mag = 10 ** math.floor(math.log10(raw))
    step = min((m * mag for m in (1, 2, 5, 10) if m * mag >= raw), default=10 * mag)
    t = math.ceil(lo / step) * step
    out = []
    while t <= hi + 1e-9:
        out.append(round(t, 6))
        t += step
    return out


def tick_label(v):
    return str(int(v)) if float(v).is_integer() else f"{v:g}"


RAMP = [(68, 1, 84), (59, 82, 139), (33, 145, 140), (94, 201, 98), (253, 231, 37)]


def ramp(f):
    """Time colour: purple (early) to yellow (late); readable on light and dark."""
    f = min(max(f, 0), 1) * (len(RAMP) - 1)
    i = min(int(f), len(RAMP) - 2)
    a, b, u = RAMP[i], RAMP[i + 1], f - i
    return "#%02x%02x%02x" % tuple(round(a[j] + (b[j] - a[j]) * u) for j in range(3))


def nearest_pos(pos, t):
    """Last sample at or before t (or the first one)."""
    best = None
    for p in pos:
        if p["t"] <= t:
            best = p
        else:
            break
    return best or (pos[0] if pos else None)


def short_dim(d):
    return str(d).replace("minecraft:", "") or "?"


def series_chart(width, height, series, tmax, bands, marks, ylabel, ymin=None, ymax=None):
    """Lines over ticks. series: [(label, css-class, [(t, v), ...])]; bands: stage rows."""
    ml, mr, mt, mb = 44, 12, 10, 26
    pw, ph = width - ml - mr, height - mt - mb
    vals = [v for _, _, pts in series for _, v in pts]
    lo = ymin if ymin is not None else (min(vals) if vals else 0)
    hi = ymax if ymax is not None else (max(vals) if vals else 1)
    if hi - lo < 1e-9:
        hi, lo = hi + 1, lo - 1
    pad = 0 if ymin is not None else (hi - lo) * 0.05
    lo, hi = lo - pad, hi + pad
    tmax = max(tmax, 1)
    X = lambda t: ml + pw * t / tmax
    Y = lambda v: mt + ph * (1 - (v - lo) / (hi - lo))
    o = [f'<svg viewBox="0 0 {width} {height}" class="chart" role="img" aria-label="{esc(ylabel)} over time">']
    for i, b in enumerate(bands):
        if b["t0"] is None:
            continue
        t1 = b["t1"] if b["t1"] is not None else tmax
        o.append(f'<rect class="band b{i % 2}" x="{X(b["t0"]):.1f}" y="{mt}" width="{max(X(t1) - X(b["t0"]), 1):.1f}" '
                 f'height="{ph}"><title>{esc(b["name"])}</title></rect>')
    for v in nice_ticks(lo, hi, 4):
        o.append(f'<line class="grid" x1="{ml}" x2="{ml + pw}" y1="{Y(v):.1f}" y2="{Y(v):.1f}"/>'
                 f'<text class="tick" x="{ml - 5}" y="{Y(v) + 4:.1f}" text-anchor="end">{tick_label(v)}</text>')
    for t in nice_ticks(0, tmax / TPS, 6):
        o.append(f'<line class="grid" x1="{X(t * TPS):.1f}" x2="{X(t * TPS):.1f}" y1="{mt}" y2="{mt + ph}"/>'
                 f'<text class="tick" x="{X(t * TPS):.1f}" y="{height - 8}" text-anchor="middle">{fmt_igt(t * TPS)[:5]}</text>')
    for t, label in marks:
        o.append(f'<line class="death" x1="{X(t):.1f}" x2="{X(t):.1f}" y1="{mt}" y2="{mt + ph}"><title>{esc(label)}</title></line>')
    for label, cls, pts in series:
        if not pts:
            continue
        d = "M" + " L".join(f"{X(t):.1f} {Y(v):.1f}" for t, v in pts)
        if len(pts) == 1:
            o.append(f'<circle class="dot {cls}" cx="{X(pts[0][0]):.1f}" cy="{Y(pts[0][1]):.1f}" r="2.5"/>')
        o.append(f'<path class="line {cls}" d="{d}"><title>{esc(label)}</title></path>')
    o.append(f'<text class="tick" x="4" y="{mt + 8}" >{esc(ylabel)}</text></svg>')
    return "".join(o)


def runs_by_dim(pos):
    """Split samples into consecutive runs with the same dimension."""
    runs = []
    for p in pos:
        d = p.get("dim") or "?"
        if not runs or runs[-1][0] != d:
            runs.append((d, []))
        runs[-1][1].append(p)
    return runs


def map_svg(dim, runs, all_pos, stages, deaths, tmax, is_first, is_last):
    pts = [p for d, r in runs if d == dim for p in r]
    xs, zs = [p["x"] for p in pts], [p["z"] for p in pts]
    W = 640
    x0, x1, z0, z1 = min(xs), max(xs), min(zs), max(zs)
    span = max(x1 - x0, z1 - z0, 16.0)  # equal scale for both axes, never tiny
    cx, cz = (x0 + x1) / 2, (z0 + z1) / 2
    span *= 1.12
    ml, mb, mt, mr = 46, 24, 8, 10
    side = W - ml - mr
    H = side + mt + mb
    vx0, vz0 = cx - span / 2, cz - span / 2
    X = lambda x: ml + side * (x - vx0) / span
    Z = lambda z: mt + side * (z - vz0) / span  # +z is south = down, as in F3 top-down
    o = [f'<svg viewBox="0 0 {W} {H}" class="chart map" role="img" aria-label="path in {esc(short_dim(dim))}">',
         f'<rect class="frame" x="{ml}" y="{mt}" width="{side}" height="{side}"/>']
    for v in nice_ticks(vx0, vx0 + span, 5):
        o.append(f'<line class="grid" x1="{X(v):.1f}" x2="{X(v):.1f}" y1="{mt}" y2="{mt + side}"/>'
                 f'<text class="tick" x="{X(v):.1f}" y="{H - 8}" text-anchor="middle">{tick_label(v)}</text>')
    for v in nice_ticks(vz0, vz0 + span, 5):
        o.append(f'<line class="grid" x1="{ml}" x2="{ml + side}" y1="{Z(v):.1f}" y2="{Z(v):.1f}"/>'
                 f'<text class="tick" x="{ml - 5}" y="{Z(v) + 4:.1f}" text-anchor="end">{tick_label(v)}</text>')
    # path: one coloured segment per pair of consecutive samples
    for d, r in runs:
        if d != dim:
            continue
        if len(r) == 1:
            p = r[0]
            o.append(f'<circle cx="{X(p["x"]):.1f}" cy="{Z(p["z"]):.1f}" r="2" fill="{ramp(p["t"] / max(tmax, 1))}"/>')
        for a, b in zip(r, r[1:]):
            o.append(f'<line x1="{X(a["x"]):.1f}" y1="{Z(a["z"]):.1f}" x2="{X(b["x"]):.1f}" y2="{Z(b["z"]):.1f}" '
                     f'stroke="{ramp(b["t"] / max(tmax, 1))}" stroke-width="2.2" stroke-linecap="round">'
                     f'<title>{fmt_igt(b["t"])} y={b["y"]:.1f}</title></line>')
    def inside(t):
        p = nearest_pos(all_pos, t)
        return p if p is not None and (p.get("dim") or "?") == dim else None
    for st in stages:
        if st["t0"] is None:
            continue
        p = inside(st["t0"])
        if p:
            px, pz = X(p["x"]), Z(p["z"])
            o.append(f'<circle class="stage" cx="{px:.1f}" cy="{pz:.1f}" r="4"/>'
                     f'<text class="lbl" x="{px + 6:.1f}" y="{pz - 5:.1f}">{esc(st["name"])}</text>')
    for dth in deaths:
        p = inside(dth["t"])
        if p:
            px, pz = X(p["x"]), Z(p["z"])
            o.append(f'<path class="deathmark" d="M{px - 5:.1f} {pz - 5:.1f}L{px + 5:.1f} {pz + 5:.1f}M{px - 5:.1f} {pz + 5:.1f}L{px + 5:.1f} {pz - 5:.1f}">'
                     f'<title>death {fmt_igt(dth["t"])}</title></path>')
    if is_first and all_pos:
        p = all_pos[0]
        o.append(f'<circle class="start" cx="{X(p["x"]):.1f}" cy="{Z(p["z"]):.1f}" r="5.5"><title>start</title></circle>')
    if is_last and all_pos:
        p = all_pos[-1]
        px, pz = X(p["x"]), Z(p["z"])
        o.append(f'<rect class="end" x="{px - 5:.1f}" y="{pz - 5:.1f}" width="10" height="10"><title>end {fmt_igt(p["t"])}</title></rect>')
    o.append(f'<text class="tick" x="{ml + 4}" y="{mt + 12}">x &#8594; (east), z &#8595; (south)</text></svg>')
    return "".join(o)


# ---------- page ----------

CSS = """
:root{--bg:#fafafa;--fg:#1d1f23;--mute:#666d78;--card:#fff;--line:#d9dce1;--grid:#e8eaee;--acc:#2a6fdb;
--ok:#1b7f3b;--bad:#c0262d;--hp:#d23b3b;--food:#b8741a;--y:#2a6fdb;--band0:rgba(120,130,150,.10);--band1:rgba(120,130,150,0)}
@media(prefers-color-scheme:dark){:root{--bg:#14161a;--fg:#e6e8ec;--mute:#9aa1ac;--card:#1c1f25;--line:#343942;--grid:#2a2e36;
--acc:#6aa3ff;--ok:#5cc47c;--bad:#ff6b72;--hp:#ff6b6b;--food:#e8a54a;--y:#6aa3ff;--band0:rgba(160,170,190,.12);--band1:rgba(160,170,190,0)}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.45 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}
main{max-width:860px;margin:0 auto;padding:16px}
h1{font-size:1.35rem;margin:.2em 0 .6em}h2{font-size:1.1rem;margin:1.6em 0 .5em}
.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:8px}
.card{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:8px 12px}
.card b{display:block;font-size:1.25rem;font-variant-numeric:tabular-nums}.card span{color:var(--mute);font-size:.8rem}
.ok{color:var(--ok)}.bad{color:var(--bad)}.mute{color:var(--mute)}
.scroll{overflow-x:auto}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}
th,td{text-align:left;padding:5px 8px;border-bottom:1px solid var(--line);white-space:nowrap}
td.err{white-space:normal;min-width:12em;word-break:break-word}th{color:var(--mute);font-weight:600;font-size:.8rem}
td.n,th.n{text-align:right}
svg.chart{width:100%;height:auto;display:block;background:var(--card);border:1px solid var(--line);border-radius:8px}
.grid{stroke:var(--grid);stroke-width:1}.tick{fill:var(--mute);font-size:11px}.lbl{fill:var(--fg);font-size:11px}
.frame{fill:none;stroke:var(--line)}.band.b0{fill:var(--band0)}.band.b1{fill:var(--band1)}
.line{fill:none;stroke-width:1.8;stroke-linejoin:round}.line.hp{stroke:var(--hp)}.line.food{stroke:var(--food)}.line.yy{stroke:var(--y)}
.dot.hp{fill:var(--hp)}.dot.food{fill:var(--food)}.dot.yy{fill:var(--y)}
.death{stroke:var(--bad);stroke-width:1.5;stroke-dasharray:4 3}.deathmark{stroke:var(--bad);stroke-width:2.5;fill:none}
.stage{fill:var(--bg);stroke:var(--fg);stroke-width:1.6}.start{fill:var(--ok);stroke:var(--bg);stroke-width:1.5}.end{fill:var(--fg);stroke:var(--bg)}
.legend{display:flex;gap:14px;flex-wrap:wrap;color:var(--mute);font-size:.85rem;margin:.3em 0 .5em;align-items:center}
.sw{display:inline-block;width:18px;height:3px;vertical-align:middle;margin-right:4px;border-radius:2px}
.ramp{display:inline-block;width:90px;height:8px;border-radius:4px;vertical-align:middle}
ul.ev{list-style:none;padding:0;margin:0}ul.ev li{padding:4px 0;border-bottom:1px solid var(--line);display:flex;gap:10px}
ul.ev time{color:var(--mute);font-variant-numeric:tabular-nums;min-width:6.5em;flex:none}
code{font-size:.85em;word-break:break-all}
"""


def esc_val(v):
    s = json.dumps(v, ensure_ascii=False) if not isinstance(v, str) else v
    return esc(s)


def build(path, events, skipped, best):
    stages, pos, deaths, advs, others = analyse(events)
    tmax = max([e["t"] for e in events] + [0])
    wmax = max([e["wall"] for e in events] + [0])
    name = os.path.basename(path)
    out = []
    add = out.append

    # result
    done = [s for s in stages if s["t1"] is not None]
    running = [s for s in stages if s["t1"] is None]
    if not events:
        result, rcls = "empty log", "mute"
    elif running:
        result, rcls = f"stopped during {running[-1]['name']} (no stage_end)", "bad"
    elif done and not done[-1]["ok"]:
        result, rcls = f"failed in {done[-1]['name']}: {done[-1].get('error') or 'no error text'}", "bad"
    elif done:
        result, rcls = f"ok, last stage reached: {done[-1]['name']}", "ok"
    else:
        result, rcls = "no stages recorded", "mute"
    first_wall = ""
    try:  # logs are named by the unix start time
        import datetime
        ts = int(os.path.splitext(name)[0])
        first_wall = datetime.datetime.fromtimestamp(ts, datetime.timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
    except (ValueError, OverflowError, OSError):
        first_wall = "unknown"

    add(f"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>"
        f"<title>Run report {esc(name)}</title><style>{CSS}</style></head><body><main>")
    add(f"<h1>Run report <span class=mute>{esc(name)}</span></h1><div class=cards>")
    for big, small, cls in [(first_wall, "run start", ""), (fmt_igt(tmax), "total IGT (mm:ss.ss)", ""),
                            (fmt_real(wmax), "real time (mm:ss.ss)", ""), (str(len(deaths)), "deaths", "bad" if deaths else ""),
                            (str(len(done)) + " / " + str(len(stages)), "stages finished / started", "")]:
        add(f"<div class=card><b class='{cls}' style='font-size:{'1rem' if big == first_wall else '1.25rem'}'>{esc(big)}</b><span>{small}</span></div>")
    add(f"</div><p class='{rcls}'><b>Result:</b> {esc(result)}</p>")
    if skipped:
        add(f"<p class=mute>{skipped} unreadable line(s) skipped (a truncated last line is normal for a run that is still going).</p>")
    if not events:
        add("<p>No data in this log.</p></main></body></html>")
        return "".join(out)

    # splits
    add("<h2>Splits</h2><div class=scroll><table><tr><th>stage<th class=n>start IGT<th class=n>end IGT<th class=n>IGT time"
        "<th class=n>real time<th>status<th class=n>best before<th class=n>vs best<th>error</tr>")
    for s in stages:
        dur = (s["t1"] - s["t0"]) if s["t0"] is not None and s["t1"] is not None else None
        wdur = (s["w1"] - s["w0"]) if s["w0"] is not None and s["w1"] is not None else None
        status = "<span class=mute>unfinished</span>" if s["t1"] is None else (
            "<span class=ok>ok</span>" if s["ok"] else "<span class=bad>failed</span>")
        b = best.get(s["name"])
        bcell, dcell = "", ""
        if b:
            bcell = f"<span title='{esc(b[1])}'>{fmt_igt(b[0])}</span>"
            if dur is not None and s["ok"]:
                dcell = f"<span class='{'ok' if dur <= b[0] else 'bad'}'>{fmt_delta(dur - b[0])}</span>"
        add(f"<tr><td>{esc(s['name'])}<td class=n>{fmt_igt(s['t0']) if s['t0'] is not None else '-'}"
            f"<td class=n>{fmt_igt(s['t1']) if s['t1'] is not None else '-'}"
            f"<td class=n>{fmt_igt(dur) if dur is not None else '-'}<td class=n>{fmt_real(wdur) if wdur is not None else '-'}"
            f"<td>{status}<td class=n>{bcell}<td class=n>{dcell}<td class=err>{esc(s.get('error') or '')}</tr>")
    if not stages:
        add("<tr><td colspan=9 class=mute>no stage events</tr>")
    add("</table></div>")
    add("<p class=mute>Best before: best ok duration of the stage among the other logs in the same folder"
        + ("" if best else " (none found)") + ". Hover for the log name.</p>")

    # maps
    add("<h2>Path</h2>")
    runs = runs_by_dim(pos)
    dims = []
    for d, _ in runs:
        if d not in dims:
            dims.append(d)
    if not pos:
        add("<p class=mute>No position samples.</p>")
    else:
        add(f"<div class=legend><span>time: <span class=ramp style='background:linear-gradient(90deg,{','.join(ramp(i / 8) for i in range(9))})'></span> "
            f"0 to {fmt_igt(tmax)}</span><span>&#9679; green start</span><span>&#9633; end</span>"
            f"<span>&#9675; stage start</span><span style='color:var(--bad)'>&#10005; death</span></div>")
        for i, d in enumerate(dims):
            n = sum(len(r) for dd, r in runs if dd == d)
            first_dim = pos[0].get("dim") or "?"
            last_dim = pos[-1].get("dim") or "?"
            add(f"<h3 style='font-size:1rem;margin:1em 0 .3em'>{esc(short_dim(d))} <span class=mute>({n} samples)</span></h3>")
            add(map_svg(d, runs, pos, stages, deaths, tmax, d == first_dim, d == last_dim))
        # y chart
        ys = []
        for d, r in runs:
            ys.append((f"y in {short_dim(d)}", "yy", [(p["t"], p["y"]) for p in r]))
        add("<h2>Height (y) over time</h2>")
        add(series_chart(640, 170, ys, tmax, stages, [(d["t"], "death") for d in deaths], "y"))
        if len(dims) > 1:
            changes = ", ".join(f"{fmt_igt(r[0]['t'])[:5]} {short_dim(d)}" for d, r in runs)
            add(f"<p class=mute>Dimension changes: {esc(changes)}</p>")

    # hp/food
    add("<h2>HP and food</h2>")
    if pos:
        hp = [(p["t"], num(p.get("hp"))) for p in pos]
        fd = [(p["t"], num(p.get("food"))) for p in pos]
        add("<div class=legend><span><i class=sw style='background:var(--hp)'></i>hp (half hearts)</span>"
            "<span><i class=sw style='background:var(--food)'></i>food</span><span>shaded: stage bands, dashed red: death</span></div>")
        add(series_chart(640, 200, [("hp", "hp", hp), ("food", "food", fd)], tmax, stages,
                         [(d["t"], "death") for d in deaths], "hp / food", ymin=0, ymax=20))
    else:
        add("<p class=mute>No samples.</p>")

    # advancements
    add("<h2>Advancements</h2>")
    if advs:
        add("<ul class=ev>" + "".join(f"<li><time>{fmt_igt(a['t'])}</time><span>{esc(a.get('id', '?'))}</span></li>" for a in advs) + "</ul>")
    else:
        add("<p class=mute>None recorded.</p>")

    # events
    add("<h2>Events</h2><ul class=ev>")
    rows = []
    for e in events:
        k = e["kind"]
        if k == "pos":
            continue
        if k == "stage_start":
            rows.append((e["t"], f"stage start: <b>{esc(e.get('stage'))}</b>", ""))
        elif k == "stage_end":
            ok = e.get("ok")
            rows.append((e["t"], f"stage end: <b>{esc(e.get('stage'))}</b> <span class='{'ok' if ok else 'bad'}'>{'ok' if ok else 'failed'}</span>"
                         + (f" - {esc(e.get('error'))}" if e.get("error") else ""), ""))
        elif k == "advancement":
            rows.append((e["t"], f"advancement: {esc(e.get('id'))}", ""))
        elif k == "death":
            extra = {a: b for a, b in e.items() if a not in ("t", "wall", "kind")}
            rows.append((e["t"], "<b class=bad>death</b>" + (f" <code>{esc_val(extra)}</code>" if extra else ""), ""))
        else:
            extra = {a: b for a, b in e.items() if a not in ("t", "wall", "kind")}
            rows.append((e["t"], f"<b>{esc(k)}</b> <code>{esc_val(extra)}</code>" if extra else f"<b>{esc(k)}</b>", ""))
    for t, text, _ in rows:
        add(f"<li><time>{fmt_igt(t)}</time><span>{text}</span></li>")
    if not rows:
        add("<li class=mute>no events</li>")
    add("</ul></main></body></html>")
    return "".join(out)


def main():
    ap = argparse.ArgumentParser(description="HTML report from a bot telemetry log")
    ap.add_argument("log", nargs="?", help="runs/<file>.jsonl (default: newest in ./runs)")
    ap.add_argument("-o", "--output", help="output html (default: next to the log)")
    a = ap.parse_args()
    path = a.log
    if not path:
        logs = sorted(glob.glob("runs/*.jsonl"), key=lambda p: (os.path.getmtime(p), p))
        if not logs:
            sys.exit("no logs in runs/")
        path = logs[-1]
    events, skipped = load(path)
    best = best_previous(path, os.path.dirname(os.path.abspath(path)))
    out = a.output or os.path.splitext(path)[0] + ".html"
    with open(out, "w", encoding="utf-8") as f:
        f.write(build(path, events, skipped, best))
    print(out)


if __name__ == "__main__":
    main()
