#!/usr/bin/env python3
"""Preprocess the Montréal RSQA archive (1986–2024) into compact static files.

Reads every annual multi-pollutant CSV plus the IQA "détaillé par station"
bundles from ``data-src/`` (fetch them with ``scripts/fetch-archive.sh``) and the
station list, then writes the slim files the WASM app fetches at runtime.

The annual files drift in schema across eras (station/time column names, date
format and hour convention, pollutant spellings, missing-value markers, a leading
blank line in some years), so parsing is deliberately tolerant — see
``find_header`` / ``canon_poll`` / ``parse_dt``.

Outputs (under ``static/data/``):
  stations.json                     station metadata + years each reported
  map-stats.json                    {year|"all" -> station -> substance -> {mean,median,min,max,n}}
  map-stats-detailed.json           {year -> station -> substance -> {wd,we: [[mean,median,min,max,n]|null x24]}}
  iqa-dominance.json                {year -> station -> {peak_pollutant,peak_iqa,shares}}
  series-daily/station-<id>.json    daily means per substance across all years
  series/station-<id>-<year>.json   hourly values per station-year (loaded on demand)
  meta.json                         years / latest / generation stamp / attribution

No third-party dependencies — standard library only.

Usage:
    scripts/fetch-archive.sh        # download the archive into data-src/ (once)
    python3 scripts/preprocess.py
"""

from __future__ import annotations

import csv
import glob
import json
import os
import re
import statistics
from collections import Counter, defaultdict
from datetime import date, datetime, timedelta, timezone
from zoneinfo import ZoneInfo

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW = os.path.join(ROOT, "data-src")
OUT = os.path.join(ROOT, "static", "data")
STATIONS_CSV = os.path.join(RAW, "liste-des-stations.csv")

MONTREAL = ZoneInfo("America/Montreal")
UTC = timezone.utc

SOURCE_URL = "https://donnees.montreal.ca/dataset/rsqa-polluants-gazeux"
STATION_LIST_URL = "https://donnees.montreal.ca/dataset/rsqa-liste-des-stations"

MISSING = {"", "N/M", "N/A", "NA", "ND"}
STATION_COLS = {"poste", "numero_station", "no_poste"}
TIME_COLS = {"temps", "date_heure"}  # single datetime column (spaces removed)
IGNORE_COLS = {"site", "nom", "name", "adresse"}  # non-data text columns
IQA_KEY = "IQA"

# Canonical pollutant key for each source spelling (normalized lookup).
POLL_CANON = {
    "co": "CO", "h2s": "H2S", "no": "NO", "no2": "NO2",
    "pm25": "PM2.5", "pm2.5": "PM2.5", "pm2,5": "PM2.5", "pm10": "PM10", "pst": "PST",
    "o3": "O3", "so2": "SO2", "coh": "COH",
    "bc1_370nm": "BC1_370nm", "bc6_880nm": "BC6_880nm", "puf": "PUF",
    "benzene": "Benzene", "toluene": "Toluene", "ethylbenzene": "Ethylbenzene",
    "mp-xylene": "MP-Xylene", "o-xylene": "O-Xylene",
}


def normcol(s: str) -> str:
    """Normalize a header cell for alias matching: drop BOM/spaces, lower-case."""
    return s.strip().lower().replace("﻿", "").replace(" ", "")
# Display order for the substance picker (present ones only; IQA leads).
SUB_ORDER = [
    IQA_KEY, "CO", "H2S", "NO", "NO2", "PM2.5", "PM10", "PST", "O3", "SO2",
    "COH", "BC1_370nm", "BC6_880nm", "PUF",
    "Benzene", "Toluene", "Ethylbenzene", "MP-Xylene", "O-Xylene",
]
# IQA file pollutant codes → catalogue keys.
IQA_POLL = {"PM": "PM2.5", "O3": "O3", "NO2": "NO2", "CO": "CO", "SO2": "SO2"}


def canon_poll(name: str) -> str:
    return POLL_CANON.get(normcol(name), name.strip())


def parse_date_hour(date_str: str, hour_str: str):
    """Combine a separate date column + 'heure' column (e.g. 2014) into UTC.
    `hour_str` is like '1:00' / '24:00' / '1'; date is YYYY-MM-DD or DD-MM-YYYY."""
    ds = date_str.strip()
    m = re.match(r"^(\d{4})-(\d{2})-(\d{2})", ds)
    if m:
        y, mo, d = (int(x) for x in m.groups())
    else:
        m = re.match(r"^(\d{2})-(\d{2})-(\d{4})", ds)
        if not m:
            return None
        d, mo, y = (int(x) for x in m.groups())
    try:
        hh = int(hour_str.strip().split(":")[0])
        base = datetime(y, mo, d, 0, 0, tzinfo=MONTREAL)
    except ValueError:
        return None
    return (base + timedelta(hours=hh)).astimezone(UTC)


def num(v: str):
    v = v.strip()
    if v in MISSING:
        return None
    try:
        return float(v)
    except ValueError:
        return None


def parse_dt(s: str):
    """Parse a timestamp in either era's format to UTC (Montréal-local based).

    2013+  : 'DD-MM-YYYY HH:MM', end-of-measurement (HH=24 → next-day 00:00).
    ≤2012  : 'YYYY-MM-DD HH:MM[:SS[.fff]]', start-of-measurement (HH 0–23).
    The ≤1 h start/end labelling difference is immaterial at day/month/year scale.
    """
    s = s.strip()
    if not s:
        return None
    m = re.match(r"^(\d{2})-(\d{2})-(\d{4}) (\d{1,2}):(\d{2})$", s)
    if m:
        d, mo, y, hh, mm = (int(x) for x in m.groups())
    else:
        m = re.match(r"^(\d{4})-(\d{2})-(\d{2}) (\d{1,2}):(\d{2})", s)
        if not m:
            return None
        y, mo, d, hh, mm = (int(x) for x in m.groups())
    try:
        base = datetime(y, mo, d, 0, mm, tzinfo=MONTREAL)
    except ValueError:
        return None  # occasional malformed timestamp in the archive — skip
    return (base + timedelta(hours=hh)).astimezone(UTC)


def find_header(rows):
    """Return (header_index, fields) for the first row containing a station-column
    alias (in any position — some years prepend a `site` text column). Tolerates
    leading blank / BOM-only rows seen in some years."""
    for i, fields in enumerate(rows):
        if any(normcol(x) in STATION_COLS for x in fields):
            return i, fields
    return None, None


def load_station_coords() -> dict[int, dict]:
    out: dict[int, dict] = {}
    with open(STATIONS_CSV, encoding="utf-8-sig", newline="") as f:
        r = csv.reader(f)
        next(r, None)
        for row in r:
            if not row or not row[0].strip():
                continue
            try:
                sid = int(row[0].strip())
                lat = float(row[6])
                lon = float(row[7])
            except (ValueError, IndexError):
                continue
            # Guard against malformed coordinates in the source (e.g. station 62's
            # latitude is published as "45045755"). Keep only plausibly-Montréal
            # points; out-of-range stations are treated as having no coordinates.
            if not (45.0 <= lat <= 46.0 and -74.5 <= lon <= -73.0):
                continue
            out[sid] = {
                "name": row[3].strip(), "address": row[4].strip(),
                "borough": row[5].strip(), "lat": lat, "lon": lon,
            }
    return out


def year_base_utc(year: int) -> datetime:
    """UTC instant of Jan 1 00:00 Montréal local for the given year (hourly base)."""
    return datetime(year, 1, 1, 0, 0, tzinfo=MONTREAL).astimezone(UTC)


def stat_cell(values: list[float]) -> dict:
    return {
        "mean": round(statistics.fmean(values), 4),
        "median": round(statistics.median(values), 4),
        "min": round(min(values), 4),
        "max": round(max(values), 4),
        "n": len(values),
    }


def stat_cell_compact(values: list[float]) -> list:
    """Compact ``[mean, median, min, max, n]`` cell for the detailed map stats.
    Same math as ``stat_cell`` but array-encoded to keep the bucketed file small."""
    return [
        round(statistics.fmean(values), 4),
        round(statistics.median(values), 4),
        round(min(values), 4),
        round(max(values), 4),
        len(values),
    ]


# (local hour 0-23, is_weekend) per UTC instant, in Montréal time. Memoized within
# a year (timestamps repeat across stations/substances) to avoid re-running the
# tz conversion millions of times; cleared at the top of each year.
_LOCAL_HW: dict = {}


def local_hour_weekend(ts) -> tuple:
    hw = _LOCAL_HW.get(ts)
    if hw is None:
        loc = ts.astimezone(MONTREAL)
        hw = (loc.hour, loc.weekday() >= 5)
        _LOCAL_HW[ts] = hw
    return hw


def iqa_bundle_for_year(year: int) -> str | None:
    for path in glob.glob(os.path.join(RAW, "rsqa-indice-qualite-air-*.csv")):
        m = re.search(r"(\d{4})-(\d{4})", os.path.basename(path))
        if m and int(m.group(1)) <= year <= int(m.group(2)):
            return path
    return None


def main() -> None:
    coords = load_station_coords()

    gaz_files = {}
    for path in glob.glob(os.path.join(RAW, "rsqa-multi-polluants*.csv")):
        m = re.search(r"(\d{4})", os.path.basename(path))
        if m:
            gaz_files[int(m.group(1))] = path
    years = sorted(gaz_files)
    assert years, "no annual files found in data-src/ (run scripts/fetch-archive.sh)"

    os.makedirs(os.path.join(OUT, "series"), exist_ok=True)
    os.makedirs(os.path.join(OUT, "series-daily"), exist_ok=True)

    # Cross-year accumulators (small per entry).
    daily: dict[tuple, list] = defaultdict(lambda: [0.0, 0])  # (sid,sub,ordinal) -> [sum,count]
    all_acc: dict[tuple, dict] = {}                            # (sid,sub) -> {sum,min,max,n}
    map_stats: dict[str, dict] = {}                            # year|"all" -> sid -> sub -> cell
    detailed_stats: dict[str, dict] = {}                       # year -> sid -> sub -> {wd,we: [cell|null x24]}
    iqa_dom: dict[str, dict] = {}                              # year -> sid -> {...}
    station_years: dict[int, set] = defaultdict(set)
    present_subs: set[str] = set()
    min_ord = None
    max_ord = None
    min_utc = None
    max_utc = None
    total_rows = 0

    def add_value(sid, sub, ts, val, buf):
        nonlocal min_ord, max_ord, min_utc, max_utc
        buf[(sid, sub)].append((ts, val))
        o = ts.date().toordinal()
        cell = daily[(sid, sub, o)]
        cell[0] += val
        cell[1] += 1
        a = all_acc.get((sid, sub))
        if a is None:
            all_acc[(sid, sub)] = {"sum": val, "min": val, "max": val, "n": 1}
        else:
            a["sum"] += val
            a["min"] = min(a["min"], val)
            a["max"] = max(a["max"], val)
            a["n"] += 1
        present_subs.add(sub)
        station_years[sid].add(ts_year_local(ts))
        if min_ord is None or o < min_ord:
            min_ord = o
        if max_ord is None or o > max_ord:
            max_ord = o
        if min_utc is None or ts < min_utc:
            min_utc = ts
        if max_utc is None or ts > max_utc:
            max_utc = ts

    def ts_year_local(ts):
        return ts.astimezone(MONTREAL).year

    for year in years:
        buf: dict[tuple, list] = defaultdict(list)

        # ── multi-pollutant ──
        with open(gaz_files[year], encoding="utf-8-sig", newline="") as f:
            rows = list(csv.reader(f))
        hi, header = find_header(rows)
        if header is None:
            print(f"  WARNING: no header found in {os.path.basename(gaz_files[year])}, skipping")
            continue
        nh = [normcol(c) for c in header]
        station_idx = next((i for i, c in enumerate(nh) if c in STATION_COLS), 0)
        time_idx = next((i for i, c in enumerate(nh) if c in TIME_COLS), None)
        date_idx = next((i for i, c in enumerate(nh) if c == "date"), None)
        hour_idx = next((i for i, c in enumerate(nh) if c == "heure"), None)
        used = {station_idx, time_idx, date_idx, hour_idx}
        sub_cols = [
            (i, canon_poll(header[i])) for i in range(len(header))
            if i not in used and nh[i] and nh[i] not in IGNORE_COLS and nh[i] not in STATION_COLS
        ]
        for fields in rows[hi + 1:]:
            if not fields or station_idx >= len(fields) or not fields[station_idx].strip():
                continue
            try:
                sid = int(fields[station_idx].strip())
            except ValueError:
                continue
            if time_idx is not None and time_idx < len(fields):
                ts = parse_dt(fields[time_idx])
            elif date_idx is not None and hour_idx is not None \
                    and date_idx < len(fields) and hour_idx < len(fields):
                ts = parse_date_hour(fields[date_idx], fields[hour_idx])
            else:
                ts = None
            if ts is None:
                continue
            total_rows += 1
            for ci, sub in sub_cols:
                if ci >= len(fields):
                    continue
                v = num(fields[ci])
                if v is not None:
                    add_value(sid, sub, ts, v, buf)

        # ── IQA for this year (from the 3-year bundle covering it) ──
        bundle = iqa_bundle_for_year(year)
        if bundle:
            iqa_hour: dict[tuple, tuple] = {}  # (sid,ts) -> (val, poll)
            with open(bundle, encoding="utf-8-sig", newline="") as f:
                for row in csv.DictReader(f):
                    if row.get("date", "")[:4] != str(year):
                        continue
                    try:
                        sid = int(row["stationId"])
                        y, mo, d = (int(x) for x in row["date"].split("-"))
                        hh = int(row["heure"])
                        val = float(row["valeur"])
                    except (ValueError, KeyError):
                        continue
                    poll = IQA_POLL.get(row["polluant"], row["polluant"])
                    ts = (datetime(y, mo, d, 0, 0, tzinfo=MONTREAL) + timedelta(hours=hh)).astimezone(UTC)
                    k = (sid, ts)
                    cur = iqa_hour.get(k)
                    if cur is None or val > cur[0]:
                        iqa_hour[k] = (val, poll)
            dom_counts: dict[int, Counter] = defaultdict(Counter)
            peak: dict[int, tuple] = {}
            for (sid, ts), (val, poll) in iqa_hour.items():
                add_value(sid, IQA_KEY, ts, val, buf)
                dom_counts[sid][poll] += 1
                if sid not in peak or val > peak[sid][0]:
                    peak[sid] = (val, poll)
            if dom_counts:
                ydom = {}
                for sid, counts in dom_counts.items():
                    if sid not in coords:
                        continue
                    tot = sum(counts.values())
                    shares = sorted(([p, round(c / tot, 4)] for p, c in counts.items()), key=lambda x: -x[1])
                    ydom[str(sid)] = {
                        "peak_pollutant": peak[sid][1],
                        "peak_iqa": round(peak[sid][0], 4),
                        "shares": shares,
                    }
                iqa_dom[str(year)] = ydom

        # ── per-year map-stats + hourly station-year files ──
        ybase = year_base_utc(year)
        per_station_subs: dict[int, dict] = defaultdict(dict)
        ymap = map_stats.setdefault(str(year), {})
        ydet = detailed_stats.setdefault(str(year), {})
        _LOCAL_HW.clear()
        for (sid, sub), vals in buf.items():
            if sid not in coords:
                continue  # only emit data for mappable (coordinate-having) stations
            ymap.setdefault(str(sid), {})[sub] = stat_cell([v for _, v in vals])
            # Hour-of-day × weekday/weekend buckets in local Montréal time, so the
            # map can filter by time-of-day range and day type. Empty buckets → null.
            wd = [[] for _ in range(24)]
            we = [[] for _ in range(24)]
            for ts, v in vals:
                h, weekend = local_hour_weekend(ts)
                (we if weekend else wd)[h].append(v)
            ydet.setdefault(str(sid), {})[sub] = {
                "wd": [stat_cell_compact(b) if b else None for b in wd],
                "we": [stat_cell_compact(b) if b else None for b in we],
            }
            pairs = sorted(
                (int((ts - ybase).total_seconds() // 3600), round(v, 4)) for ts, v in vals
            )
            per_station_subs[sid][sub] = [[i, v] for i, v in pairs]
        for sid, subs in per_station_subs.items():
            if sid not in coords:
                continue
            payload = {
                "id": sid, "year": year,
                "start_utc": ybase.strftime("%Y-%m-%dT%H:%M:%SZ"),
                "step_secs": 3600, "substances": subs,
            }
            with open(os.path.join(OUT, "series", f"station-{sid}-{year}.json"), "w", encoding="utf-8") as f:
                json.dump(payload, f, ensure_ascii=False, separators=(",", ":"))
        del buf
        print(f"  {year}: stations {sorted(s for s in ymap.keys())}")

    # ── "all"-years map-stats (mean/min/max exact; median over daily means) ──
    base_ord = min_ord
    daily_means: dict[tuple, list] = defaultdict(list)  # (sid,sub) -> [(ordinal, mean)]
    for (sid, sub, o), (s, c) in daily.items():
        daily_means[(sid, sub)].append((o, s / c))
    allmap: dict[str, dict] = {}
    for (sid, sub), a in all_acc.items():
        if sid not in coords:
            continue
        dm = [m for _, m in daily_means[(sid, sub)]]
        allmap.setdefault(str(sid), {})[sub] = {
            "mean": round(a["sum"] / a["n"], 4),
            "median": round(statistics.median(dm), 4),
            "min": round(a["min"], 4),
            "max": round(a["max"], 4),
            "n": a["n"],
        }
    map_stats["all"] = allmap

    with open(os.path.join(OUT, "map-stats.json"), "w", encoding="utf-8") as f:
        json.dump(map_stats, f, ensure_ascii=False, separators=(",", ":"))
    # Detailed (hour × day-type) stats — numeric years only; the map iterates years
    # for any active filter, so no "all" fast-path layer is needed here.
    with open(os.path.join(OUT, "map-stats-detailed.json"), "w", encoding="utf-8") as f:
        json.dump(detailed_stats, f, ensure_ascii=False, separators=(",", ":"))
    with open(os.path.join(OUT, "iqa-dominance.json"), "w", encoding="utf-8") as f:
        json.dump(iqa_dom, f, ensure_ascii=False, separators=(",", ":"))

    # ── daily series per station (across all years) ──
    base_date = date.fromordinal(base_ord)
    by_station: dict[int, dict] = defaultdict(dict)
    for (sid, sub), pts in daily_means.items():
        pts.sort()
        by_station[sid][sub] = [[o - base_ord, round(m, 4)] for o, m in pts]
    for sid, subs in by_station.items():
        if sid not in coords:
            continue
        payload = {
            "id": sid, "start_date": base_date.isoformat(),
            "step_days": 1, "substances": subs,
        }
        with open(os.path.join(OUT, "series-daily", f"station-{sid}.json"), "w", encoding="utf-8") as f:
            json.dump(payload, f, ensure_ascii=False, separators=(",", ":"))

    # ── stations.json (union with coordinates) ──
    active_subs = [s for s in SUB_ORDER if s in present_subs]
    active_subs += [s for s in sorted(present_subs) if s not in active_subs]
    stations_out = []
    missing = []
    for sid in sorted(station_years):
        c = coords.get(sid)
        if c is None:
            missing.append(sid)
            continue
        # substances ever measured here, in display order
        subs_here = sorted(
            {sub for (s, sub) in all_acc if s == sid},
            key=lambda x: (active_subs.index(x) if x in active_subs else 999),
        )
        stations_out.append({
            "id": sid, "name": c["name"], "address": c["address"],
            "borough": c["borough"], "lat": round(c["lat"], 6), "lon": round(c["lon"], 6),
            "years": sorted(station_years[sid]), "substances": subs_here,
        })
    with open(os.path.join(OUT, "stations.json"), "w", encoding="utf-8") as f:
        json.dump(stations_out, f, ensure_ascii=False, separators=(",", ":"))

    # ── meta.json ──
    meta = {
        "years": years,
        "min_year": years[0], "max_year": years[-1], "latest_year": years[-1],
        "generated": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "daily_start": base_date.isoformat(),
        "rows": total_rows,
        "stations": len(stations_out),
        "substances": active_subs,
        "source_url": SOURCE_URL,
        "station_list_url": STATION_LIST_URL,
    }
    with open(os.path.join(OUT, "meta.json"), "w", encoding="utf-8") as f:
        json.dump(meta, f, ensure_ascii=False, indent=2)

    print(f"\nYears {years[0]}–{years[-1]} · {total_rows} rows · {len(stations_out)} mapped stations")
    print(f"Active substances: {', '.join(active_subs)}")
    if missing:
        print(f"Stations without coordinates (time-series only-capable, omitted): {missing}")


if __name__ == "__main__":
    main()
