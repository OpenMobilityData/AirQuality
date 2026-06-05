use chrono::{Datelike, Duration, NaiveDate, Timelike, Weekday};
use chrono_tz::America::Montreal as MontrealTz;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};

use std::collections::BTreeMap;

use crate::data::loader::{DailySeries, IqaDominanceMap, SeriesFile};
use crate::data::pollutants;
use crate::data::types::{DayType, IqaDominance, MapStat, Stat, Station};
use crate::i18n::Lang;

/// The map's per-station aggregated slice: `station id -> substance -> MapStat`,
/// computed client-side over the selected date range from the daily/hourly tier.
type MapSlice = BTreeMap<String, BTreeMap<String, MapStat>>;

/// Synthetic substance key for the Air Quality Index (mirrors `IQA_KEY` in the
/// preprocessor); the dominant-pollutant detail only applies to it.
const IQA_KEY: &str = "IQA";

const TILE_SIZE: f64 = 256.0;
/// Markers/heatmap are scaled to fill this fraction of the smaller viewport axis.
const FILL_FRACTION: f64 = 0.82;
const MAX_FILL_SCALE: f64 = 2.6;
/// Padding (fraction of span) added around the station bounding box.
const BBOX_PAD: f64 = 0.14;
/// IDW influence radius — alpha fades to 0 this far from the nearest station.
/// Sized so adjacent central-island stations (~3–5 km apart) blend into a
/// continuous field while isolated outliers still fade out honestly.
const COVERAGE_KM: f64 = 6.0;
/// Peak overlay opacity directly over a station (0–255).
const BASE_ALPHA: f64 = 150.0;
/// Downscale factor for the heatmap render (computed coarse, drawn smooth).
const HEATMAP_STEP: f64 = 4.0;

// ── Colour ramp (ColorBrewer RdYlBu, reversed: cool=low, warm=high) ──────────

const RAMP: &[(f64, (u8, u8, u8))] = &[
    (0.00, (0x2c, 0x7b, 0xb6)),
    (0.25, (0xab, 0xd9, 0xe9)),
    (0.50, (0xff, 0xff, 0xbf)),
    (0.75, (0xfd, 0xae, 0x61)),
    (1.00, (0xd7, 0x19, 0x1c)),
];

/// CSS gradient string matching `RAMP`, for the colour-bar legend.
pub const RAMP_CSS: &str =
    "linear-gradient(90deg,#2c7bb6,#abd9e9,#ffffbf,#fdae61,#d7191c)";

fn ramp_rgb(t: f64) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    for w in RAMP.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            let lerp = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * f).round() as u8;
            return (lerp(c0.0, c1.0), lerp(c0.1, c1.1), lerp(c0.2, c1.2));
        }
    }
    RAMP[RAMP.len() - 1].1
}

fn ramp_hex(t: f64) -> String {
    let (r, g, b) = ramp_rgb(t);
    format!("#{r:02x}{g:02x}{b:02x}")
}

// ── IQA absolute colour scale (anchored to the official acceptability bands) ──
//
// Unlike raw concentrations (coloured on a relative min–max ramp), the index is
// coloured on a fixed value scale so a colour means the same thing everywhere:
// green (good, 0–25) → yellow/orange (acceptable, 26–50) → red (poor, 50+).
// Higher always reads warmer = worse.
const IQA_GOOD_MAX: f64 = 25.0;
const IQA_ACCEPTABLE_MAX: f64 = 50.0;
const IQA_STOPS: &[(f64, (u8, u8, u8))] = &[
    (0.0, (0x1a, 0x98, 0x50)),
    (25.0, (0xd9, 0xef, 0x8b)),
    (37.5, (0xfe, 0xe0, 0x8b)),
    (50.0, (0xfd, 0xae, 0x61)),
    (75.0, (0xf4, 0x6d, 0x43)),
    (100.0, (0xd7, 0x30, 0x27)),
];

fn iqa_color_rgb(v: f64) -> (u8, u8, u8) {
    let v = v.clamp(0.0, 100.0);
    for w in IQA_STOPS.windows(2) {
        let (v0, c0) = w[0];
        let (v1, c1) = w[1];
        if v <= v1 {
            let f = if v1 > v0 { (v - v0) / (v1 - v0) } else { 0.0 };
            let lerp = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * f).round() as u8;
            return (lerp(c0.0, c1.0), lerp(c0.1, c1.1), lerp(c0.2, c1.2));
        }
    }
    IQA_STOPS[IQA_STOPS.len() - 1].1
}

fn iqa_color_hex(v: f64) -> String {
    let (r, g, b) = iqa_color_rgb(v);
    format!("#{r:02x}{g:02x}{b:02x}")
}

// ── Web-Mercator projection (same math as BikeStat's map) ───────────────────

fn lat_lon_to_tile(lat: f64, lon: f64, zoom: u32) -> (f64, f64) {
    let n = (1u64 << zoom) as f64;
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

fn pick_zoom(lat0: f64, lat1: f64, lon0: f64, lon1: f64, w: f64, h: f64) -> u32 {
    for z in (0..=16u32).rev() {
        let (x0, y_top) = lat_lon_to_tile(lat1, lon0, z);
        let (x1, y_bot) = lat_lon_to_tile(lat0, lon1, z);
        if (x1 - x0) * TILE_SIZE <= w && (y_bot - y_top) * TILE_SIZE <= h {
            return z;
        }
    }
    0
}

/// Projection geometry shared by the declarative tile/marker layer and the
/// imperative canvas heatmap so they stay pixel-aligned.
#[derive(Clone, Copy)]
struct Geom {
    zoom: u32,
    cx_tile: f64,
    cy_tile: f64,
    eff_tile: f64,
    w: f64,
    h: f64,
    /// Metres per screen pixel — used for the heatmap's distance-based fade.
    m_per_px: f64,
}

impl Geom {
    fn screen(&self, lat: f64, lon: f64) -> (f64, f64) {
        let (tx, ty) = lat_lon_to_tile(lat, lon, self.zoom);
        (
            self.w / 2.0 + (tx - self.cx_tile) * self.eff_tile,
            self.h / 2.0 + (ty - self.cy_tile) * self.eff_tile,
        )
    }
}

fn compute_geom(stations: &[Station], w: f64, h: f64) -> Option<Geom> {
    if stations.is_empty() || w < 1.0 || h < 1.0 {
        return None;
    }
    let lats: Vec<f64> = stations.iter().map(|s| s.lat).collect();
    let lons: Vec<f64> = stations.iter().map(|s| s.lon).collect();
    let lat0 = lats.iter().cloned().fold(f64::INFINITY, f64::min);
    let lat1 = lats.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let lon0 = lons.iter().cloned().fold(f64::INFINITY, f64::min);
    let lon1 = lons.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let (cy, cx) = ((lat0 + lat1) / 2.0, (lon0 + lon1) / 2.0);
    let lat_half = ((lat1 - lat0).max(0.01)) * (0.5 + BBOX_PAD);
    let lon_half = ((lon1 - lon0).max(0.01)) * (0.5 + BBOX_PAD);
    let (blat0, blat1, blon0, blon1) =
        (cy - lat_half, cy + lat_half, cx - lon_half, cx + lon_half);

    let zoom = pick_zoom(blat0, blat1, blon0, blon1, w, h);
    let (mx0, my0) = lat_lon_to_tile(blat1, blon0, zoom);
    let (mx1, my1) = lat_lon_to_tile(blat0, blon1, zoom);
    let box_w = ((mx1 - mx0) * TILE_SIZE).max(1.0);
    let box_h = ((my1 - my0) * TILE_SIZE).max(1.0);
    let scale = (w * FILL_FRACTION / box_w)
        .min(h * FILL_FRACTION / box_h)
        .clamp(1.0, MAX_FILL_SCALE);
    let eff_tile = TILE_SIZE * scale;

    let (cx_tile, cy_tile) = lat_lon_to_tile(cy, cx, zoom);
    // Web-Mercator ground resolution (m per 256-tile-pixel) ÷ our extra scale.
    let m_per_tile_px = 156_543.033_928 * cy.to_radians().cos() / (1u64 << zoom) as f64;
    let m_per_px = m_per_tile_px / scale;

    Some(Geom { zoom, cx_tile, cy_tile, eff_tile, w, h, m_per_px })
}

/// `(stat value)` for a station/substance within the aggregated slice, or `None`.
fn station_value(ys: &MapSlice, id: u32, substance: &str, stat: Stat) -> Option<f64> {
    ys.get(&id.to_string())
        .and_then(|m| m.get(substance))
        .map(|s| stat.value(s))
}

fn median_of(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = xs.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    }
}

fn is_weekend(wd: Weekday) -> bool {
    matches!(wd, Weekday::Sat | Weekday::Sun)
}

/// Does this day pass the day-type filter?
fn day_type_ok(day_type: DayType, weekend: bool) -> bool {
    match day_type {
        DayType::All => true,
        DayType::Weekday => !weekend,
        DayType::Weekend => weekend,
    }
}

/// Aggregate the **daily tier** over `[from, to]` (full-day path). For each
/// station/substance, combine the per-day cells whose date falls in range and
/// passes the day-type filter: mean is sample-weighted by each day's hourly
/// count, min/max are the true hourly extremes, and the median is the median of
/// the daily means (an approximation — exact hourly medians need the hourly tier).
fn aggregate_daily(
    daily: &BTreeMap<u32, DailySeries>,
    from: NaiveDate,
    to: NaiveDate,
    day_type: DayType,
) -> MapSlice {
    let mut out: MapSlice = BTreeMap::new();
    for (sid, ds) in daily {
        let Some(base) = ds.base_date() else { continue };
        let mut sub_map: BTreeMap<String, MapStat> = BTreeMap::new();
        for (sub, cells) in &ds.substances {
            // (Σ mean·n, min, max, Σ n, [daily means])
            let mut sum_mean_n = 0.0_f64;
            let mut mn = f64::INFINITY;
            let mut mx = f64::NEG_INFINITY;
            let mut n: u64 = 0;
            let mut means: Vec<f64> = Vec::new();
            for (idx, mean, cmin, cmax, cn) in cells {
                let date = base + Duration::days(*idx);
                if date < from || date > to {
                    continue;
                }
                if !day_type_ok(day_type, is_weekend(date.weekday())) {
                    continue;
                }
                sum_mean_n += mean * *cn as f64;
                mn = mn.min(*cmin);
                mx = mx.max(*cmax);
                n += *cn as u64;
                means.push(*mean);
            }
            if n == 0 {
                continue;
            }
            sub_map.insert(
                sub.clone(),
                MapStat { mean: sum_mean_n / n as f64, median: median_of(means), min: mn, max: mx, n: n as u32 },
            );
        }
        if !sub_map.is_empty() {
            out.insert(sid.to_string(), sub_map);
        }
    }
    out
}

/// Aggregate the **hourly tier** (used when a time-of-day window is active).
/// For each loaded station-year, keep readings whose Montréal-local date is in
/// `[from, to]`, whose local hour is in `[hour_from, hour_to]` (inclusive), and
/// that pass the day-type filter; then compute exact mean/median/min/max per
/// station/substance. Empty until the needed hourly files finish loading.
fn aggregate_hourly(
    hourly: &BTreeMap<(u32, i32), SeriesFile>,
    from: NaiveDate,
    to: NaiveDate,
    hour_from: u8,
    hour_to: u8,
    day_type: DayType,
) -> MapSlice {
    // (sid, sub) -> all matching hourly values, for exact stats.
    let mut acc: BTreeMap<(u32, String), Vec<f64>> = BTreeMap::new();
    for ((sid, _year), sf) in hourly {
        for sub in sf.substances.keys() {
            for r in sf.readings(sub) {
                let local = r.timestamp.with_timezone(&MontrealTz);
                let d = local.date_naive();
                if d < from || d > to {
                    continue;
                }
                let h = local.hour() as u8;
                if h < hour_from || h > hour_to {
                    continue;
                }
                if !day_type_ok(day_type, is_weekend(local.weekday())) {
                    continue;
                }
                acc.entry((*sid, sub.clone())).or_default().push(r.value);
            }
        }
    }
    let mut out: MapSlice = BTreeMap::new();
    for ((sid, sub), vals) in acc {
        if vals.is_empty() {
            continue;
        }
        let n = vals.len() as u32;
        let sum: f64 = vals.iter().sum();
        let mn = vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let mx = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        out.entry(sid.to_string()).or_default().insert(
            sub,
            MapStat { mean: sum / n as f64, median: median_of(vals), min: mn, max: mx, n },
        );
    }
    out
}

/// Combine IQA dominance across `[from, to]`: the year-range main driver is the
/// pollutant with the highest average share; the peak is the worst single hour.
fn aggregate_dominance(dom: &IqaDominanceMap, from: i32, to: i32) -> BTreeMap<String, IqaDominance> {
    // per sid: pollutant -> (Σ share, count), plus (peak_iqa, peak_pollutant)
    let mut share_acc: BTreeMap<String, BTreeMap<String, (f64, u32)>> = BTreeMap::new();
    let mut peak: BTreeMap<String, (f64, String)> = BTreeMap::new();
    for (yk, ydom) in dom {
        let Ok(y) = yk.parse::<i32>() else { continue };
        if y < from || y > to {
            continue;
        }
        for (sid, d) in ydom {
            let m = share_acc.entry(sid.clone()).or_default();
            for (poll, share) in &d.shares {
                let e = m.entry(poll.clone()).or_insert((0.0, 0));
                e.0 += *share;
                e.1 += 1;
            }
            let p = peak.entry(sid.clone()).or_insert((f64::NEG_INFINITY, String::new()));
            if d.peak_iqa > p.0 {
                *p = (d.peak_iqa, d.peak_pollutant.clone());
            }
        }
    }
    let mut out = BTreeMap::new();
    for (sid, polls) in share_acc {
        let mut shares: Vec<(String, f64)> =
            polls.into_iter().map(|(p, (s, c))| (p, s / c.max(1) as f64)).collect();
        shares.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let (peak_iqa, peak_pollutant) = peak.get(&sid).cloned().unwrap_or((0.0, String::new()));
        out.insert(sid, IqaDominance { peak_pollutant, peak_iqa, shares });
    }
    out
}

/// Paint the IDW heatmap onto `canvas` for the current (substance, stat).
fn draw_heatmap(
    canvas: &web_sys::HtmlCanvasElement,
    geom: &Geom,
    points: &[(f64, f64, f64)], // (screen_x, screen_y, value)
    vmin: f64,
    vmax: f64,
    is_iqa: bool,
) {
    let (w, h) = (geom.w, geom.h);
    canvas.set_width(w as u32);
    canvas.set_height(h as u32);
    let Some(ctx) = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
    else {
        return;
    };
    ctx.clear_rect(0.0, 0.0, w, h);

    // Need ≥2 stations for a meaningful surface. A degenerate value range only
    // matters for the relative ramp; the IQA scale is absolute.
    if points.len() < 2 || (!is_iqa && vmax <= vmin) {
        return;
    }

    let gw = (w / HEATMAP_STEP).ceil().max(1.0) as u32;
    let gh = (h / HEATMAP_STEP).ceil().max(1.0) as u32;
    let coverage_px = COVERAGE_KM * 1000.0 / geom.m_per_px;
    let span = (vmax - vmin).max(1e-9);

    let mut data = vec![0u8; (gw * gh * 4) as usize];
    for j in 0..gh {
        let py = (j as f64 + 0.5) * (h / gh as f64);
        for i in 0..gw {
            let px = (i as f64 + 0.5) * (w / gw as f64);
            let mut num = 0.0;
            let mut den = 0.0;
            let mut nearest = f64::INFINITY;
            for (sx, sy, val) in points {
                let d2 = (px - sx).powi(2) + (py - sy).powi(2);
                if d2 < nearest {
                    nearest = d2;
                }
                let wgt = 1.0 / (d2 + 1.0);
                num += wgt * val;
                den += wgt;
            }
            let v = num / den;
            let (r, g, b) = if is_iqa {
                iqa_color_rgb(v)
            } else {
                ramp_rgb(((v - vmin) / span).clamp(0.0, 1.0))
            };
            // Smoothstep fade with distance from the nearest station.
            let nd = nearest.sqrt() / coverage_px;
            let f = (1.0 - nd).clamp(0.0, 1.0);
            let f = f * f * (3.0 - 2.0 * f);
            let a = (BASE_ALPHA * f).round() as u8;
            let idx = ((j * gw + i) * 4) as usize;
            data[idx] = r;
            data[idx + 1] = g;
            data[idx + 2] = b;
            data[idx + 3] = a;
        }
    }

    // Render the coarse grid to a small offscreen canvas, then upscale smoothly.
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let Some(small) = document
        .create_element("canvas")
        .ok()
        .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok())
    else {
        return;
    };
    small.set_width(gw);
    small.set_height(gh);
    let Some(sctx) = small
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok())
    else {
        return;
    };
    if let Ok(image_data) =
        web_sys::ImageData::new_with_u8_clamped_array_and_sh(Clamped(&data), gw, gh)
    {
        let _ = sctx.put_image_data(&image_data, 0.0, 0.0);
        ctx.set_image_smoothing_enabled(true);
        let _ = ctx.draw_image_with_html_canvas_element_and_dw_and_dh(&small, 0.0, 0.0, w, h);
    }
}

/// Composite the live map (basemap tiles + heatmap canvas) plus freshly-drawn
/// markers, colour-bar legend, and a parameter caption into a PNG `Blob`.
/// Tiles are drawn from the on-screen `<img>`s (loaded `crossorigin`) so the
/// canvas isn't tainted; everything else is redrawn from state for crispness.
#[allow(clippy::too_many_arguments)]
async fn build_map_png_blob(
    container: web_sys::Element,
    heatmap: web_sys::HtmlCanvasElement,
    w: f64,
    h: f64,
    stations: Vec<Station>,
    ys: MapSlice,
    dom: BTreeMap<String, IqaDominance>,
    substance: String,
    stat: Stat,
    lang: Lang,
    range_label: String,
) -> Result<web_sys::Blob, JsValue> {
    let _ = dom; // dominance is summarized in the on-screen markers; not redrawn here
    let document = web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let Some(geom) = compute_geom(&stations, w, h) else {
        return Err(JsValue::from_str("no geometry"));
    };
    let is_iqa = substance == IQA_KEY;
    let unit = pollutants::unit_of(&substance);

    // Value domain across stations for the relative ramp.
    let vals: Vec<f64> = stations
        .iter()
        .filter_map(|s| station_value(&ys, s.id, &substance, stat))
        .collect();
    let (vmin, vmax) = (
        vals.iter().cloned().fold(f64::INFINITY, f64::min),
        vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    let span = (vmax - vmin).max(1e-9);
    let n_meas = vals.len();
    // Unweighted mean of the per-station values — the legend's "Station average".
    let mean = if n_meas > 0 { vals.iter().sum::<f64>() / n_meas as f64 } else { 0.0 };
    let avg_note = {
        let v = crate::components::chart::fmt_val(mean);
        if unit.is_empty() {
            format!("{}: {v}", lang.t().map_avg)
        } else {
            format!("{}: {v} {unit}", lang.t().map_avg)
        }
    };

    let cap_h = 34.0_f64;
    let total_h = h + cap_h;
    let scale = 2u32;
    let canvas = document.create_element("canvas")?.dyn_into::<web_sys::HtmlCanvasElement>()?;
    canvas.set_width(w as u32 * scale);
    canvas.set_height(total_h as u32 * scale);
    let ctx = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into::<web_sys::CanvasRenderingContext2d>()?;
    ctx.scale(scale as f64, scale as f64)?;

    // Background
    ctx.set_fill_style_str("#0a1628");
    ctx.fill_rect(0.0, 0.0, w, total_h);

    // Basemap tiles (from the live, CORS-clean <img>s)
    let cont_rect = container.get_bounding_client_rect();
    let tiles = container.query_selector_all(".map-tile")?;
    for i in 0..tiles.length() {
        let Some(node) = tiles.item(i) else { continue };
        let Ok(img) = node.dyn_into::<web_sys::HtmlImageElement>() else { continue };
        let r = img.get_bounding_client_rect();
        let _ = ctx.draw_image_with_html_image_element_and_dw_and_dh(
            &img,
            r.left() - cont_rect.left(),
            r.top() - cont_rect.top(),
            r.width(),
            r.height(),
        );
    }

    // Heatmap overlay
    let _ = ctx.draw_image_with_html_canvas_element_and_dw_and_dh(&heatmap, 0.0, 0.0, w, h);

    // Markers
    for s in &stations {
        let (sx, sy) = geom.screen(s.lat, s.lon);
        let val = station_value(&ys, s.id, &substance, stat);
        ctx.begin_path();
        ctx.arc(sx, sy, 6.0, 0.0, std::f64::consts::TAU)?;
        match val {
            Some(v) => {
                let hex = if is_iqa {
                    iqa_color_hex(v)
                } else {
                    ramp_hex(((v - vmin) / span).clamp(0.0, 1.0))
                };
                ctx.set_fill_style_str(&hex);
                ctx.fill();
                ctx.set_line_width(2.0);
                ctx.set_stroke_style_str("#ffffff");
                ctx.stroke();
            }
            None => {
                ctx.set_line_width(1.5);
                ctx.set_stroke_style_str("#6a7488");
                ctx.stroke();
            }
        }
    }

    // Colour-bar legend (bottom-left), mirroring the on-screen one.
    let t = lang.t();
    let title = format!("{} · {} · {}", pollutants::name_of(&substance, lang), stat.label(lang), range_label);
    let (bx, bw, bh) = (10.0_f64, 210.0_f64, 74.0_f64);
    let by = h - bh - 14.0;
    ctx.set_fill_style_str("rgba(13,27,42,0.85)");
    ctx.fill_rect(bx, by, bw, bh);
    ctx.set_stroke_style_str("#2a3a5c");
    ctx.set_line_width(1.0);
    ctx.stroke_rect(bx, by, bw, bh);
    ctx.set_fill_style_str("#eaeaea");
    ctx.set_font("11px Inter, system-ui, sans-serif");
    ctx.set_text_align("left");
    let _ = ctx.fill_text(&title, bx + 8.0, by + 15.0);

    let (gx, gy, gw, gh) = (bx + 8.0, by + 22.0, bw - 16.0, 9.0);
    if is_iqa {
        // Three acceptability bands.
        let bands = [
            iqa_color_hex(IQA_GOOD_MAX * 0.4),
            iqa_color_hex((IQA_GOOD_MAX + IQA_ACCEPTABLE_MAX) / 2.0),
            iqa_color_hex(IQA_ACCEPTABLE_MAX * 1.6),
        ];
        for (k, c) in bands.iter().enumerate() {
            ctx.set_fill_style_str(c);
            ctx.fill_rect(gx + gw * k as f64 / 3.0, gy, gw / 3.0 + 1.0, gh);
        }
        ctx.set_fill_style_str("#8892a4");
        ctx.set_font("9px Inter, system-ui, sans-serif");
        let _ = ctx.fill_text(&format!("{} · {} · {}", t.iqa_good, t.iqa_acceptable, t.iqa_poor), gx, gy + gh + 11.0);
        let _ = ctx.fill_text(&format!("{} · {} {}", t.iqa_higher_worse, n_meas, t.stations_measuring), gx, gy + gh + 22.0);
        ctx.set_fill_style_str("#eaeaea");
        ctx.set_font("11px Inter, system-ui, sans-serif");
        let _ = ctx.fill_text(&avg_note, gx, gy + gh + 36.0);
    } else {
        // Relative ramp drawn as thin slices.
        let slices = 40;
        for k in 0..slices {
            let tt = k as f64 / (slices - 1) as f64;
            ctx.set_fill_style_str(&ramp_hex(tt));
            ctx.fill_rect(gx + gw * k as f64 / slices as f64, gy, gw / slices as f64 + 1.0, gh);
        }
        ctx.set_fill_style_str("#8892a4");
        ctx.set_font("9px Inter, system-ui, sans-serif");
        use crate::components::chart::fmt_val;
        let _ = ctx.fill_text(&fmt_val(vmin), gx, gy + gh + 11.0);
        ctx.set_text_align("right");
        let _ = ctx.fill_text(&fmt_val(vmax), gx + gw, gy + gh + 11.0);
        ctx.set_text_align("left");
        let unit_note = if unit.is_empty() {
            format!("{n_meas} {}", t.stations_measuring)
        } else {
            format!("{unit} · {n_meas} {}", t.stations_measuring)
        };
        let _ = ctx.fill_text(&unit_note, gx, gy + gh + 22.0);
        ctx.set_fill_style_str("#eaeaea");
        ctx.set_font("11px Inter, system-ui, sans-serif");
        let _ = ctx.fill_text(&avg_note, gx, gy + gh + 36.0);
    }

    // Caption strip (guarantees the parameters are present even with no data).
    ctx.set_fill_style_str("#0d1b2a");
    ctx.fill_rect(0.0, h, w, cap_h);
    ctx.set_fill_style_str("#eaeaea");
    ctx.set_font("13px Inter, system-ui, sans-serif");
    ctx.set_text_align("left");
    let _ = ctx.fill_text(&title, 12.0, h + 21.0);
    ctx.set_fill_style_str("#8892a4");
    ctx.set_font("10px Inter, system-ui, sans-serif");
    ctx.set_text_align("right");
    let _ = ctx.fill_text("© OpenStreetMap © CARTO · RSQA", w - 10.0, h + 21.0);

    crate::components::export::canvas_to_png_blob(&canvas).await
}

#[component]
pub fn RegionMap(
    stations: ReadSignal<Vec<Station>>,
    /// Daily tier (one enriched file per station), shared with the Series view.
    /// Drives the default full-day date-range averaging.
    daily_cache: ReadSignal<BTreeMap<u32, DailySeries>>,
    /// Hourly tier (per station-year), shared with the Series view. Read only
    /// when a time-of-day window is active, to compute exact sub-day stats.
    hourly_cache: ReadSignal<BTreeMap<(u32, i32), SeriesFile>>,
    iqa_dominance: ReadSignal<IqaDominanceMap>,
    /// Inclusive date range [from, to] averaged by the map.
    date_from: ReadSignal<NaiveDate>,
    date_to: ReadSignal<NaiveDate>,
    substance: ReadSignal<String>,
    stat: ReadSignal<Stat>,
    /// Inclusive local-hour range [hour_from, hour_to] (0..23) and day-type
    /// filter. At the full-day default `[0, 23]` the map aggregates the daily
    /// tier; any narrowed hour window switches to the hourly tier (empty until
    /// the needed station-year files finish loading, so the map shows no data
    /// rather than misleading unfiltered values).
    hour_from: ReadSignal<u8>,
    hour_to: ReadSignal<u8>,
    day_type: ReadSignal<DayType>,
) -> impl IntoView {
    let lang = use_context::<ReadSignal<Lang>>().expect("Lang context not provided");

    // The date range's aggregated slice. Memoised so switching substance or
    // statistic (which don't affect it) doesn't re-run the heavy aggregation over
    // every station's daily/hourly data — only a date/hour/day-type change does.
    // Full-day window → daily tier; any hour window → hourly tier.
    let slice = Memo::new(move |_| -> MapSlice {
        let (from, to) = (date_from.get(), date_to.get());
        let (hf, ht, dt) = (hour_from.get(), hour_to.get(), day_type.get());
        if hf == 0 && ht == 23 {
            daily_cache.with(|d| aggregate_daily(d, from, to, dt))
        } else {
            hourly_cache.with(|h| aggregate_hourly(h, from, to, hf, ht, dt))
        }
    });
    let year_stats = move || slice.get();
    let year_dom = move || -> BTreeMap<String, IqaDominance> {
        aggregate_dominance(&iqa_dominance.get(), date_from.get().year(), date_to.get().year())
    };
    // Human-readable date-range label for the colour-bar / export caption.
    let year_label = move || -> String {
        let (f, t) = (date_from.get(), date_to.get());
        if f == t {
            f.format("%Y-%m-%d").to_string()
        } else {
            format!("{} → {}", f.format("%Y-%m-%d"), t.format("%Y-%m-%d"))
        }
    };

    let container_ref = NodeRef::<leptos::html::Div>::new();
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Measured container size. We watch the container's *own* box with a
    // ResizeObserver, not just the window's resize event: layout changes that
    // resize the map without resizing the window must still re-measure. The
    // chief offender is opening the mobile "Filters" sidebar, which on phones
    // drops into a grid row above the map and shrinks it (also: iOS Safari's
    // URL-bar show/hide and orientation changes). Without re-measuring, the
    // canvas bitmap (sized in px) stays CSS-stretched to a different box than
    // the px-positioned tiles and markers — distorting the heatmap and
    // misaligning the overlay, sometimes persisting after the sidebar closes.
    let (size, set_size) = signal((0.0_f64, 0.0_f64));
    let (resize_tick, set_resize_tick) = signal(0u32);
    // Window resize is kept as a fallback for any browser lacking ResizeObserver.
    let _ = leptos::prelude::window_event_listener(leptos::ev::resize, move |_| {
        set_resize_tick.update(|n| *n = n.wrapping_add(1));
    });
    Effect::new(move |_| {
        resize_tick.track();
        if let Some(el) = container_ref.get() {
            let rect = el.get_bounding_client_rect();
            set_size.set((rect.width(), rect.height()));
        }
    });
    // Attach the ResizeObserver once, when the container first mounts. It fires
    // immediately on observe (giving us the correct first measurement, so the
    // old post-paint re-measure timer is no longer needed) and on every box
    // change thereafter.
    Effect::new(move |attached: Option<bool>| {
        if attached == Some(true) {
            return true;
        }
        let Some(el) = container_ref.get() else { return false };
        let target: web_sys::Element = el.unchecked_into();
        let cb = Closure::<dyn FnMut()>::new(move || {
            set_resize_tick.update(|n| *n = n.wrapping_add(1));
        });
        match web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
            Ok(observer) => {
                observer.observe(&target);
                // Hold the observer + callback in owner-scoped local storage so
                // they live as long as the component, and disconnect on unmount
                // (so the dropped closure is never invoked after teardown). The
                // cleanup captures only the Send+Sync StoredValue handle.
                let held = StoredValue::new_local(Some((observer, cb)));
                on_cleanup(move || {
                    held.try_update_value(|slot| {
                        if let Some((obs, _cb)) = slot.take() {
                            obs.disconnect();
                        }
                    });
                });
                true
            }
            // Construction failed (no ResizeObserver support); fall back to the
            // window listener above. Returning false lets a later ref change retry.
            Err(_) => false,
        }
    });

    // Value domain across stations for the current (substance, stat):
    // (vmin, vmax, mean, n). The mean is the unweighted average of the
    // per-station values shown on the map — the legend's "Station average".
    let domain = move || -> Option<(f64, f64, f64, usize)> {
        let ys = year_stats();
        let sub = substance.get();
        let st = stat.get();
        let vals: Vec<f64> = stations
            .get()
            .iter()
            .filter_map(|s| station_value(&ys, s.id, &sub, st))
            .collect();
        if vals.is_empty() {
            return None;
        }
        let vmin = vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let vmax = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        Some((vmin, vmax, mean, vals.len()))
    };

    // Redraw the heatmap canvas whenever inputs change.
    Effect::new(move |_| {
        let (w, h) = size.get();
        let sub = substance.get();
        let st = stat.get();
        let stns = stations.get();
        let ys = year_stats();
        let Some(canvas) = canvas_ref.get() else { return };
        let Some(geom) = compute_geom(&stns, w, h) else {
            canvas.set_width(w.max(1.0) as u32);
            canvas.set_height(h.max(1.0) as u32);
            return;
        };
        let points: Vec<(f64, f64, f64)> = stns
            .iter()
            .filter_map(|s| {
                station_value(&ys, s.id, &sub, st).map(|v| {
                    let (sx, sy) = geom.screen(s.lat, s.lon);
                    (sx, sy, v)
                })
            })
            .collect();
        let (vmin, vmax) = points
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (_, _, v)| {
                (lo.min(*v), hi.max(*v))
            });
        draw_heatmap(&canvas, &geom, &points, vmin, vmax, sub == IQA_KEY);
    });

    // ── PNG export (copy / download), mirroring the chart's widgets ──
    // Snapshot state + the live DOM refs, then build the composite off-thread.
    let snapshot = move || {
        let container: Option<web_sys::Element> = container_ref.get().map(|d| d.unchecked_into());
        let heatmap = canvas_ref.get();
        let (w, h) = size.get();
        (container, heatmap, w, h, stations.get(), year_stats(), year_dom(),
         substance.get(), stat.get(), lang.get(), year_label())
    };

    let on_download = move |_| {
        let (Some(cont), Some(hc), w, h, stns, ys, dom, sub, st, l, yl) = snapshot() else { return };
        if w < 1.0 { return; }
        let filename = format!("airquality-map-{}.png", chrono::Local::now().format("%Y-%m-%d_%H%M%S"));
        crate::components::export::run_download(filename, move || {
            Box::pin(build_map_png_blob(cont, hc, w, h, stns, ys, dom, sub, st, l, yl))
        });
    };

    let (copy_flash, set_copy_flash) = signal(false);
    let on_copy_success = Callback::new(move |_: ()| {
        set_copy_flash.set(true);
        let cb = wasm_bindgen::closure::Closure::once_into_js(move || set_copy_flash.set(false));
        let _ = web_sys::window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), 1500);
    });
    let on_copy = move |_| {
        let (Some(cont), Some(hc), w, h, stns, ys, dom, sub, st, l, yl) = snapshot() else { return };
        if w < 1.0 { return; }
        crate::components::export::run_copy(
            move || {
                Box::pin(build_map_png_blob(
                    cont.clone(), hc.clone(), w, h, stns.clone(), ys.clone(), dom.clone(),
                    sub.clone(), st, l, yl.clone(),
                ))
            },
            on_copy_success,
        );
    };

    view! {
        <div class="map-container" node_ref=container_ref>
            <canvas class="heatmap-canvas" node_ref=canvas_ref></canvas>

            <div class="chart-export-group">
                <button class="chart-export"
                        class:flash=move || copy_flash.get()
                        title=move || if copy_flash.get() {
                            lang.get().t().copied_to_clipboard
                        } else {
                            lang.get().t().copy_chart_to_clipboard
                        }
                        on:click=on_copy>
                    {move || if copy_flash.get() {
                        view! {
                            <svg viewBox="0 0 24 24" aria-hidden="true">
                                <path d="M5 12 l4 4 l10 -10" fill="none" stroke="currentColor"
                                      stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                            </svg>
                        }.into_any()
                    } else {
                        view! {
                            <svg viewBox="0 0 24 24" aria-hidden="true">
                                <rect x="6" y="5" width="12" height="16" rx="1.5"
                                      fill="none" stroke="currentColor" stroke-width="1.6"/>
                                <rect x="9" y="3" width="6" height="3" rx="0.8"
                                      fill="none" stroke="currentColor" stroke-width="1.6"/>
                            </svg>
                        }.into_any()
                    }}
                </button>
                <button class="chart-export"
                        title=move || lang.get().t().download_chart_png
                        on:click=on_download>
                    <svg viewBox="0 0 24 24" aria-hidden="true">
                        <path d="M12 4 V15 M7 11 l5 5 l5 -5 M5 20 h14"
                              fill="none" stroke="currentColor" stroke-width="1.8"
                              stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                </button>
            </div>

            {move || {
                let (w, h) = size.get();
                let stns = stations.get();
                let ms = year_stats();
                let iqa_dom = year_dom();
                let sub = substance.get();
                let is_iqa = sub == IQA_KEY;
                let st = stat.get();
                let l = lang.get();
                let Some(geom) = compute_geom(&stns, w, h) else {
                    return view! { <div class="map-hint">{l.t().loading_stations}</div> }.into_any();
                };

                let dom = domain();
                let (vmin, vmax) = dom.map(|(a, b, _, _)| (a, b)).unwrap_or((0.0, 1.0));
                let span = (vmax - vmin).max(1e-9);

                // ── Tiles ──
                let half_w_tiles = (w / 2.0 / geom.eff_tile).ceil() as i64 + 1;
                let half_h_tiles = (h / 2.0 / geom.eff_tile).ceil() as i64 + 1;
                let cxt = geom.cx_tile.floor() as i64;
                let cyt = geom.cy_tile.floor() as i64;
                let n_tiles: i64 = 1i64 << geom.zoom;
                let tiles = (cxt - half_w_tiles..=cxt + half_w_tiles)
                    .flat_map(move |tx| (cyt - half_h_tiles..=cyt + half_h_tiles).map(move |ty| (tx, ty)))
                    .filter(|&(_, ty)| ty >= 0 && ty < n_tiles)
                    .map(|(tx, ty)| {
                        let txw = ((tx % n_tiles) + n_tiles) % n_tiles;
                        let left = geom.w / 2.0 + (tx as f64 - geom.cx_tile) * geom.eff_tile;
                        let top = geom.h / 2.0 + (ty as f64 - geom.cy_tile) * geom.eff_tile;
                        let url = format!(
                            "https://a.basemaps.cartocdn.com/dark_all/{}/{}/{}@2x.png",
                            geom.zoom, txw, ty,
                        );
                        let style = format!(
                            "left:{left:.0}px;top:{top:.0}px;width:{0:.0}px;height:{0:.0}px;",
                            geom.eff_tile,
                        );
                        // crossorigin lets the export canvas composite these tiles
                        // without tainting (CARTO sends Access-Control-Allow-Origin: *).
                        view! { <img class="map-tile" src=url style=style draggable="false" crossorigin="anonymous"/> }
                    })
                    .collect_view();

                // ── Markers ──
                let markers = stns.into_iter().map(|s| {
                    let (sx, sy) = geom.screen(s.lat, s.lon);
                    let val = station_value(&ms, s.id, &sub, st);
                    let label_above = sy > geom.h - 44.0;
                    let class = match (val.is_some(), label_above) {
                        (true, true) => "map-marker label-above",
                        (true, false) => "map-marker",
                        (false, true) => "map-marker nodata label-above",
                        (false, false) => "map-marker nodata",
                    };
                    let style = format!("left:{sx:.0}px;top:{sy:.0}px;", );
                    let unit = pollutants::unit_of(&sub);
                    // "17" for the unitless index, "6.5 µg/m³" for concentrations.
                    let with_unit = |v: f64| {
                        let n = crate::components::chart::fmt_val(v);
                        if unit.is_empty() { n } else { format!("{n} {unit}") }
                    };
                    let (dot_style, chip, full) = match val {
                        Some(v) => {
                            let hex = if is_iqa {
                                iqa_color_hex(v)
                            } else {
                                ramp_hex(((v - vmin) / span).clamp(0.0, 1.0))
                            };
                            (
                                format!("background:{hex};border-color:#fff;"),
                                crate::components::chart::fmt_val(v),
                                format!("{} — {}", s.name, with_unit(v)),
                            )
                        }
                        None => (
                            String::new(),
                            String::new(),
                            format!("{} — {} {}", s.name, sub, l.t().no_data_substance),
                        ),
                    };
                    let chip_show = !chip.is_empty();
                    let name = s.name.clone();
                    let value_line = match val {
                        Some(v) => with_unit(v),
                        None => l.t().no_data_substance.to_string(),
                    };

                    // For the IQA index, name the driving pollutant: the peak-hour
                    // driver under the Maximum stat, else the year-round most-
                    // frequent driver (with its share of hours).
                    let driver = if is_iqa && val.is_some() {
                        iqa_dom.get(&s.id.to_string()).and_then(|d| match st {
                            Stat::Max => Some(format!("{}: {}", l.t().iqa_peak_driver, d.peak_pollutant)),
                            _ => d.shares.first().map(|(p, frac)| {
                                format!("{}: {} {}%", l.t().iqa_main_driver, p, (frac * 100.0).round() as i64)
                            }),
                        })
                    } else {
                        None
                    };
                    // Fold the driver into the native title tooltip too.
                    let full = match &driver {
                        Some(d) => format!("{full}\n{d}"),
                        None => full,
                    };

                    view! {
                        <div class=class style=style title=full>
                            <span class="marker-dot" style=dot_style></span>
                            {chip_show.then(|| view! { <span class="marker-value-chip">{chip.clone()}</span> })}
                            <span class="marker-label">
                                {name}<br/><span class="marker-value">{value_line}</span>
                                {driver.map(|d| view! { <br/><span class="marker-driver">{d}</span> })}
                            </span>
                        </div>
                    }
                }).collect_view();

                view! {
                    {tiles}
                    {markers}
                }.into_any()
            }}

            // ── Colour-bar legend ──
            {move || {
                let l = lang.get();
                let sub = substance.get();
                let st = stat.get();
                let Some((vmin, vmax, mean, n)) = domain() else {
                    return ().into_any();
                };
                // Title carries all three map parameters: substance · statistic · year(s).
                let title = format!(
                    "{} · {} · {}",
                    pollutants::name_of(&sub, l), st.label(l), year_label()
                );
                // Global (unweighted) average of the per-station values on display.
                let unit = pollutants::unit_of(&sub);
                let avg_txt = {
                    let v = crate::components::chart::fmt_val(mean);
                    if unit.is_empty() {
                        format!("{}: {v}", l.t().map_avg)
                    } else {
                        format!("{}: {v} {unit}", l.t().map_avg)
                    }
                };

                if sub == IQA_KEY {
                    // Absolute acceptability bands: Good / Acceptable / Poor.
                    let t = l.t();
                    let good = iqa_color_hex(IQA_GOOD_MAX * 0.4);
                    let acc = iqa_color_hex((IQA_GOOD_MAX + IQA_ACCEPTABLE_MAX) / 2.0);
                    let poor = iqa_color_hex(IQA_ACCEPTABLE_MAX * 1.6);
                    return view! {
                        <div class="colorbar">
                            <div class="colorbar-title">{title}</div>
                            <div class="colorbar-bands">
                                <span style=format!("background:{good};")></span>
                                <span style=format!("background:{acc};")></span>
                                <span style=format!("background:{poor};")></span>
                            </div>
                            <div class="colorbar-bandlabels">
                                <span>{format!("{} 0–25", t.iqa_good)}</span>
                                <span>{format!("{} 26–50", t.iqa_acceptable)}</span>
                                <span>{format!("{} 50+", t.iqa_poor)}</span>
                            </div>
                            <div class="colorbar-avg">{avg_txt}</div>
                            <div class="colorbar-note warn">{t.iqa_higher_worse}</div>
                            <div class="colorbar-note">{format!("{n} {}", t.stations_measuring)}</div>
                        </div>
                    }.into_any();
                }

                let mid = (vmin + vmax) / 2.0;
                view! {
                    <div class="colorbar">
                        <div class="colorbar-title">{title}</div>
                        <div class="colorbar-gradient" style=format!("background:{RAMP_CSS};")></div>
                        <div class="colorbar-scale">
                            <span>{crate::components::chart::fmt_val(vmin)}</span>
                            <span>{crate::components::chart::fmt_val(mid)}</span>
                            <span>{crate::components::chart::fmt_val(vmax)}</span>
                        </div>
                        <div class="colorbar-avg">{avg_txt}</div>
                        <div class="colorbar-note">
                            {if unit.is_empty() {
                                format!("{n} {}", l.t().stations_measuring)
                            } else {
                                format!("{unit} · {n} {}", l.t().stations_measuring)
                            }}
                        </div>
                    </div>
                }.into_any()
            }}

            <div class="map-hint">{move || lang.get().t().click_marker}</div>
            // When a time-of-day window is active the map reads the hourly tier;
            // note this (and the long-range bound) so the values read honestly.
            {move || {
                (hour_from.get() != 0 || hour_to.get() != 23).then(|| {
                    view! { <div class="map-hint hour-note">{lang.get().t().map_hour_note}</div> }
                })
            }}
            <div class="map-attribution">"© OpenStreetMap © CARTO · RSQA"</div>
        </div>
    }
}
