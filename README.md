# AirQualityMTL

Web application for visualizing the **Réseau de surveillance de la qualité de
l'air (RSQA)** hourly multi-pollutant measurements from the City of Montréal's
network of fixed monitoring stations.

Production deployment: https://mtl-aq.org

Built in the same Rust/WebAssembly stack as [BikeStat](https://bikestat.org):
all rendering, aggregation, and interpolation run client-side; the server hosts
only static files.

## Views

1. **Map** — a colour-coded regional overlay of the **mean / median / maximum /
   minimum** for a selected **substance** over a chosen **year range** (From–To,
   defaulting to the whole record). Because the stations are sparse and don't cover
   the whole island, an inverse-distance-weighted (IDW) interpolated heatmap is
   painted between them, with the per-pixel opacity fading out away from the nearest
   station so the map never claims certainty where there are no sensors. Station
   markers (coloured by value) and a colour-bar legend sit on top; the range reflects
   the stations and substances active in those years. A **PNG copy/download** widget
   exports the composited map (basemap + heatmap + markers + legend).
2. **Time series** — concentration vs. time for a selected **station**,
   **substance**, and **aggregation interval** (Hour / Day / Week / Month / Year),
   over ranges up to the full 1986–2024 record, with a hover crosshair tooltip and
   PNG export. Long-range views (Day and coarser) use a daily-resolution tier; the
   Hour interval pulls hourly detail for the years in view. An optional **averaging
   profile** folds the selected range onto a short repeating base: **Weekday** and
   **Weekend** show the mean 24-hour diurnal cycle (from hourly data, so limited to
   the in-range hourly years), and **Weekly** shows the mean for each day of the
   week (Mon–Sun, from the daily tier, spanning the whole range).
3. **Network** — a bilingual long-form explainer on the RSQA monitoring network
   (coverage, pollutants, history, data caveats) with cited sources.
4. **Methodology** — this site's data sources (with attribution) and the exact
   processing steps behind the maps and charts.

Both article pages live as unstyled semantic HTML fragments in
`src/content/rsqa-{en,fr}.html` (Network) and `src/content/rsqa-methods-{en,fr}.html`
(Methodology), embedded at compile time and styled by the app's `.info-page` rules.

For the **IQA**, higher means worse: the map colours it on an absolute
acceptability scale (Good 0–25 / Acceptable 26–50 / Poor 50+) rather than the
relative ramp used for raw concentrations, and the time-series chart marks the
25 and 50 thresholds with reference lines.

Both views carry a self-describing **parameter caption** (the map's colour-bar
title shows substance · statistic · year-range; the chart shows station ·
substance · interval · date-range), and that caption is baked into the PNG
exports so a saved image is interpretable on its own. Shared export plumbing
lives in `src/components/export.rs`.

Bilingual interface (English / French), browser-detected and toggled in-page;
the preference is persisted in `localStorage`.

## Data

The app covers the **full published archive, 1986–2024**, from three open
datasets:

| Source | Files |
|---|---|
| [RSQA multi-pollutant](https://donnees.montreal.ca/dataset/rsqa-polluants-gazeux) | 39 annual hourly CSVs, 1986–2024 |
| [RSQA station list](https://donnees.montreal.ca/dataset/rsqa-liste-des-stations) | station names, boroughs, coordinates |
| [RSQA Air Quality Index (IQA)](https://donnees.montreal.ca/dataset/rsqa-iqa-historique) | "détaillé par station" 3-year bundles, 2007–present (per-pollutant sub-indices) |

`scripts/fetch-archive.sh` downloads everything into `data-src/` (re-runs skip
files already present). The portal's `robots.txt` permits the resource
`download/` URLs (only `/api/` is blocked, with a 10 s crawl-delay), so **no
manual download is required**.

The annual files drift in schema across four decades — station/time column
names, date format and hour convention (start-of-hour ≤2012, end-of-hour 2013+),
pollutant spellings (`PM25`/`PM2.5`/`PM2,5`), split date/hour columns, a stray
leading blank line, malformed coordinates — so `scripts/preprocess.py` detects
columns by name, normalizes spellings, parses both date formats, and bounds
coordinates to the Montréal region. It emits compact files under `static/data/`:

- `stations.json` — station metadata, the years each reported, and its substances
  (union of stations that appear in any year and have valid coordinates).
- `map-stats.json` — `{year|"all" → station → substance → {mean, median, min, max, n}}`,
  powering the map + year selector instantly (no client-side CSV parsing).
- `series-daily/station-<id>.json` — one **daily** mean per substance spanning all
  years; drives Day/Week/Month/Year chart intervals over long ranges. **Committed.**
- `series/station-<id>-<year>.json` — **hourly** values per station-year, loaded on
  demand for the Hour interval. Large in aggregate (~250 MB) → **git-ignored**,
  generated by `preprocess.py` and shipped via `deploy.sh`'s rsync.
- `iqa-dominance.json` — per year/station, the index's driving pollutant.
- `meta.json` — years list / latest year / generation stamp / attribution.

The **Air Quality Index (IQA)** is folded in as a synthetic substance `IQA`: the
IQA file gives the City's pre-computed per-pollutant sub-indices, and the
preprocessor takes the **maximum across pollutants per station-hour** (the
official definition), so no unit conversion or reference constants are involved.
It leads the substance picker and is the default metric. The dominant pollutant
is surfaced in the map marker tooltip as the year-round *main driver* (with its
share of hours) or, under the Maximum stat, the *peak-hour driver*. IQA is
available from 2007 (when the City began publishing it).

Timestamps are converted to UTC (Montréal-local, DST-aware) and the chart renders
Montréal local. Coverage is uneven by design and evolves over the decades —
stations open/close and instruments change (legacy COH in early years; black
carbon from 2017, ultrafine particles from 2021) — so the year selector and
substance picker adapt to what each year and station actually reported.

## Stack

- Rust + Leptos 0.7 client-side rendering, compiled to WebAssembly via Trunk.
- No JavaScript beyond the `index.html` scaffolding Trunk requires.
- SVG time-series chart; HTML `<canvas>` IDW heatmap over Web-Mercator CARTO
  tiles; everything else is declarative Leptos.

## Repository layout

```
src/
  lib.rs              App root: view toggle, signal wiring, data-load orchestration
  i18n.rs             EN / FR translation table
  data/
    types.rs          Domain types (Station, Reading, Stat, Interval, MapStat, View)
    pollutants.rs     Pollutant catalogue (display names EN/FR + units)
    loader.rs         Fetch + parse JSON; mean aggregation
  components/
    chart.rs          SVG line/area chart with hover crosshair + PNG export
    map.rs            IDW heatmap (canvas) + tiles + graduated markers + colour bar
    controls.rs       Filter sidebar (substance / statistic / station / interval / dates)
    info.rs           Network / Methodology views — renders the embedded fragments
  content/
    rsqa-en.html, rsqa-fr.html              Network page content (EN/FR)
    rsqa-methods-en.html, rsqa-methods-fr.html  Methodology page content (EN/FR)
scripts/
  fetch-archive.sh    Download the 1986–2024 archive + IQA bundles into data-src/
  preprocess.py       Raw CSVs → compact JSON (schema-tolerant, all years)
  animate.py          Local: render map-overlay animation frames + MP4 per time bucket
  deploy.sh           [--download] + preprocess + trunk build --release + rsync
data-src/             Raw input CSVs (git-ignored; re-downloadable)
static/
  style.css, favicon.svg
  data/               Compact JSON served to the app (preprocess.py output)
                      series-daily/ committed; series/ (hourly) git-ignored
```

## Local development

```bash
scripts/fetch-archive.sh          # download the archive into data-src/ (once, ~400 MB)
python3 scripts/preprocess.py     # regenerate static/data/*
trunk serve                       # compile WASM + live-reload at http://localhost:8080
```

The committed `static/data/` (map-stats + daily series) is enough to run the app
without the archive; `fetch-archive.sh` + `preprocess.py` are only needed to
regenerate data or to produce the hourly tier for the Hour interval.

## Deployment

```bash
./scripts/deploy.sh            # code-only: build + rsync the app (fast)
./scripts/deploy.sh --data     # also regenerate + sync the data
./scripts/deploy.sh --download # fetch/refresh the raw archive first, then --data
```

Default deploys are **code-only**: they build `trunk build --release` and rsync
`dist/` while leaving the data already on the server in place — they don't re-run
`preprocess.py` or touch the large data tiers (the hourly series alone is
~250 MB), so a code change ships quickly. `--data` regenerates `static/data` from
`data-src/` and syncs it with `rsync --checksum` (preprocess rewrites every file
each run, so a checksum compare avoids re-sending the byte-identical ones — only a
newly-added year and the summaries actually transfer). `--download` fetches the
raw archive first. Target defaults to `rhoge@bikestat.org:/var/www/mtl-aq/` (the
same VPS as BikeStat, own vhost); override with `AIRQUALITY_REMOTE` /
`AIRQUALITY_DEST`.

When the City publishes a new year, add its resource URL to `fetch-archive.sh`,
then `./scripts/deploy.sh --download` regenerates and ships it — a server cron
could automate this, as the live BikeStat feeds do.

## Animations (local tool)

`scripts/animate.py` renders a time-series of map frames — one per time bucket,
each the bucket-mean of a substance/index IDW-interpolated over the island — and
stitches them into an MP4. It's a **local-only** tool (not part of the WASM
build): it mirrors the web map's rendering (CARTO basemap, the same colour ramps
/ IQA bands, coverage fade, markers, colour-bar) on a **colour scale fixed across
all frames** so change over time is readable, and stamps each frame with its
date. It reads the committed daily tier, so day/week/month/year buckets work
without the raw archive.

```bash
pip3 install pillow numpy            # one-time (ffmpeg optional, for MP4)
python3 scripts/animate.py --substance PM2.5 --bucket week --from 2023-01-01 --to 2023-12-31
python3 scripts/animate.py --substance IQA   --bucket month
python3 scripts/animate.py --substance O3    --bucket week --from 2024-01-01
```

`--bucket` is `week` (default) / `month` / `year` / `<N>d`; `--from`/`--to`
default to the substance's full extent (weekly over 1986–2024 is ~2,000 frames,
so narrow the range or use a bigger bucket). Other flags: `--fps`,
`--width`/`--height`, `--vmin`/`--vmax` (pin the scale), `--no-basemap`,
`--out`. Output lands in `anim/<substance>_<bucket>/frame_NNNNN.png` plus an MP4
(both git-ignored); basemap tiles are cached under `scripts/.tilecache/`.

## Attribution

Data © Ville de Montréal (RSQA), distributed under the
[Creative Commons CC-BY 4.0](https://creativecommons.org/licenses/by/4.0/)
licence. Base map © OpenStreetMap contributors © CARTO.
