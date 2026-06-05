#!/usr/bin/env python3
"""Render a time-series of map frames (and an optional MP4) of an RSQA pollutant
or the IQA index, one frame per time bucket, each showing the bucket-mean
interpolated over the island.

Local tool — NOT part of the WASM build. It mirrors the web app's map rendering
(Web-Mercator CARTO basemap, inverse-distance-weighted heatmap with a
coverage-distance opacity fade, the same colour ramps / IQA acceptability bands,
station markers and colour-bar) but computes a fresh per-bucket average and
stamps each frame with its date. The colour scale is fixed across all frames so
colours are comparable over time.

Reads the committed daily tier (static/data/series-daily/station-<id>.json +
stations.json), so it's well suited to bucket sizes of a day or longer
(week/month/year). Sub-day buckets would need the hourly tier.

Requirements (local only):
    pip3 install pillow numpy
    # optional, for MP4 assembly:
    brew install ffmpeg

Examples:
    python3 scripts/animate.py --substance PM2.5 --bucket week --from 2023-01-01
    python3 scripts/animate.py --substance IQA --bucket month
    python3 scripts/animate.py --substance NO2 --bucket 10d --from 2024-01-01 --to 2024-12-31
"""

from __future__ import annotations

import argparse
import glob
import json
import math
import os
import subprocess
import sys
import time
import urllib.request
from datetime import date, timedelta

try:
    import numpy as np
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    sys.exit("This script needs Pillow and NumPy:  pip3 install pillow numpy")

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
DATA = os.path.join(ROOT, "static", "data")
TILE_CACHE = os.path.join(HERE, ".tilecache")

# ── Rendering constants (kept in sync with src/components/map.rs) ──
TILE = 512  # CARTO @2x tiles
BBOX_PAD = 0.14
COVERAGE_KM = 6.0
BASE_ALPHA = 150 / 255.0
TILE_URL = "https://a.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}@2x.png"

# RdYlBu reversed — cool=low, warm=high (concentrations, relative scale).
RAMP = [(0.0, (0x2C, 0x7B, 0xB6)), (0.25, (0xAB, 0xD9, 0xE9)), (0.5, (0xFF, 0xFF, 0xBF)),
        (0.75, (0xFD, 0xAE, 0x61)), (1.0, (0xD7, 0x19, 0x1C))]
# IQA absolute acceptability scale (green good → red poor), keyed by index value.
IQA_STOPS = [(0.0, (0x1A, 0x98, 0x50)), (25.0, (0xD9, 0xEF, 0x8B)), (37.5, (0xFE, 0xE0, 0x8B)),
             (50.0, (0xFD, 0xAE, 0x61)), (75.0, (0xF4, 0x6D, 0x43)), (100.0, (0xD7, 0x30, 0x27))]
IQA_GOOD_MAX, IQA_ACCEPTABLE_MAX = 25.0, 50.0

# Minimal display catalogue (mirrors src/data/pollutants.rs for the labels we show).
POLL = {
    "IQA": ("Air Quality Index (IQA)", ""), "CO": ("Carbon monoxide", "ppb"),
    "H2S": ("Hydrogen sulfide", "ppb"), "NO": ("Nitric oxide", "ppb"),
    "NO2": ("Nitrogen dioxide", "ppb"), "PM2.5": ("Fine particles (PM2.5)", "µg/m³"),
    "PM10": ("Respirable particles (PM10)", "µg/m³"), "PST": ("Total suspended particles", "µg/m³"),
    "O3": ("Ozone", "ppb"), "SO2": ("Sulfur dioxide", "ppb"),
    "COH": ("Coefficient of haze", "COH"), "BC1_370nm": ("Black carbon (370 nm)", "µg/m³"),
    "BC6_880nm": ("Black carbon (880 nm)", "µg/m³"), "PUF": ("Ultrafine particles", "part/cm³"),
    "Benzene": ("Benzene", "µg/m³"), "Toluene": ("Toluene", "µg/m³"),
    "Ethylbenzene": ("Ethylbenzene", "µg/m³"), "MP-Xylene": ("m,p-Xylene", "µg/m³"),
    "O-Xylene": ("o-Xylene", "µg/m³"),
}

FONT_CANDIDATES = [
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/Library/Fonts/Arial.ttf",
]


def font(size: int):
    for p in FONT_CANDIDATES:
        if os.path.exists(p):
            try:
                return ImageFont.truetype(p, size)
            except OSError:
                pass
    return ImageFont.load_default()


# ── Web-Mercator projection (same math as map.rs) ──
def lat_lon_to_tile(lat, lon, z):
    n = 2 ** z
    x = (lon + 180.0) / 360.0 * n
    y = (1.0 - math.asinh(math.tan(math.radians(lat))) / math.pi) / 2.0 * n
    return x, y


def pick_zoom(lat0, lat1, lon0, lon1, w, h):
    for z in range(18, -1, -1):
        x0, ytop = lat_lon_to_tile(lat1, lon0, z)
        x1, ybot = lat_lon_to_tile(lat0, lon1, z)
        if (x1 - x0) * TILE <= w and (ybot - ytop) * TILE <= h:
            return z
    return 0


# ── Colour helpers ──
def lerp_color(stops, x):
    for (x0, c0), (x1, c1) in zip(stops, stops[1:]):
        if x <= x1:
            f = (x - x0) / (x1 - x0) if x1 > x0 else 0.0
            return tuple(round(a + (b - a) * f) for a, b in zip(c0, c1))
    return stops[-1][1]


def ramp_color(t):
    return lerp_color(RAMP, max(0.0, min(1.0, t)))


def iqa_color(v):
    return lerp_color(IQA_STOPS, max(0.0, min(100.0, v)))


def color_channels(stops):
    xs = [s[0] for s in stops]
    return xs, [s[1][0] for s in stops], [s[1][1] for s in stops], [s[1][2] for s in stops]


# ── Data loading ──
def load_stations(substance):
    stations = json.load(open(os.path.join(DATA, "stations.json"), encoding="utf-8"))
    return [s for s in stations if substance in s.get("substances", [])]


def load_daily(station_id, substance):
    """Return (ordinals[np.int64], values[np.float64]) of daily means for the
    substance at this station, or (None, None) if absent."""
    path = os.path.join(DATA, "series-daily", f"station-{station_id}.json")
    if not os.path.exists(path):
        return None, None
    d = json.load(open(path, encoding="utf-8"))
    pairs = d.get("substances", {}).get(substance)
    if not pairs:
        return None, None
    base = date.fromisoformat(d["start_date"]).toordinal()
    arr = np.array(pairs, dtype=float)  # columns: day_index, value
    return (arr[:, 0].astype(np.int64) + base), arr[:, 1]


# ── Buckets ──
def make_buckets(start: date, end: date, bucket: str):
    """Yield (bucket_start, bucket_end_exclusive, label) over [start, end]."""
    # Labels use fixed-width numeric formats (YYYY-MM-DD / YYYY-MM / YYYY) so the
    # caption doesn't jitter frame-to-frame as variable-width month names would.
    out = []
    if bucket == "week":
        cur = start - timedelta(days=start.weekday())  # align to Monday
        while cur <= end:
            nxt = cur + timedelta(days=7)
            out.append((cur, nxt, cur.isoformat()))  # YYYY-MM-DD (week start)
            cur = nxt
    elif bucket == "month":
        y, m = start.year, start.month
        while date(y, m, 1) <= end:
            nm_y, nm_m = (y + 1, 1) if m == 12 else (y, m + 1)
            out.append((date(y, m, 1), date(nm_y, nm_m, 1), f"{y:04d}-{m:02d}"))  # YYYY-MM
            y, m = nm_y, nm_m
    elif bucket == "year":
        for y in range(start.year, end.year + 1):
            out.append((date(y, 1, 1), date(y + 1, 1, 1), str(y)))  # YYYY
    else:  # "<N>d"
        n = int(bucket.rstrip("d"))
        cur = start
        while cur <= end:
            nxt = cur + timedelta(days=n)
            out.append((cur, nxt, cur.isoformat()))  # YYYY-MM-DD (bucket start)
            cur = nxt
    return out


# ── Basemap ──
def fetch_tile(z, x, y):
    cache = os.path.join(TILE_CACHE, str(z), str(x), f"{y}.png")
    if os.path.exists(cache):
        return Image.open(cache).convert("RGB")
    os.makedirs(os.path.dirname(cache), exist_ok=True)
    url = TILE_URL.format(z=z, x=x, y=y)
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 (animate.py)"})
    with urllib.request.urlopen(req, timeout=30) as r:
        data = r.read()
    with open(cache, "wb") as f:
        f.write(data)
    time.sleep(0.15)  # be polite on cache miss
    return Image.open(cache).convert("RGB")


def build_basemap(geom, use_basemap):
    w, h = geom["w"], geom["h"]
    if not use_basemap:
        return Image.new("RGB", (w, h), (10, 22, 40))
    img = Image.new("RGB", (w, h), (10, 22, 40))
    z = geom["z"]
    x0, y0 = geom["x0"], geom["y0"]
    n = 2 ** z
    tx_min, tx_max = math.floor(x0), math.floor(x0 + w / TILE)
    ty_min, ty_max = math.floor(y0), math.floor(y0 + h / TILE)
    for tx in range(tx_min, tx_max + 1):
        for ty in range(ty_min, ty_max + 1):
            if ty < 0 or ty >= n:
                continue
            try:
                tile = fetch_tile(z, tx % n, ty)
            except Exception as e:  # noqa: BLE001 — best-effort basemap
                print(f"  tile {z}/{tx}/{ty} failed: {e}", file=sys.stderr)
                continue
            px = round((tx - x0) * TILE)
            py = round((ty - y0) * TILE)
            img.paste(tile, (px, py))
    return img


def compute_geom(stations, target_w, target_h):
    lats = [s["lat"] for s in stations]
    lons = [s["lon"] for s in stations]
    cy, cx = (min(lats) + max(lats)) / 2, (min(lons) + max(lons)) / 2
    lat_half = max(max(lats) - min(lats), 0.01) * (0.5 + BBOX_PAD)
    lon_half = max(max(lons) - min(lons), 0.01) * (0.5 + BBOX_PAD)
    blat0, blat1, blon0, blon1 = cy - lat_half, cy + lat_half, cx - lon_half, cx + lon_half
    z = pick_zoom(blat0, blat1, blon0, blon1, target_w, target_h)
    x0, ytop = lat_lon_to_tile(blat1, blon0, z)
    x1, ybot = lat_lon_to_tile(blat0, blon1, z)
    w = max(2, round((x1 - x0) * TILE)) & ~1  # even dims for yuv420p MP4
    h = max(2, round((ybot - ytop) * TILE)) & ~1
    mpp = 156543.03392 * math.cos(math.radians(cy)) / (2 ** z) * (256.0 / TILE)
    return {"z": z, "x0": x0, "y0": ytop, "w": w, "h": h, "cy": cy, "mpp": mpp}


def screen_xy(geom, lat, lon):
    tx, ty = lat_lon_to_tile(lat, lon, geom["z"])
    return (tx - geom["x0"]) * TILE, (ty - geom["y0"]) * TILE


# ── Overlay (IDW heatmap) ──
def render_overlay(base_img, geom, pts, is_iqa, vmin, vmax):
    """pts: list of (sx, sy, value). Returns base composited with the heatmap."""
    w, h = geom["w"], geom["h"]
    if len(pts) < 1:
        return base_img.copy()
    Y, X = np.mgrid[0:h, 0:w].astype(float)
    num = np.zeros((h, w))
    den = np.zeros((h, w))
    nearest = np.full((h, w), np.inf)
    for sx, sy, v in pts:
        d2 = (X - sx) ** 2 + (Y - sy) ** 2
        nearest = np.minimum(nearest, d2)
        wgt = 1.0 / (d2 + 1.0)
        num += wgt * v
        den += wgt
    val = num / den
    if is_iqa:
        xs, rr, gg, bb = color_channels(IQA_STOPS)
        clip = np.clip(val, 0.0, 100.0)
        r = np.interp(clip, xs, rr)
        g = np.interp(clip, xs, gg)
        b = np.interp(clip, xs, bb)
    else:
        span = max(vmax - vmin, 1e-9)
        t = np.clip((val - vmin) / span, 0.0, 1.0)
        xs, rr, gg, bb = color_channels(RAMP)
        r = np.interp(t, xs, rr)
        g = np.interp(t, xs, gg)
        b = np.interp(t, xs, bb)

    coverage_px = COVERAGE_KM * 1000.0 / geom["mpp"]
    nd = np.sqrt(nearest) / coverage_px
    f = np.clip(1.0 - nd, 0.0, 1.0)
    f = f * f * (3.0 - 2.0 * f)
    if len(pts) < 2:
        f *= 0.0  # a single station can't define a surface; show the marker only
    a = (BASE_ALPHA * f)[..., None]

    base = np.asarray(base_img, dtype=float)
    overlay = np.stack([r, g, b], axis=-1)
    out = base * (1 - a) + overlay * a
    return Image.fromarray(out.clip(0, 255).astype(np.uint8))


def draw_markers(draw, geom, stations, values, is_iqa, vmin, vmax):
    span = max(vmax - vmin, 1e-9)
    for s in stations:
        sx, sy = screen_xy(geom, s["lat"], s["lon"])
        v = values.get(s["id"])
        if v is None:
            draw.ellipse([sx - 5, sy - 5, sx + 5, sy + 5], outline=(150, 160, 180), width=2)
        else:
            col = iqa_color(v) if is_iqa else ramp_color((v - vmin) / span)
            draw.ellipse([sx - 6, sy - 6, sx + 6, sy + 6], fill=col, outline=(255, 255, 255), width=2)


def draw_colorbar(img, draw, substance, is_iqa, vmin, vmax):
    name, unit = POLL.get(substance, (substance, ""))
    bx, by, bw, bh = 12, img.height - 78, 232, 64
    draw.rectangle([bx, by, bx + bw, by + bh], fill=(13, 27, 42), outline=(42, 58, 92))
    draw.text((bx + 8, by + 6), f"{name} · mean", font=font(13), fill=(234, 234, 234))
    gx, gy, gw, gh = bx + 8, by + 26, bw - 16, 10
    if is_iqa:
        bands = [iqa_color(IQA_GOOD_MAX * 0.4), iqa_color((IQA_GOOD_MAX + IQA_ACCEPTABLE_MAX) / 2),
                 iqa_color(IQA_ACCEPTABLE_MAX * 1.6)]
        for k, c in enumerate(bands):
            draw.rectangle([gx + gw * k // 3, gy, gx + gw * (k + 1) // 3, gy + gh], fill=c)
        draw.text((gx, gy + gh + 3), "Good · Acceptable · Poor   (higher = worse)",
                  font=font(10), fill=(136, 146, 164))
    else:
        for k in range(gw):
            draw.line([(gx + k, gy), (gx + k, gy + gh)], fill=ramp_color(k / max(gw - 1, 1)))
        draw.text((gx, gy + gh + 3), f"{vmin:.1f}", font=font(10), fill=(136, 146, 164))
        lbl = f"{vmax:.1f} {unit}".strip()
        w = draw.textlength(lbl, font=font(10))
        draw.text((gx + gw - w, gy + gh + 3), lbl, font=font(10), fill=(136, 146, 164))


def draw_caption(img, draw, label, substance, n, agg_label):
    # Per-frame date overlay (top-left) + attribution (bottom-right).
    name = POLL.get(substance, (substance, ""))[0]
    pad = 10
    big = font(24)
    tw = draw.textlength(label, font=big)
    draw.rectangle([pad - 4, pad - 2, pad + tw + 8, pad + 30], fill=(13, 27, 42))
    draw.text((pad + 2, pad), label, font=big, fill=(255, 255, 255))
    sub = f"{name} · {agg_label} · {n} stations"
    draw.text((pad + 2, pad + 32), sub, font=font(12), fill=(200, 210, 230))
    attr = "© OpenStreetMap © CARTO · RSQA"
    aw = draw.textlength(attr, font=font(10))
    draw.text((img.width - aw - 8, img.height - 16), attr, font=font(10), fill=(200, 210, 230))


def main():
    ap = argparse.ArgumentParser(description="Render RSQA map animation frames.")
    ap.add_argument("--substance", default="PM2.5", help="pollutant key or IQA (default PM2.5)")
    ap.add_argument("--bucket", default="week", help="week | month | year | <N>d (default week)")
    ap.add_argument("--from", dest="dfrom", help="start date YYYY-MM-DD (default: data start)")
    ap.add_argument("--to", dest="dto", help="end date YYYY-MM-DD (default: data end)")
    ap.add_argument("--out", help="output directory (default anim/<sub>_<bucket>/)")
    ap.add_argument("--fps", type=int, default=8, help="MP4 frame rate (default 8)")
    ap.add_argument("--width", type=int, default=1100, help="target map width px (default 1100)")
    ap.add_argument("--height", type=int, default=820, help="target map height px (default 820)")
    ap.add_argument("--no-basemap", action="store_true", help="plain dark background, no tiles")
    ap.add_argument("--vmin", type=float, help="fix colour-scale minimum (concentrations)")
    ap.add_argument("--vmax", type=float, help="fix colour-scale maximum (concentrations)")
    args = ap.parse_args()

    sub = args.substance
    is_iqa = sub == "IQA"
    stations = load_stations(sub)
    if not stations:
        sys.exit(f"No stations measure '{sub}'. Check the substance key.")

    # Load each station's daily series for the substance.
    series = {}
    for s in stations:
        ords, vals = load_daily(s["id"], sub)
        if ords is not None:
            series[s["id"]] = (ords, vals)
    if not series:
        sys.exit(f"No daily data found for '{sub}'.")

    all_ords = np.concatenate([o for o, _ in series.values()])
    data_start = date.fromordinal(int(all_ords.min()))
    data_end = date.fromordinal(int(all_ords.max()))
    start = date.fromisoformat(args.dfrom) if args.dfrom else data_start
    end = date.fromisoformat(args.dto) if args.dto else data_end

    buckets = make_buckets(start, end, args.bucket)
    print(f"Substance: {sub} | bucket: {args.bucket} | range: {start}..{end}")
    print(f"Stations: {len(series)} | frames: {len(buckets)}")
    if not buckets:
        sys.exit("No buckets in range.")
    if len(buckets) > 1200:
        print(f"  WARNING: {len(buckets)} frames is a lot — consider --from/--to or a larger bucket.")

    # Pass 1: per-bucket {station_id: mean}, and the fixed global colour range.
    frame_values = []
    gmin, gmax = math.inf, -math.inf
    for bstart, bend, label in buckets:
        lo, hi = bstart.toordinal(), bend.toordinal()
        vals = {}
        for sid, (ords, vv) in series.items():
            mask = (ords >= lo) & (ords < hi)
            if mask.any():
                m = float(vv[mask].mean())
                vals[sid] = m
                gmin, gmax = min(gmin, m), max(gmax, m)
        frame_values.append((bstart, bend, label, vals))
    if gmin is math.inf:
        sys.exit("No data in the selected range.")
    vmin = args.vmin if args.vmin is not None else gmin
    vmax = args.vmax if args.vmax is not None else gmax
    print(f"Colour scale: {'IQA bands (0–100)' if is_iqa else f'{vmin:.2f} … {vmax:.2f}'}")

    agg_label = {"week": "weekly mean", "month": "monthly mean", "year": "annual mean"}.get(
        args.bucket, f"{args.bucket} mean")

    geom = compute_geom(stations, args.width, args.height)
    print(f"Canvas: {geom['w']}×{geom['h']} @ zoom {geom['z']}")
    out_dir = args.out or os.path.join(ROOT, "anim", f"{sub.replace('/', '_')}_{args.bucket}")
    os.makedirs(out_dir, exist_ok=True)
    # Clear any frames from a previous run so a shorter range can't leave stale
    # frames that would leak into the assembled MP4.
    for old in glob.glob(os.path.join(out_dir, "frame_*.png")):
        os.remove(old)

    # The basemap is identical for every frame → build it once.
    base = build_basemap(geom, not args.no_basemap)

    # Pass 2: render frames.
    by_station = {s["id"]: s for s in stations}
    for i, (bstart, bend, label, vals) in enumerate(frame_values, 1):
        pts = []
        for sid, v in vals.items():
            s = by_station[sid]
            sx, sy = screen_xy(geom, s["lat"], s["lon"])
            pts.append((sx, sy, v))
        img = render_overlay(base, geom, pts, is_iqa, vmin, vmax)
        draw = ImageDraw.Draw(img)
        draw_markers(draw, geom, stations, vals, is_iqa, vmin, vmax)
        draw_colorbar(img, draw, sub, is_iqa, vmin, vmax)
        draw_caption(img, draw, label, sub, len(vals), agg_label)
        img.save(os.path.join(out_dir, f"frame_{i:05d}.png"))
        if i % 20 == 0 or i == len(frame_values):
            print(f"  rendered {i}/{len(frame_values)}")

    print(f"Frames written to {out_dir}")

    # Optional MP4 via ffmpeg.
    mp4 = f"{out_dir}.mp4"
    if shutil_which("ffmpeg"):
        cmd = ["ffmpeg", "-y", "-framerate", str(args.fps),
               "-i", os.path.join(out_dir, "frame_%05d.png"),
               "-c:v", "libx264", "-pix_fmt", "yuv420p", mp4]
        print(f"Assembling MP4: {mp4}")
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode == 0:
            print(f"Wrote {mp4}")
        else:
            print("ffmpeg failed (frames are still available):\n" + r.stderr[-500:], file=sys.stderr)
    else:
        print("ffmpeg not found — frames only. Install with `brew install ffmpeg` for MP4,\n"
              f"or assemble manually, e.g.:\n"
              f"  ffmpeg -framerate {args.fps} -i {out_dir}/frame_%05d.png -pix_fmt yuv420p {mp4}")


def shutil_which(name):
    from shutil import which
    return which(name)


if __name__ == "__main__":
    main()
