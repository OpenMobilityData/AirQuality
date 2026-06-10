use chrono::{DateTime, Utc};
use chrono_tz::America::Montreal as MontrealTz;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::data::types::{Interval, Profile};
use crate::i18n::Lang;

#[derive(Clone, PartialEq)]
pub struct Series {
    pub label: String,
    pub color: String,
    /// SVG `stroke-dasharray` value; empty string means solid.
    pub dash: String,
    pub points: Vec<(DateTime<Utc>, f64)>,
}

/// Format a concentration value with a sensible number of significant digits.
pub fn fmt_val(v: f64) -> String {
    let a = v.abs();
    if a == 0.0 {
        "0".into()
    } else if a < 1.0 {
        format!("{v:.3}")
    } else if a < 10.0 {
        format!("{v:.2}")
    } else if a < 100.0 {
        format!("{v:.1}")
    } else if a < 10_000.0 {
        format!("{v:.0}")
    } else {
        // Large counts (e.g. ultrafine particles): thousands-separated integer.
        let n = v.round() as i64;
        let mag = n.unsigned_abs().to_string();
        let bytes = mag.as_bytes();
        let mut out = String::new();
        if n < 0 {
            out.push('-');
        }
        for (i, b) in bytes.iter().enumerate() {
            if i > 0 && (bytes.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(*b as char);
        }
        out
    }
}

fn series_stats(points: &[(DateTime<Utc>, f64)], lang: Lang) -> String {
    if points.is_empty() {
        return String::new();
    }
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0_f64;
    for (_, v) in points {
        if *v < min {
            min = *v;
        }
        if *v > max {
            max = *v;
        }
        sum += *v;
    }
    let mean = sum / points.len() as f64;
    let t = lang.t();
    format!(
        "{} {}  {} {}  {} {}",
        t.leg_mean, fmt_val(mean),
        t.leg_min, fmt_val(min),
        t.leg_max, fmt_val(max),
    )
}

/// Layout + data-range parameters mapping data coords (timestamp / value) to
/// SVG user coords. Shared between rendering and the hover handler.
#[derive(Clone)]
struct ChartGeom {
    x_min: f64,
    y_min: f64,
    x_span: f64,
    y_span: f64,
    pad_l: f64,
    pad_t: f64,
    w: f64,
    h: f64,
}

impl ChartGeom {
    fn to_x(&self, ts: i64) -> f64 {
        self.pad_l + (ts as f64 - self.x_min) / self.x_span * self.w
    }
    fn to_y(&self, v: f64) -> f64 {
        self.pad_t + self.h - (v - self.y_min) / self.y_span * self.h
    }
}

fn compute_geom(
    s: &[Series],
    x_range: Option<(DateTime<Utc>, DateTime<Utc>)>,
    pad_l: f64,
    pad_t: f64,
    w: f64,
    h: f64,
) -> Option<ChartGeom> {
    if s.is_empty() || s.iter().all(|ser| ser.points.is_empty()) {
        return None;
    }
    // A fixed range (profile modes) overrides the data-derived x extent.
    let (x_min, x_max) = match x_range {
        Some((lo, hi)) => (lo.timestamp() as f64, hi.timestamp() as f64),
        None => {
            let xs: Vec<i64> = s
                .iter()
                .flat_map(|ser| ser.points.iter().map(|(t, _)| t.timestamp()))
                .collect();
            (*xs.iter().min().unwrap() as f64, *xs.iter().max().unwrap() as f64)
        }
    };
    // Concentrations can be near-zero but never negative; anchor at 0.
    let y_min = 0.0_f64;
    let y_max = s
        .iter()
        .flat_map(|ser| ser.points.iter().map(|(_, v)| *v))
        .fold(0.0_f64, f64::max)
        * 1.1;
    Some(ChartGeom {
        x_min,
        y_min,
        x_span: (x_max - x_min).max(1.0),
        y_span: (y_max - y_min).max(1e-6),
        pad_l,
        pad_t,
        w,
        h,
    })
}

#[derive(Clone)]
struct HoverInfo {
    crosshair_x: f64,
    client_x: f64,
    client_y: f64,
    flip_x: bool,
    flip_y: bool,
    rows: Vec<HoverRow>,
}

#[derive(Clone)]
struct HoverRow {
    color: String,
    label: String,
    timestamp: DateTime<Utc>,
    value: f64,
    point_x: f64,
    point_y: f64,
}

/// Tooltip lead-in. Profiles label by their synthetic base (hour-of-day or
/// weekday); otherwise Hour interval shows Montréal-local time and coarser
/// intervals show the date.
fn format_hover_date(
    ts: DateTime<Utc>,
    interval: Interval,
    profile: Option<Profile>,
    x_min: f64,
    lang: Lang,
) -> String {
    match profile {
        Some(Profile::Weekday) | Some(Profile::Weekend) => ts.format("%H:%M").to_string(),
        Some(Profile::Weekly) => {
            // Points sit mid-cell (day + 12 h), so floor — not round — maps
            // them back to their weekday.
            let d = (((ts.timestamp() as f64 - x_min) / 86_400.0).floor() as i64).rem_euclid(7) as usize;
            lang.t().dow[d].to_string()
        }
        None => match interval {
            Interval::Hour => ts.with_timezone(&MontrealTz).format("%Y-%m-%d %H:%M %Z").to_string(),
            _ => ts.format("%Y-%m-%d").to_string(),
        },
    }
}

const EXPORT_EMBEDDED_CSS: &str = r#"
    text { font-family: Inter, system-ui, sans-serif; }
    .chart-axis line, .chart-axis path { stroke: #2a3a5c; }
    .chart-axis text { fill: #8892a4; font-size: 11px; }
    .chart-axis-title { fill: #eaeaea; font-size: 11px; opacity: 0.85; }
    .chart-grid line { stroke: #2a3a5c; stroke-dasharray: 3 4; }
    .chart-line { fill: none; stroke-width: 2; }
    .chart-area { opacity: 0.15; }
    .chart-threshold { stroke: rgba(255,255,255,0.38); stroke-width: 1; stroke-dasharray: 5 4; }
    .chart-threshold-label { fill: #8892a4; font-size: 10px; }
    .export-caption { fill: #eaeaea; font-size: 13px; font-weight: 600; }
    .export-legend-label { fill: #eaeaea; font-size: 12px; }
    .export-legend-stats {
        fill: #8892a4; font-size: 11px;
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    }
"#;

fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
fn escape_xml_attr(s: &str) -> String {
    escape_xml_text(s).replace('"', "&quot;")
}

/// Build the composite SVG (caption + chart + legend) and rasterize to a PNG.
/// The `caption` carries the full selection (station / substance / interval /
/// dates) so a saved image is self-describing.
async fn build_chart_png_blob(
    series: Vec<Series>,
    caption: String,
    lang: Lang,
) -> Result<web_sys::Blob, JsValue> {
    let document = web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;

    let chart_svg = document
        .query_selector(".chart-container svg.chart-plot")?
        .ok_or_else(|| JsValue::from_str("chart svg not found"))?;

    let caption_h = 28.0_f64;
    let chart_clone = chart_svg.clone_node_with_deep(true)?.dyn_into::<web_sys::Element>()?;
    chart_clone.set_attribute("width", "900")?;
    chart_clone.set_attribute("height", "400")?;
    chart_clone.set_attribute("y", &caption_h.to_string())?; // leave room for the caption
    chart_clone.set_attribute("preserveAspectRatio", "xMidYMid meet")?;
    if let Some(g) = chart_clone.query_selector(".chart-hover")? {
        g.remove();
    }

    let serializer = web_sys::XmlSerializer::new()?;
    let chart_xml = serializer.serialize_to_string(&chart_clone)?;

    let total_w = 900.0_f64;
    let chart_h = 400.0_f64;
    let row_h = 20.0;
    let pad_top = 8.0;
    let pad_bot = 12.0;
    let legend_h = pad_top + (series.len().max(1) as f64) * row_h + pad_bot;
    let total_h = caption_h + chart_h + legend_h;

    let mut legend_xml = String::new();
    for (i, ser) in series.iter().enumerate() {
        let y = caption_h + chart_h + pad_top + (i as f64) * row_h + 14.0;
        let lx = 22.0;
        let lxe = lx + 26.0;
        let tx = lxe + 8.0;
        let dash = if ser.dash.is_empty() {
            String::new()
        } else {
            format!(" stroke-dasharray=\"{}\"", escape_xml_attr(&ser.dash))
        };
        let stats = series_stats(&ser.points, lang);
        legend_xml.push_str(&format!(
            "<line x1=\"{lx:.1}\" y1=\"{ly:.1}\" x2=\"{lxe:.1}\" y2=\"{ly:.1}\" \
             stroke=\"{color}\" stroke-width=\"2\"{dash}/>\
             <text x=\"{tx:.1}\" y=\"{y:.1}\" class=\"export-legend-label\">{label}\
             <tspan class=\"export-legend-stats\" dx=\"8\">{stats}</tspan></text>",
            ly = y - 4.0,
            color = escape_xml_attr(&ser.color),
            label = escape_xml_text(&ser.label),
            stats = escape_xml_text(&stats),
        ));
    }

    let composite = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.1} {h:.1}" width="{w:.1}" height="{h:.1}">
<style>{styles}</style>
<rect width="{w:.1}" height="{h:.1}" fill="#0d1b2a"/>
<text x="22" y="19" class="export-caption">{caption}</text>
{chart}
{legend}
</svg>"##,
        w = total_w,
        h = total_h,
        styles = EXPORT_EMBEDDED_CSS,
        caption = escape_xml_text(&caption),
        chart = chart_xml,
        legend = legend_xml,
    );

    crate::components::export::svg_to_png_blob(composite, total_w, total_h).await
}

/// Max gap (seconds) between consecutive buckets before the line breaks.
fn gap_threshold_secs(interval: Interval) -> i64 {
    match interval {
        Interval::Hour => 2 * 3600,
        Interval::Day => 2 * 86_400,
        Interval::Week => 2 * 7 * 86_400,
        Interval::Month => 2 * 28 * 86_400,
        Interval::Year => 2 * 365 * 86_400,
    }
}

#[component]
pub fn Chart(
    series: ReadSignal<Vec<Series>>,
    interval: ReadSignal<Interval>,
    /// Y-axis title, e.g. "Nitrogen dioxide (ppb)".
    y_title: Signal<String>,
    /// Optional horizontal reference lines `(value, label)` — used to mark the
    /// IQA acceptability thresholds. Empty for ordinary concentrations.
    thresholds: Signal<Vec<(f64, String)>>,
    /// Human-readable selection summary (station · substance · interval · dates),
    /// shown above the plot and embedded in the PNG export.
    caption: Signal<String>,
    /// Multi-line note on how the plotted data falls short of the query range
    /// (late start / early end / long gaps); shown as a hoverable info chip
    /// beside the caption. `None` hides the chip.
    coverage: Signal<Option<String>>,
    /// Active averaging profile (None = ordinary time series) — switches the
    /// x-axis to a 24-hour (diurnal) or 7-day (weekly) synthetic base.
    profile: Signal<Option<Profile>>,
    /// Fixed x-axis range; `Some` in profile modes, `None` otherwise.
    x_range: Signal<Option<(DateTime<Utc>, DateTime<Utc>)>>,
) -> impl IntoView {
    let lang = use_context::<ReadSignal<Lang>>().expect("Lang context not provided");
    let view_box = "0 0 900 400";
    let pad_l = 75.0_f64;
    let pad_r = 20.0_f64;
    let pad_t = 20.0_f64;
    let pad_b = 50.0_f64;
    let w = 900.0_f64 - pad_l - pad_r;
    let h = 400.0_f64 - pad_t - pad_b;

    let (hover, set_hover) = signal::<Option<HoverInfo>>(None);

    let _ = leptos::prelude::window_event_listener(leptos::ev::scroll, move |_| set_hover.set(None));
    let _ = leptos::prelude::window_event_listener(leptos::ev::touchstart, move |_| set_hover.set(None));

    let derived = move || {
        let s = series.get();
        let res = interval.get();
        let prof = profile.get();
        let xr = x_range.get();
        let g = compute_geom(&s, xr, pad_l, pad_t, w, h)?;
        let y_max = g.y_min + g.y_span;
        let base_y = g.to_y(g.y_min);
        // Profiles are dense contiguous folds — never break the line.
        let gap_threshold = if prof.is_some() { i64::MAX } else { gap_threshold_secs(res) };

        let paths: Vec<(String, String, String, String)> = s
            .iter()
            .map(|ser| {
                if ser.points.is_empty() {
                    return (ser.color.clone(), ser.dash.clone(), String::new(), String::new());
                }
                let mut line_d = String::new();
                let mut area_d = String::new();
                let mut prev_ts: Option<i64> = None;
                let mut seg_first_x: Option<f64> = None;
                let mut seg_last_x: f64 = 0.0;
                for (dt, v) in &ser.points {
                    let ts = dt.timestamp();
                    let x = g.to_x(ts);
                    let y = g.to_y(*v);
                    let new_segment = match prev_ts {
                        None => true,
                        Some(p) => ts - p > gap_threshold,
                    };
                    if new_segment {
                        if let Some(fx) = seg_first_x {
                            area_d.push_str(&format!(" L {:.1},{:.1} L {:.1},{:.1} Z", seg_last_x, base_y, fx, base_y));
                        }
                        if !line_d.is_empty() {
                            line_d.push(' ');
                        }
                        line_d.push_str(&format!("M {x:.1},{y:.1}"));
                        if !area_d.is_empty() {
                            area_d.push(' ');
                        }
                        area_d.push_str(&format!("M {x:.1},{y:.1}"));
                        seg_first_x = Some(x);
                    } else {
                        line_d.push_str(&format!(" L {x:.1},{y:.1}"));
                        area_d.push_str(&format!(" L {x:.1},{y:.1}"));
                    }
                    seg_last_x = x;
                    prev_ts = Some(ts);
                }
                if let Some(fx) = seg_first_x {
                    area_d.push_str(&format!(" L {:.1},{:.1} L {:.1},{:.1} Z", seg_last_x, base_y, fx, base_y));
                }
                (ser.color.clone(), ser.dash.clone(), line_d, area_d)
            })
            .collect();

        let tick_count = 5;
        let y_ticks: Vec<(f64, String)> = (0..=tick_count)
            .map(|i| {
                let v = g.y_min + (y_max - g.y_min) * i as f64 / tick_count as f64;
                (g.to_y(v), fmt_val(v))
            })
            .collect();

        let l = lang.get();
        let x_ticks: Vec<(f64, String)> = match prof {
            // Diurnal: 00:00 … 24:00 by raw fraction of the day, on round hours.
            Some(Profile::Weekday) | Some(Profile::Weekend) => (0..=6)
                .map(|i| {
                    let hr = i * 4;
                    (g.pad_l + (hr as f64 / 24.0) * g.w, format!("{hr:02}:00"))
                })
                .collect(),
            // Weekly: one label per weekday, centred in its day cell.
            Some(Profile::Weekly) => (0..7)
                .map(|d| {
                    let ts = (g.x_min + (d as f64 + 0.5) * 86_400.0) as i64;
                    (g.to_x(ts), l.t().dow[d].to_string())
                })
                .collect(),
            None => {
                let max_points = s.iter().map(|ser| ser.points.len()).max().unwrap_or(0);
                let x_tick_n = 7.min(max_points);
                if x_tick_n == 0 {
                    vec![]
                } else {
                    let span_days = g.x_span / 86_400.0;
                    (0..=x_tick_n)
                        .map(|i| {
                            let ts = (g.x_min + g.x_span * i as f64 / x_tick_n as f64) as i64;
                            let x = g.to_x(ts);
                            let label = DateTime::from_timestamp(ts, 0)
                                .map(|dt: DateTime<Utc>| match res {
                                    Interval::Hour => dt.with_timezone(&MontrealTz).format("%b %-d %H:%M").to_string(),
                                    Interval::Year => dt.format("%Y").to_string(),
                                    _ if span_days > 800.0 => dt.format("%b %Y").to_string(),
                                    _ => dt.format("%b %-d").to_string(),
                                })
                                .unwrap_or_default();
                            (x, label)
                        })
                        .collect()
                }
            }
        };

        // Acceptability reference lines that fall within the visible y-range.
        let thresh_lines: Vec<(f64, String)> = thresholds
            .get()
            .iter()
            .filter(|(v, _)| *v <= y_max)
            .map(|(v, lab)| (g.to_y(*v), format!("{lab} · {v:.0}")))
            .collect();

        Some((paths, y_ticks, x_ticks, thresh_lines))
    };

    let compute_hover = move |client_x: f64, client_y: f64, rect: web_sys::DomRect, force_flip_y: bool| -> Option<HoverInfo> {
        let s = series.get();
        let g = compute_geom(&s, x_range.get(), pad_l, pad_t, w, h)?;

        let frac_x = ((client_x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
        let frac_y = ((client_y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
        let cursor_svg_x = g.pad_l + frac_x * g.w;
        let cursor_svg_y = g.pad_t + frac_y * g.h;

        let mut anchor_ts: Option<DateTime<Utc>> = None;
        let mut best_d2 = f64::INFINITY;
        for ser in s.iter() {
            for (t, v) in &ser.points {
                let px = g.to_x(t.timestamp());
                let py = g.to_y(*v);
                let d2 = (px - cursor_svg_x).powi(2) + (py - cursor_svg_y).powi(2);
                if d2 < best_d2 {
                    best_d2 = d2;
                    anchor_ts = Some(*t);
                }
            }
        }
        let anchor_ts = anchor_ts?;
        let anchor_secs = anchor_ts.timestamp() as f64;
        let coverage_tol = (gap_threshold_secs(interval.get()) / 2) as f64;

        let rows: Vec<HoverRow> = s
            .iter()
            .filter_map(|ser| {
                if ser.points.is_empty() {
                    return None;
                }
                let nearest = ser.points.iter().min_by(|a, b| {
                    let da = (a.0.timestamp() as f64 - anchor_secs).abs();
                    let db = (b.0.timestamp() as f64 - anchor_secs).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })?;
                if (nearest.0.timestamp() as f64 - anchor_secs).abs() >= coverage_tol {
                    return None;
                }
                Some(HoverRow {
                    color: ser.color.clone(),
                    label: ser.label.clone(),
                    timestamp: nearest.0,
                    value: nearest.1,
                    point_x: g.to_x(nearest.0.timestamp()),
                    point_y: g.to_y(nearest.1),
                })
            })
            .collect();

        if rows.is_empty() {
            return None;
        }

        let crosshair_x = g.to_x(anchor_ts.timestamp());
        let viewport = web_sys::window()
            .map(|w| {
                (
                    w.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1920.0),
                    w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(1080.0),
                )
            })
            .unwrap_or((1920.0, 1080.0));
        let flip_x = client_x + 300.0 > viewport.0;
        let flip_y = force_flip_y || client_y + 200.0 > viewport.1;

        Some(HoverInfo { crosshair_x, client_x, client_y, flip_x, flip_y, rows })
    };

    let on_move = move |ev: web_sys::MouseEvent| {
        let Some(target) = ev.current_target() else { return };
        let Ok(elem) = target.dyn_into::<web_sys::Element>() else { return };
        let rect = elem.get_bounding_client_rect();
        set_hover.set(compute_hover(ev.client_x() as f64, ev.client_y() as f64, rect, false));
    };
    let on_leave = move |_: web_sys::MouseEvent| set_hover.set(None);
    let on_touch = move |ev: web_sys::TouchEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        let Some(touch) = ev.touches().get(0) else { return };
        let Some(target) = ev.current_target() else { return };
        let Ok(elem) = target.dyn_into::<web_sys::Element>() else { return };
        let rect = elem.get_bounding_client_rect();
        set_hover.set(compute_hover(touch.client_x() as f64, touch.client_y() as f64, rect, true));
    };

    let on_download = move |_| {
        let s = series.get();
        if s.is_empty() {
            return;
        }
        let (cap, l) = (caption.get(), lang.get());
        let filename = format!("airquality-{}.png", chrono::Local::now().format("%Y-%m-%d_%H%M%S"));
        crate::components::export::run_download(filename, move || {
            Box::pin(build_chart_png_blob(s, cap, l))
        });
    };

    let (copy_flash, set_copy_flash) = signal(false);
    let on_copy_success = Callback::new(move |_: ()| {
        set_copy_flash.set(true);
        let cb = Closure::once_into_js(move || set_copy_flash.set(false));
        let _ = web_sys::window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), 1500);
    });
    let on_copy = move |_| {
        let s = series.get();
        if s.is_empty() {
            return;
        }
        let (cap, l) = (caption.get(), lang.get());
        crate::components::export::run_copy(
            move || Box::pin(build_chart_png_blob(s.clone(), cap.clone(), l)),
            on_copy_success,
        );
    };

    view! {
        <div class="chart-container">
            <div class="chart-export-group"
                 style:display=move || if series.get().is_empty() { "none" } else { "" }>
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
                                <path d="M5 12 l4 4 l10 -10" fill="none"
                                      stroke="currentColor" stroke-width="2"
                                      stroke-linecap="round" stroke-linejoin="round"/>
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

            <div class="chart-caption"
                 style:display=move || if series.get().is_empty() { "none" } else { "" }>
                {move || caption.get()}
                {move || coverage.get().map(|tip| view! {
                    <span class="info-chip" title=tip>"i"</span>
                })}
            </div>

            {move || match derived() {
                None => view! {
                    <div class="placeholder">
                        <span class="show-desktop">{move || lang.get().t().select_station_desktop}</span>
                        <span class="show-mobile">{move || lang.get().t().select_station_mobile}</span>
                    </div>
                }.into_any(),
                Some((paths, y_ticks, x_ticks, thresh_lines)) => view! {
                    <svg class="chart-plot" viewBox=view_box preserveAspectRatio="none">
                        <g class="chart-grid">
                            {y_ticks.iter().map(|(y, _)| view! {
                                <line x1=pad_l y1=*y x2=(pad_l + w) y2=*y />
                            }).collect_view()}
                        </g>

                        {paths.iter().map(|(color, _, _, area_d)| view! {
                            <path d=area_d.clone() fill=color.clone() class="chart-area" />
                        }).collect_view()}

                        {paths.iter().map(|(color, dash, line_d, _)| view! {
                            <path d=line_d.clone() class="chart-line"
                                  stroke=color.clone()
                                  stroke-dasharray=dash.clone() />
                        }).collect_view()}

                        <g class="chart-axis">
                            <line x1=pad_l y1=pad_t x2=pad_l y2=(pad_t + h) />
                            {y_ticks.iter().map(|(y, label)| view! {
                                <g>
                                    <line x1=(pad_l - 4.0) y1=*y x2=pad_l y2=*y />
                                    <text x=(pad_l - 8.0) y=*y
                                          text-anchor="end" dominant-baseline="middle">
                                        {label.clone()}
                                    </text>
                                </g>
                            }).collect_view()}
                        </g>

                        <text class="chart-axis-title"
                              x=14.0 y=(pad_t + h / 2.0)
                              transform=format!("rotate(-90 14 {:.1})", pad_t + h / 2.0)
                              text-anchor="middle" dominant-baseline="middle">
                            {move || y_title.get()}
                        </text>

                        // Acceptability reference lines (e.g. IQA 25 / 50)
                        {thresh_lines.iter().map(|(y, label)| view! {
                            <g>
                                <line class="chart-threshold"
                                      x1=pad_l y1=*y x2=(pad_l + w) y2=*y />
                                <text class="chart-threshold-label"
                                      x=(pad_l + w - 4.0) y=(*y - 3.0)
                                      text-anchor="end">{label.clone()}</text>
                            </g>
                        }).collect_view()}

                        <g class="chart-axis">
                            <line x1=pad_l y1=(pad_t + h) x2=(pad_l + w) y2=(pad_t + h) />
                            {x_ticks.iter().map(|(x, label)| view! {
                                <g>
                                    <line x1=*x y1=(pad_t + h) x2=*x y2=(pad_t + h + 4.0) />
                                    <text x=*x y=(pad_t + h + 16.0)
                                          text-anchor="middle">{label.clone()}</text>
                                </g>
                            }).collect_view()}
                        </g>

                        <rect class="chart-hover-area"
                              x=pad_l y=pad_t width=w height=h
                              fill="transparent"
                              on:mousemove=on_move
                              on:mouseleave=on_leave
                              on:touchstart=on_touch.clone()
                              on:touchmove=on_touch />

                        {move || hover.get().map(|hi| view! {
                            <g class="chart-hover">
                                <line class="chart-crosshair"
                                      x1=hi.crosshair_x y1=pad_t
                                      x2=hi.crosshair_x y2=(pad_t + h) />
                                {hi.rows.iter().map(|row| view! {
                                    <circle class="chart-hover-dot"
                                            cx=row.point_x cy=row.point_y
                                            r="4" fill=row.color.clone() />
                                }).collect_view()}
                            </g>
                        })}
                    </svg>
                }.into_any(),
            }}

            {move || hover.get().map(|hi| {
                let res = interval.get();
                let prof = profile.get();
                let x_min = x_range.get().map(|(lo, _)| lo.timestamp() as f64).unwrap_or(0.0);
                let date = hi.rows.first()
                    .map(|r| format_hover_date(r.timestamp, res, prof, x_min, lang.get()))
                    .unwrap_or_default();
                let tx = if hi.flip_x { "calc(-100% - 14px)" } else { "14px" };
                let ty = if hi.flip_y { "calc(-100% - 14px)" } else { "14px" };
                view! {
                    <div class="chart-tooltip"
                         style=format!("left: {}px; top: {}px; transform: translate({}, {});",
                             hi.client_x, hi.client_y, tx, ty)>
                        <div class="chart-tooltip-date">{date}</div>
                        {hi.rows.iter().map(|row| {
                            let value = fmt_val(row.value);
                            view! {
                                <div class="chart-tooltip-row">
                                    <span class="chart-tooltip-swatch"
                                          style=format!("background: {};", row.color)></span>
                                    <span class="chart-tooltip-label">{row.label.clone()}</span>
                                    <span class="chart-tooltip-value">{value}</span>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }
            })}

            {move || {
                let s = series.get();
                if s.is_empty() {
                    return view! { <div></div> }.into_any();
                }
                view! {
                    <div class="chart-legend">
                        {s.into_iter().map(|ser| {
                            let stats = series_stats(&ser.points, lang.get());
                            view! {
                                <div class="chart-legend-item">
                                    <svg width="24" height="10">
                                        <line x1="1" y1="5" x2="23" y2="5"
                                              stroke=ser.color.clone()
                                              stroke-width="2"
                                              stroke-dasharray=ser.dash.clone() />
                                    </svg>
                                    <span>{ser.label.clone()}</span>
                                    <span class="legend-stats">{stats}</span>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
