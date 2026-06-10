mod components;
mod data;
mod i18n;

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc, Weekday};
use chrono_tz::America::Montreal as MontrealTz;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use components::chart::{Chart, Series};
use components::controls::Sidebar;
use components::info::{InfoKind, InfoPage};
use components::map::RegionMap;
use components::ufp::UfpView;
use data::loader::{self, DailySeries, IqaDominanceMap, Meta, ProfileSeries, SeriesFile, UfpSurface};
use data::pollutants;
use data::types::{
    DayType, Interval, Profile, Reading, Stat, Station, View, MONTH_LEN_DAYS, MONTH_START_DAYS,
};
use i18n::Lang;

/// Fixed anchor for synthetic profile axes — a Monday at UTC midnight. Diurnal
/// profiles lay points at `ANCHOR + h hours`; the weekly profile at `ANCHOR + d days`.
fn profile_anchor() -> chrono::DateTime<Utc> {
    Utc.from_utc_datetime(
        &NaiveDate::from_ymd_opt(2001, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap(),
    )
}

/// A frozen copy of the live trace, kept on the chart while the controls are
/// reconfigured into further traces (e.g. the same diurnal profile across eras).
#[derive(Clone)]
struct PinnedTrace {
    series: Series,
    /// Substance key and unit of the pinned trace. When the displayed traces
    /// span more than one substance, the chart switches to a normalized
    /// (%-of-trace-maximum) axis — different pollutants have wildly different
    /// dynamic ranges even when their nominal units match.
    substance: String,
    unit: &'static str,
    /// X-axis basis the trace was built on (see [`axis_kind`]).
    axis: u8,
    /// For ordinary (non-profile) pins, the in-range readings behind the
    /// trace, so it can be re-bucketed to follow the live interval. Profile
    /// pins fold to a fixed base, so they keep their points as pinned.
    readings: Option<Vec<Reading>>,
    /// Whether those readings are hourly (pinned under the Hour interval).
    /// Daily readings can't honestly produce hourly buckets, so such pins
    /// degrade to daily resolution when the live interval is Hour.
    hourly: bool,
}

/// Most pinned traces held at once (so with the live one the chart never draws
/// more than five — beyond that the legend and palette stop being readable).
const MAX_PINS: usize = 4;

/// X-axis basis of the chart for a given profile selection: ordinary time (0),
/// the 24-hour diurnal base (1), the 7-day weekly base (2), or the 12-month
/// annual base (3). Traces on different bases can't share an axis, so changing
/// basis clears the pins.
fn axis_kind(p: Option<Profile>) -> u8 {
    match p {
        None => 0,
        Some(Profile::Weekday | Profile::Weekend) => 1,
        Some(Profile::Weekly) => 2,
        Some(Profile::Annual) => 3,
    }
}

/// Pinned-trace palette, assigned in pin order and recycled on removal (the
/// live trace keeps the usual blue): orange, green, magenta, gold.
const PIN_COLORS: [&str; MAX_PINS] = ["#ff9f43", "#2ecc71", "#e06cf0", "#f5d442"];

/// First palette colour not used by an existing pin (pins can be removed in
/// any order, so recycle gaps rather than cycling an index).
fn next_pin_color(pins: &[PinnedTrace]) -> &'static str {
    PIN_COLORS
        .iter()
        .find(|c| !pins.iter().any(|p| p.series.color == **c))
        .copied()
        .unwrap_or(PIN_COLORS[0])
}

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// Preferred default substance when the current one is unavailable: NO (a
/// primary, traffic-emitted pollutant that stays well localized near sources),
/// then PM2.5 (measured everywhere), then the first available option.
fn default_substance(opts: &[String]) -> String {
    opts.iter()
        .find(|s| s.as_str() == "NO")
        .or_else(|| opts.iter().find(|s| s.as_str() == "PM2.5"))
        .or_else(|| opts.first())
        .cloned()
        .unwrap_or_default()
}

/// Read the active view from the `?view=<slug>` query parameter. `None` when
/// the parameter is absent or unrecognized, so the caller falls back to the
/// default view. Slugs are plain ASCII, so a manual parse avoids pulling in the
/// `UrlSearchParams` web-sys feature.
fn view_from_url() -> Option<View> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("view=").and_then(View::from_slug))
}

/// Record a view in the browser history as `?view=<slug>`, so the address bar
/// is shareable and Back/Forward step between tabs.
fn push_view_url(v: View) {
    if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
        let url = format!("?view={}", v.slug());
        let _ = history.push_state_with_url(&JsValue::NULL, "", Some(&url));
    }
}

#[component]
fn App() -> impl IntoView {
    // ── Language (persisted; provided as context) ──
    let (lang, set_lang) = signal(Lang::from_browser());
    Effect::new(move |_| lang.get().store());
    provide_context(lang);

    // ── Core data ──
    let (stations, set_stations) = signal::<Vec<Station>>(vec![]);
    let (iqa_dominance, set_iqa_dominance) = signal::<IqaDominanceMap>(BTreeMap::new());
    let (meta, set_meta) = signal::<Option<Meta>>(None);
    let (active_subs, set_active_subs) = signal::<Vec<String>>(vec![]);
    let (years, set_years) = signal::<Vec<i32>>(vec![]);
    // Daily tier (all years, one file per station) drives Day/Week/Month/Year;
    // hourly tier (per station-year) is loaded on demand for the Hour interval.
    let (daily_cache, set_daily_cache) = signal::<BTreeMap<u32, DailySeries>>(BTreeMap::new());
    let (hourly_cache, set_hourly_cache) =
        signal::<BTreeMap<(u32, i32), SeriesFile>>(BTreeMap::new());
    // Diurnal-profile tier (all years, one file per station): Weekday/Weekend
    // profiles over ranges too long for the bounded hourly tier.
    let (profile_cache, set_profile_cache) =
        signal::<BTreeMap<u32, ProfileSeries>>(BTreeMap::new());
    // Modelled UFP surface grid, fetched lazily the first time its view opens.
    let (ufp_surface, set_ufp_surface) = signal::<Option<UfpSurface>>(None);

    // ── UI state ──
    // Open on the view named by `?view=<slug>` (shareable deep link), else Map.
    let (view, set_view) = signal(view_from_url().unwrap_or(View::Map));
    let (selected_substance, set_selected_substance) = signal(String::from("NO"));
    let (stat, set_stat) = signal(Stat::Mean);
    // Map averaging window: an arbitrary [from, to] date range, kept separate from
    // the Series range so each view keeps its own default (the Map opens on the
    // latest year, set from meta; the Series on the whole record).
    let (map_date_from, set_map_date_from) = signal(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap());
    let (map_date_to, set_map_date_to) = signal(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap());
    // Map time-of-day filter: an inclusive local-hour window; [0, 23] = whole day.
    let (hour_from, set_hour_from) = signal(0u8);
    let (hour_to, set_hour_to) = signal(23u8);
    // Map day-type filter (all days / weekdays / weekends).
    let (day_type, set_day_type) = signal(DayType::All);
    // Whether the map draws station names (off by default).
    let (show_names, set_show_names) = signal(false);
    let (selected_station, set_selected_station) = signal::<Option<u32>>(None);
    let (interval, set_interval) = signal(Interval::Month);
    // Averaging profile (None = ordinary time series).
    let (profile, set_profile) = signal::<Option<Profile>>(None);
    let (date_from, set_date_from) = signal(NaiveDate::from_ymd_opt(1986, 1, 1).unwrap());
    let (date_to, set_date_to) = signal(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap());
    let (date_presets, set_date_presets) = signal::<Vec<(String, NaiveDate, NaiveDate)>>(vec![]);
    let (sidebar_open, set_sidebar_open) = signal(false);

    // ── URL ⇄ view sync (query-param deep links + Back/Forward) ──
    // Keep the document title in step with the active view and language (nicer
    // browser tabs and bookmarks for the shared `?view=…` links).
    Effect::new(move |_| {
        let t = lang.get().t();
        let label = match view.get() {
            View::Map => t.view_map,
            View::Series => t.view_series,
            View::Ufp => t.view_ufp,
            View::Network => t.view_network,
            View::Methods => t.view_methods,
            View::Limits => t.view_limits,
            View::Links => t.view_links,
        };
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            doc.set_title(&format!("AirQualityMTL — {label}"));
        }
    });
    // Follow browser Back/Forward: popstate only *reads* the URL into the view
    // signal, while tab clicks (`go` below) are the only writers — so the two
    // never feed back on each other.
    let _ = leptos::prelude::window_event_listener(leptos::ev::popstate, move |_| {
        let v = view_from_url().unwrap_or(View::Map);
        if view.get_untracked() != v {
            set_view.set(v);
        }
    });

    // ── Load static data on startup ──
    spawn_local(async move {
        match loader::fetch_stations().await {
            Ok(list) => {
                // Default to the station with the longest record (most years), so
                // the time-series view opens on a rich multi-decade series rather
                // than a station that closed decades ago.
                if let Some(best) = list.iter().max_by_key(|s| s.years.len()) {
                    set_selected_station.set(Some(best.id));
                }
                set_stations.set(list);
            }
            Err(e) => web_sys::console::error_1(&format!("stations: {e}").into()),
        }
    });
    spawn_local(async move {
        set_iqa_dominance.set(loader::fetch_iqa_dominance().await);
    });
    spawn_local(async move {
        match loader::fetch_meta().await {
            Ok(m) => {
                if let (Some(s), Some(e)) = (
                    NaiveDate::from_ymd_opt(m.min_year, 1, 1),
                    NaiveDate::from_ymd_opt(m.max_year, 12, 31),
                ) {
                    // Series opens on the whole record …
                    set_date_from.set(s);
                    set_date_to.set(e);
                }
                // … while the Map defaults to the latest year (a single-year
                // snapshot) — the most common entry point.
                if let (Some(s), Some(e)) = (
                    NaiveDate::from_ymd_opt(m.max_year, 1, 1),
                    NaiveDate::from_ymd_opt(m.max_year, 12, 31),
                ) {
                    set_map_date_from.set(s);
                    set_map_date_to.set(e);
                }
                set_years.set(m.years.clone());
                set_active_subs.set(m.substances.clone());
                set_meta.set(Some(m));
            }
            Err(e) => web_sys::console::error_1(&format!("meta: {e}").into()),
        }
    });

    // ── Date presets (depend on the loaded span + language) ──
    Effect::new(move |_| {
        let Some(m) = meta.get() else { return };
        let (Some(start), Some(end)) = (
            NaiveDate::from_ymd_opt(m.min_year, 1, 1),
            NaiveDate::from_ymd_opt(m.max_year, 12, 31),
        ) else {
            return;
        };
        let t = lang.get().t();
        let presets = vec![
            (t.all_years.to_string(), start, end),
            (t.last_10_years.to_string(), end - Duration::days(3653), end),
            (t.last_5_years.to_string(), end - Duration::days(1826), end),
            (t.last_year.to_string(), end - Duration::days(365), end),
            (t.last_3_months.to_string(), end - Duration::days(91), end),
        ];
        // Clamp each preset's start to the data span.
        let presets = presets
            .into_iter()
            .map(|(lbl, f, tt)| (lbl, f.max(start), tt))
            .collect();
        set_date_presets.set(presets);
    });

    // ── Substance options: all in Map view; the station's set in Series view ──
    let substance_options = Memo::new(move |_| {
        let subs = active_subs.get();
        match view.get() {
            View::Map | View::Ufp | View::Network | View::Methods | View::Limits
            | View::Links => subs,
            View::Series => match selected_station.get() {
                Some(sid) => stations
                    .get()
                    .iter()
                    .find(|s| s.id == sid)
                    .map(|s| s.substances.clone())
                    .unwrap_or(subs),
                None => subs,
            },
        }
    });

    // Keep the selected substance valid for the current option set.
    Effect::new(move |_| {
        let opts = substance_options.get();
        if opts.is_empty() {
            return;
        }
        let cur = selected_substance.get_untracked();
        if !opts.iter().any(|s| s == &cur) {
            set_selected_substance.set(default_substance(&opts));
        }
    });

    // Hourly fine-detail is bounded: when the Hour interval spans many years we
    // load only the most recent few, to avoid pulling tens of per-year files.
    const MAX_HOURLY_YEARS: usize = 3;
    // Diurnal (Weekday/Weekend) profiles use the exact hourly tier only while
    // the range fits the hourly bound; longer ranges read the precomputed
    // profile tier instead, so the profile spans the full record.
    let diurnal_uses_hourly = move || -> bool {
        (date_to.get().year() - date_from.get().year() + 1) as usize <= MAX_HOURLY_YEARS
    };
    let hourly_years_in_range = move || -> Vec<i32> {
        let (lo, hi) = (date_from.get().year(), date_to.get().year());
        let avail = years.get();
        let mut ys: Vec<i32> = (lo..=hi).filter(|y| avail.contains(y)).collect();
        if ys.len() > MAX_HOURLY_YEARS {
            ys = ys.split_off(ys.len() - MAX_HOURLY_YEARS); // keep newest
        }
        ys
    };

    // ── Fetch the daily tier (all years) for the selected station ──
    Effect::new(move |_| {
        if view.get() != View::Series {
            return;
        }
        let Some(sid) = selected_station.get() else { return };
        if daily_cache.get_untracked().contains_key(&sid) {
            return;
        }
        spawn_local(async move {
            match loader::fetch_daily_series(sid).await {
                Ok(d) => set_daily_cache.update(|c| {
                    c.insert(sid, d);
                }),
                Err(e) => web_sys::console::error_1(&format!("daily {sid}: {e}").into()),
            }
        });
    });

    // ── Fetch hourly per-year files on demand (Hour interval or short-range
    // diurnal profile; long-range diurnal profiles use the profile tier) ──
    Effect::new(move |_| {
        let needs_hourly = match profile.get() {
            Some(p) => p.needs_hourly() && diurnal_uses_hourly(),
            None => interval.get() == Interval::Hour,
        };
        if view.get() != View::Series || !needs_hourly {
            return;
        }
        let Some(sid) = selected_station.get() else { return };
        let cached = hourly_cache.get_untracked();
        for y in hourly_years_in_range() {
            if cached.contains_key(&(sid, y)) {
                continue;
            }
            spawn_local(async move {
                match loader::fetch_series(sid, y).await {
                    Ok(sf) => set_hourly_cache.update(|c| {
                        c.insert((sid, y), sf);
                    }),
                    Err(e) => web_sys::console::error_1(&format!("hourly {sid}-{y}: {e}").into()),
                }
            });
        }
    });

    // ── Fetch the diurnal-profile tier for long-range Weekday/Weekend profiles ──
    Effect::new(move |_| {
        let needs_profile =
            profile.get().is_some_and(|p| p.needs_hourly()) && !diurnal_uses_hourly();
        if view.get() != View::Series || !needs_profile {
            return;
        }
        let Some(sid) = selected_station.get() else { return };
        if profile_cache.get_untracked().contains_key(&sid) {
            return;
        }
        spawn_local(async move {
            match loader::fetch_profile_series(sid).await {
                Ok(p) => set_profile_cache.update(|c| {
                    c.insert(sid, p);
                }),
                Err(e) => web_sys::console::error_1(&format!("profiles {sid}: {e}").into()),
            }
        });
    });

    // ── UFP view: fetch the modelled surface grid once, on first open ──
    Effect::new(move |_| {
        if view.get() != View::Ufp || ufp_surface.with_untracked(|s| s.is_some()) {
            return;
        }
        spawn_local(async move {
            match loader::fetch_ufp_surface().await {
                Ok(s) => set_ufp_surface.set(Some(s)),
                Err(e) => web_sys::console::error_1(&format!("ufp surface: {e}").into()),
            }
        });
    });

    // ── Map: fetch the daily tier for every station when the Map view is open ──
    // The map draws all stations at once, so it needs every station's daily file.
    // Shared with the Series cache, so a station already viewed there is warm.
    Effect::new(move |_| {
        if view.get() != View::Map {
            return;
        }
        let cached = daily_cache.get_untracked();
        for s in stations.get() {
            let sid = s.id;
            if cached.contains_key(&sid) {
                continue;
            }
            spawn_local(async move {
                match loader::fetch_daily_series(sid).await {
                    Ok(d) => set_daily_cache.update(|c| {
                        c.insert(sid, d);
                    }),
                    Err(e) => web_sys::console::error_1(&format!("daily {sid}: {e}").into()),
                }
            });
        }
    });

    // Years spanning the map's date range, bounded like the Series hourly load so
    // a wide hour-filtered range doesn't pull dozens of per-station-year files.
    let map_hourly_years = move || -> Vec<i32> {
        let (lo, hi) = (map_date_from.get().year(), map_date_to.get().year());
        let avail = years.get();
        let mut ys: Vec<i32> = (lo..=hi).filter(|y| avail.contains(y)).collect();
        if ys.len() > MAX_HOURLY_YEARS {
            ys = ys.split_off(ys.len() - MAX_HOURLY_YEARS); // keep newest
        }
        ys
    };

    // ── Map: fetch the hourly tier when a time-of-day window is active ──
    // Hour-of-day filtering can't come from the daily tier, so load every
    // station's hourly files for the (bounded) years spanning the map range.
    Effect::new(move |_| {
        let hour_filtered = hour_from.get() != 0 || hour_to.get() != 23;
        if view.get() != View::Map || !hour_filtered {
            return;
        }
        let cached = hourly_cache.get_untracked();
        let yrs = map_hourly_years();
        for s in stations.get() {
            let sid = s.id;
            for &y in &yrs {
                if cached.contains_key(&(sid, y)) {
                    continue;
                }
                spawn_local(async move {
                    match loader::fetch_series(sid, y).await {
                        Ok(sf) => set_hourly_cache.update(|c| {
                            c.insert((sid, y), sf);
                        }),
                        Err(e) => web_sys::console::error_1(&format!("hourly {sid}-{y}: {e}").into()),
                    }
                });
            }
        }
    });

    // ── Derived chart series (single station × substance) ──
    // Normal mode: bucket by interval on the appropriate tier. Profile modes fold
    // the range onto a short repeating base — Weekday/Weekend → a 24-hour diurnal
    // mean from the hourly tier; Weekly → a 7-point day-of-week mean from the daily
    // tier (Montreal-local; UTC-midnight daily stamps use their calendar weekday).
    // Also returns the date extent of the readings that actually feed the chart
    // (the station's record is often much shorter than the query range, and the
    // caption should describe the data shown, not the query), a coverage note
    // describing what's missing relative to the query — shown as an info chip
    // beside the caption when the two disagree — and, in ordinary (non-profile)
    // mode, the in-range readings themselves so a pinned copy of this trace can
    // be re-bucketed when the live interval changes.
    type BuiltSeries =
        (Vec<Series>, Option<(NaiveDate, NaiveDate)>, Option<String>, Option<Vec<Reading>>);
    let build_series = move || -> BuiltSeries {
        let Some(sid) = selected_station.get() else { return (vec![], None, None, None) };
        let sub = selected_substance.get();
        let prof = profile.get();
        let iv = interval.get();
        let anchor = profile_anchor();
        let l = lang.get();
        let station_name = |sid: u32| {
            stations
                .get()
                .iter()
                .find(|s| s.id == sid)
                .map(|s| s.name.clone())
                .unwrap_or_default()
        };

        // ── Long-range diurnal profile: precomputed profile tier ──
        // Sums the per-year weekday/weekend × hour-of-day bins overlapping the
        // range (year resolution), covering the whole record without hourly files.
        if let Some(p @ (Profile::Weekday | Profile::Weekend)) = prof {
            if !diurnal_uses_hourly() {
                let t = l.t();
                let cache = profile_cache.get();
                let Some(bins) = cache.get(&sid).and_then(|ps| ps.substances.get(&sub)) else {
                    return (vec![], None, None, None);
                };
                let (ylo, yhi) = (date_from.get().year(), date_to.get().year());
                let weekend = p == Profile::Weekend;
                let mut sum = [0.0_f64; 24];
                let mut cnt = [0_u64; 24];
                let mut years_present: Vec<i32> = Vec::new();
                for (yr, wd_s, wd_c, we_s, we_c) in bins {
                    if *yr < ylo || *yr > yhi {
                        continue;
                    }
                    let (s, c) = if weekend { (we_s, we_c) } else { (wd_s, wd_c) };
                    let mut any = false;
                    for h in 0..24 {
                        let n = c.get(h).copied().unwrap_or(0);
                        if n > 0 {
                            sum[h] += s.get(h).copied().unwrap_or(0.0);
                            cnt[h] += n as u64;
                            any = true;
                        }
                    }
                    if any {
                        years_present.push(*yr);
                    }
                }
                let pts: Vec<(chrono::DateTime<Utc>, f64)> = (0..24)
                    .filter(|&h| cnt[h] > 0)
                    .map(|h| (anchor + Duration::hours(h as i64), sum[h] / cnt[h] as f64))
                    .collect();
                let (Some(&first), Some(&last)) = (years_present.first(), years_present.last())
                else {
                    return (vec![], None, None, None);
                };
                if pts.is_empty() {
                    return (vec![], None, None, None);
                }
                // Extent at year resolution, clamped to the query range.
                let data_from = NaiveDate::from_ymd_opt(first, 1, 1).unwrap().max(date_from.get());
                let data_to = NaiveDate::from_ymd_opt(last, 12, 31).unwrap().min(date_to.get());
                // Coverage note: late start / early end, plus whole missing years.
                let mut parts: Vec<String> = Vec::new();
                if (data_from - date_from.get()).num_days() > 7 {
                    parts.push(format!("{} {}", t.cov_begins, data_from.format("%Y-%m-%d")));
                }
                if (date_to.get() - data_to).num_days() > 7 {
                    parts.push(format!("{} {}", t.cov_ends, data_to.format("%Y-%m-%d")));
                }
                let mut n_gaps = 0u32;
                let mut longest: Option<(i32, i32)> = None;
                for w in years_present.windows(2) {
                    if w[1] - w[0] > 1 {
                        n_gaps += 1;
                        if longest.is_none_or(|(a, b)| w[1] - w[0] > b - a) {
                            longest = Some((w[0], w[1]));
                        }
                    }
                }
                if let Some((a, b)) = longest {
                    let (ga, gb) = (format!("{}-12-31", a), format!("{}-01-01", b));
                    parts.push(if n_gaps == 1 {
                        format!("1 {}: {ga} → {gb}", t.cov_gap_singular)
                    } else {
                        format!("{n_gaps} {} ({} {ga} → {gb})", t.cov_gaps_plural, t.cov_longest)
                    });
                }
                let coverage = (!parts.is_empty()).then(|| {
                    let query = format!(
                        "{}: {} → {}",
                        t.cov_query,
                        date_from.get().format("%Y-%m-%d"),
                        date_to.get().format("%Y-%m-%d"),
                    );
                    std::iter::once(query).chain(parts).collect::<Vec<_>>().join("\n")
                });
                let label = format!(
                    "{} — {} ({})",
                    station_name(sid),
                    pollutants::name_of(&sub, l),
                    p.label(l)
                );
                return (
                    vec![Series {
                        label,
                        color: "#4a9eff".to_string(),
                        dash: String::new(),
                        points: pts,
                        max_gap_secs: None,
                    }],
                    Some((data_from, data_to)),
                    coverage,
                    None,
                );
            }
        }

        let use_hourly =
            prof.is_some_and(|p| p.needs_hourly()) || (prof.is_none() && iv == Interval::Hour);
        let mut readings = if use_hourly {
            let cache = hourly_cache.get();
            let mut rs = Vec::new();
            for y in hourly_years_in_range() {
                if let Some(sf) = cache.get(&(sid, y)) {
                    rs.extend(sf.readings(&sub));
                }
            }
            rs
        } else {
            match daily_cache.get().get(&sid) {
                Some(d) => d.readings(&sub),
                None => return (vec![], None, None, None),
            }
        };
        if readings.is_empty() {
            return (vec![], None, None, None);
        }

        // Date-range filter on the raw readings.
        let from_dt = date_from.get().and_hms_opt(0, 0, 0).map(|n| Utc.from_utc_datetime(&n));
        let to_dt = date_to.get().and_hms_opt(23, 59, 59).map(|n| Utc.from_utc_datetime(&n));
        readings.retain(|r| {
            from_dt.map_or(true, |f| r.timestamp >= f) && to_dt.map_or(true, |t| r.timestamp <= t)
        });
        if readings.is_empty() {
            return (vec![], None, None, None);
        }

        // Actual span of the in-range data (the readings are not sorted across
        // tier files, so scan rather than take first/last).
        let (data_from, data_to) = {
            let mut lo = readings[0].timestamp;
            let mut hi = readings[0].timestamp;
            for r in &readings {
                lo = lo.min(r.timestamp);
                hi = hi.max(r.timestamp);
            }
            (lo.date_naive(), hi.date_naive())
        };
        let extent = Some((data_from, data_to));

        // Coverage note: how the plotted data falls short of the query — a
        // late start, an early end, and/or long internal gaps. `None` when the
        // data covers the query (within a week) with no gap over 30 days.
        let coverage = {
            let t = lang.get().t();
            let mut parts: Vec<String> = Vec::new();
            if (data_from - date_from.get()).num_days() > 7 {
                parts.push(format!("{} {}", t.cov_begins, data_from.format("%Y-%m-%d")));
            }
            if (date_to.get() - data_to).num_days() > 7 {
                parts.push(format!("{} {}", t.cov_ends, data_to.format("%Y-%m-%d")));
            }
            let mut tss: Vec<i64> = readings.iter().map(|r| r.timestamp.timestamp()).collect();
            tss.sort_unstable();
            let mut n_gaps = 0u32;
            let mut longest: Option<(i64, i64)> = None;
            for w in tss.windows(2) {
                if w[1] - w[0] > 30 * 86_400 {
                    n_gaps += 1;
                    if longest.is_none_or(|(a, b)| w[1] - w[0] > b - a) {
                        longest = Some((w[0], w[1]));
                    }
                }
            }
            if let Some((a, b)) = longest {
                let day = |s: i64| {
                    chrono::DateTime::from_timestamp(s, 0)
                        .map(|d| d.date_naive().format("%Y-%m-%d").to_string())
                        .unwrap_or_default()
                };
                parts.push(if n_gaps == 1 {
                    format!("1 {}: {} → {}", t.cov_gap_singular, day(a), day(b))
                } else {
                    format!("{n_gaps} {} ({} {} → {})", t.cov_gaps_plural, t.cov_longest, day(a), day(b))
                });
            }
            if parts.is_empty() {
                None
            } else {
                // Lead with the query range the notes are relative to; the
                // chip's `title` tooltip renders each part on its own line.
                let query = format!(
                    "{}: {} → {}",
                    t.cov_query,
                    date_from.get().format("%Y-%m-%d"),
                    date_to.get().format("%Y-%m-%d"),
                );
                Some(std::iter::once(query).chain(parts).collect::<Vec<_>>().join("\n"))
            }
        };

        let pts: Vec<(chrono::DateTime<Utc>, f64)> = match prof {
            None => loader::aggregate(&readings, iv),
            Some(p @ (Profile::Weekday | Profile::Weekend)) => {
                let mut sum = [0.0_f64; 24];
                let mut cnt = [0u32; 24];
                for r in &readings {
                    let local = r.timestamp.with_timezone(&MontrealTz);
                    let weekend = matches!(local.weekday(), Weekday::Sat | Weekday::Sun);
                    if (p == Profile::Weekday) == weekend {
                        continue; // weekday profile drops weekends, and vice-versa
                    }
                    let h = local.hour() as usize;
                    sum[h] += r.value;
                    cnt[h] += 1;
                }
                (0..24)
                    .filter(|&h| cnt[h] > 0)
                    .map(|h| (anchor + Duration::hours(h as i64), sum[h] / cnt[h] as f64))
                    .collect()
            }
            Some(Profile::Weekly) => {
                let mut sum = [0.0_f64; 7];
                let mut cnt = [0u32; 7];
                for r in &readings {
                    let d = r.timestamp.date_naive().weekday().num_days_from_monday() as usize;
                    sum[d] += r.value;
                    cnt[d] += 1;
                }
                // Centre each point in its day cell (+12 h): the axis labels
                // each weekday at the middle of its cell, so a point at the
                // cell start would sit half a day left of its label and leave
                // the last (Sunday) seventh of the chart empty.
                (0..7)
                    .filter(|&d| cnt[d] > 0)
                    .map(|d| {
                        (
                            anchor + Duration::days(d as i64) + Duration::hours(12),
                            sum[d] / cnt[d] as f64,
                        )
                    })
                    .collect()
            }
            Some(Profile::Annual) => {
                // Seasonal (month-of-year) mean over a synthetic 365-day base,
                // each point centred in its month cell like the weekly profile.
                let mut sum = [0.0_f64; 12];
                let mut cnt = [0u32; 12];
                for r in &readings {
                    let m = r.timestamp.date_naive().month0() as usize;
                    sum[m] += r.value;
                    cnt[m] += 1;
                }
                (0..12)
                    .filter(|&m| cnt[m] > 0)
                    .map(|m| {
                        let mid = Duration::days(MONTH_START_DAYS[m])
                            + Duration::hours(MONTH_LEN_DAYS[m] * 12);
                        (anchor + mid, sum[m] / cnt[m] as f64)
                    })
                    .collect()
            }
        };
        if pts.is_empty() {
            return (vec![], None, None, None);
        }

        let name = station_name(sid);
        let label = match prof {
            None => format!("{} — {}", name, pollutants::name_of(&sub, l)),
            Some(p) => format!("{} — {} ({})", name, pollutants::name_of(&sub, l), p.label(l)),
        };
        // Ordinary-mode traces hand back their readings so a pinned copy can
        // re-bucket when the live interval changes (profiles never re-bucket).
        let live_readings = prof.is_none().then_some(readings);
        (
            vec![Series {
                label,
                color: "#4a9eff".to_string(),
                dash: String::new(),
                points: pts,
                max_gap_secs: None,
            }],
            extent,
            coverage,
            live_readings,
        )
    };
    let (chart_series, set_chart_series) = signal::<Vec<Series>>(vec![]);
    let (chart_extent, set_chart_extent) = signal::<Option<(NaiveDate, NaiveDate)>>(None);
    let (chart_coverage, set_chart_coverage) = signal::<Option<String>>(None);
    let (chart_readings, set_chart_readings) = signal::<Option<Vec<Reading>>>(None);
    Effect::new(move |_| {
        let (series, extent, coverage, readings) = build_series();
        set_chart_series.set(series);
        set_chart_extent.set(extent);
        set_chart_coverage.set(coverage);
        set_chart_readings.set(readings);
    });

    // ── Trace comparison: pinned snapshots + the live trace ──
    // "Pin" freezes the current trace (relabelled with its data span, recoloured
    // from the pin palette) so the controls can be reconfigured into further
    // traces on the same axes — e.g. the same weekday profile across several
    // eras. Switching the x-axis basis (ordinary time ↔ diurnal ↔ weekly)
    // clears the pins, since traces on different bases can't overlay.
    let (pinned, set_pinned) = signal::<Vec<PinnedTrace>>(vec![]);
    Effect::new(move |_| {
        let kind = axis_kind(profile.get());
        if pinned.with_untracked(|ps| ps.iter().any(|p| p.axis != kind)) {
            set_pinned.set(vec![]);
        }
    });
    let on_pin = Callback::new(move |_: ()| {
        let Some(mut s) = chart_series.get_untracked().first().cloned() else { return };
        if pinned.with_untracked(|ps| ps.len() >= MAX_PINS) {
            return;
        }
        // Stamp the label with the span of data behind the snapshot, so the
        // legend identifies which era the pinned curve describes.
        if let Some((f, t)) = chart_extent.get_untracked() {
            s.label = format!("{} · {} → {}", s.label, f.format("%Y-%m-%d"), t.format("%Y-%m-%d"));
        }
        s.color = pinned.with_untracked(|ps| next_pin_color(ps)).to_string();
        let substance = selected_substance.get_untracked();
        let axis = axis_kind(profile.get_untracked());
        set_pinned.update(|ps| {
            ps.push(PinnedTrace {
                series: s,
                unit: pollutants::unit_of(&substance),
                substance,
                axis,
                // Ordinary pins keep their readings to follow interval changes.
                readings: (axis == 0).then(|| chart_readings.get_untracked()).flatten(),
                hourly: axis == 0 && interval.get_untracked() == Interval::Hour,
            })
        });
    });
    // Remove one pin by its position in the pinned list (the sidebar chips).
    let on_unpin = Callback::new(move |idx: usize| {
        set_pinned.update(|ps| {
            if idx < ps.len() {
                ps.remove(idx);
            }
        });
    });
    let on_unpin_all = Callback::new(move |_: ()| set_pinned.set(vec![]));

    // A comparison mixing substances can't share an absolute y-axis — even
    // with matching nominal units, dynamic ranges differ wildly — so the chart
    // switches to a normalized axis: each trace scaled to % of its own maximum.
    let normalized = Signal::derive(move || {
        let live = selected_substance.get();
        pinned.with(|ps| ps.iter().any(|p| p.substance != live))
    });

    // What the chart draws: pinned snapshots (oldest first) under the live trace.
    // While a comparison is active, the live trace's legend label is stamped
    // with its data span just like the pinned ones (and the caption drops its
    // date range), so every legend row identifies its own era. On a normalized
    // axis each label also records what 100 % means for that trace.
    let display_series = Signal::derive(move || {
        let live_unit = pollutants::unit_of(&selected_substance.get());
        let iv = interval.get();
        let mut traces: Vec<(Series, &'static str)> = pinned
            .get()
            .into_iter()
            .map(|p| {
                let mut s = p.series;
                // Ordinary pins follow the live interval by re-bucketing their
                // stored readings. A daily-resolution pin can't honestly make
                // hourly buckets, so it degrades to Day under the Hour interval
                // (carrying its own line-break threshold so it stays a line).
                if p.axis == 0 {
                    if let Some(rs) = &p.readings {
                        let eff =
                            if iv == Interval::Hour && !p.hourly { Interval::Day } else { iv };
                        s.points = loader::aggregate(rs, eff);
                        s.max_gap_secs =
                            Some(crate::components::chart::gap_threshold_secs(eff));
                    }
                }
                (s, p.unit)
            })
            .collect();
        let comparing = !traces.is_empty();
        for mut s in chart_series.get() {
            if comparing {
                if let Some((f, t)) = chart_extent.get() {
                    s.label =
                        format!("{} · {} → {}", s.label, f.format("%Y-%m-%d"), t.format("%Y-%m-%d"));
                }
            }
            traces.push((s, live_unit));
        }
        if normalized.get() {
            for (s, unit) in &mut traces {
                let max = s.points.iter().map(|(_, v)| *v).fold(f64::NEG_INFINITY, f64::max);
                if max > 0.0 {
                    for p in &mut s.points {
                        p.1 = p.1 / max * 100.0;
                    }
                    let mv = crate::components::chart::fmt_val(max);
                    s.label = if unit.is_empty() {
                        format!("{} — 100% = {mv}", s.label)
                    } else {
                        format!("{} — 100% = {mv} {unit}", s.label)
                    };
                }
            }
        }
        traces.into_iter().map(|(s, _)| s).collect::<Vec<Series>>()
    });
    let can_pin = Signal::derive(move || {
        !chart_series.get().is_empty() && pinned.with(|ps| ps.len() < MAX_PINS)
    });
    // Sidebar chips: (label, colour) per pin, in pin order.
    let pinned_chips = Signal::derive(move || {
        pinned
            .get()
            .into_iter()
            .map(|p| (p.series.label, p.series.color))
            .collect::<Vec<_>>()
    });

    let y_title = Signal::derive(move || {
        if normalized.get() {
            return lang.get().t().norm_axis.to_string();
        }
        let sub = selected_substance.get();
        pollutants::display_label(&sub, lang.get())
    });

    // Self-describing caption for the chart (and its PNG export): the full
    // selection a reader needs to interpret the image.
    let chart_caption = Signal::derive(move || {
        let l = lang.get();
        let name = selected_station
            .get()
            .and_then(|sid| stations.get().iter().find(|s| s.id == sid).map(|s| s.name.clone()))
            .unwrap_or_default();
        let sub = pollutants::display_label(&selected_substance.get(), l);
        // The aggregation slot shows the profile when one is active, else the interval.
        let mode = match profile.get() {
            Some(p) => format!("{} {}", p.label(l), l.t().profile.to_lowercase()),
            None => interval.get().label(l).to_string(),
        };
        // While a comparison is active the traces span different ranges, so a
        // single date range in the title would misdescribe the chart — omit it
        // (each legend row carries its own span instead).
        if pinned.with(|ps| !ps.is_empty()) {
            return format!("{name} · {sub} · {mode}");
        }
        // Date slot: the span of the data actually plotted — the station's
        // record is often much shorter than the query range, and a caption
        // showing the query would contradict the axis. Falls back to the
        // query range while nothing is plotted (loading / no data).
        let (from, to) = match chart_extent.get() {
            Some((f, t)) => (f, t),
            None => (date_from.get(), date_to.get()),
        };
        format!(
            "{name} · {sub} · {mode} · {} → {}",
            from.format("%Y-%m-%d"),
            to.format("%Y-%m-%d"),
        )
    });

    // Fixed x-axis range for profile modes (synthetic 24-hour / 7-day / 365-day base).
    let x_range = Signal::derive(move || {
        let a = profile_anchor();
        match profile.get() {
            None => None,
            Some(Profile::Weekday | Profile::Weekend) => Some((a, a + Duration::hours(24))),
            Some(Profile::Weekly) => Some((a, a + Duration::days(7))),
            Some(Profile::Annual) => Some((a, a + Duration::days(365))),
        }
    });

    // IQA acceptability thresholds, drawn as reference lines on the chart when
    // the index is selected (empty for ordinary concentrations — and on the
    // normalized comparison axis, where absolute index values don't apply).
    let iqa_thresholds = Signal::derive(move || {
        if selected_substance.get() == "IQA" && !normalized.get() {
            let t = lang.get().t();
            vec![(25.0, t.iqa_acceptable.to_string()), (50.0, t.iqa_poor.to_string())]
        } else {
            Vec::new()
        }
    });

    // ── Header data chip ──
    let data_chip = move || -> Option<_> {
        let m = meta.get()?;
        let t = lang.get().t();
        // Show the full published span so the multi-decade scope is obvious.
        let label = format!("{} {}–{}", t.data_prefix, m.min_year, m.max_year);
        let tip = format!(
            "{}: {} · {}: {}",
            t.latest_year_label, m.max_year, t.generated, m.generated
        );
        let url = m.source_url.clone();
        Some(view! {
            <a href=url title=tip target="_blank" rel="noopener noreferrer">{label}</a>
        })
    };

    // ── Callbacks ──
    // Switch the active view and record it in history (`?view=<slug>`), so deep
    // links are shareable and Back/Forward step between tabs. Clicking the
    // already-active tab is a no-op (no duplicate history entry).
    let go = Callback::new(move |v: View| {
        if view.get_untracked() == v {
            return;
        }
        set_view.set(v);
        push_view_url(v);
    });
    let on_substance = Callback::new(move |s: String| set_selected_substance.set(s));
    let on_stat = Callback::new(move |s: Stat| set_stat.set(s));
    // Map date range: keep from ≤ to by clamping the other end, mirroring the
    // hour/year-range callbacks.
    let on_map_date_from = Callback::new(move |d: NaiveDate| {
        set_map_date_from.set(d);
        if map_date_to.get_untracked() < d {
            set_map_date_to.set(d);
        }
    });
    let on_map_date_to = Callback::new(move |d: NaiveDate| {
        set_map_date_to.set(d);
        if map_date_from.get_untracked() > d {
            set_map_date_from.set(d);
        }
    });
    let on_map_date_preset = Callback::new(move |(f, t): (NaiveDate, NaiveDate)| {
        set_map_date_from.set(f);
        set_map_date_to.set(t);
    });
    // Time-of-day window: keep from ≤ to (no overnight wrap), clamping the other
    // end like the year-range callbacks do.
    let on_hour_from = Callback::new(move |h: u8| {
        set_hour_from.set(h);
        if hour_to.get_untracked() < h {
            set_hour_to.set(h);
        }
    });
    let on_hour_to = Callback::new(move |h: u8| {
        set_hour_to.set(h);
        if hour_from.get_untracked() > h {
            set_hour_from.set(h);
        }
    });
    let on_hour_range = Callback::new(move |(f, t): (u8, u8)| {
        set_hour_from.set(f);
        set_hour_to.set(t);
    });
    let on_day_type = Callback::new(move |d: DayType| set_day_type.set(d));
    let on_show_names = Callback::new(move |b: bool| set_show_names.set(b));
    let on_station = Callback::new(move |id: u32| set_selected_station.set(Some(id)));
    let on_interval = Callback::new(move |iv: Interval| set_interval.set(iv));
    // Toggle a profile: clicking the active one turns it back off.
    let on_profile = Callback::new(move |p: Profile| {
        set_profile.update(|cur| *cur = if *cur == Some(p) { None } else { Some(p) });
    });
    let on_date_from = Callback::new(move |d: NaiveDate| set_date_from.set(d));
    let on_date_to = Callback::new(move |d: NaiveDate| set_date_to.set(d));
    let on_date_preset = Callback::new(move |(f, t): (NaiveDate, NaiveDate)| {
        set_date_from.set(f);
        set_date_to.set(t);
    });

    view! {
        <div id="app"
             class:sidebar-open=move || sidebar_open.get()
             class:info-view=move || view.get().is_info()>
            <header>
                <button class="mobile-toggle"
                        on:click=move |_| set_sidebar_open.update(|v| *v = !*v)>
                    {move || {
                        let t = lang.get().t();
                        if sidebar_open.get() { t.mobile_close } else { t.mobile_filters }
                    }}
                </button>
                <h1>
                    <a href="https://github.com/OpenMobilityData/AirQualityMTL"
                       target="_blank" rel="noopener noreferrer">"AirQualityMTL"</a>
                </h1>
                <span class="subtitle">{move || lang.get().t().subtitle}</span>

                <div class="view-toggle">
                    <button class=move || if view.get() == View::Map { "active" } else { "" }
                            on:click=move |_| go.run(View::Map)>
                        {move || lang.get().t().view_map}
                    </button>
                    <button class=move || if view.get() == View::Series { "active" } else { "" }
                            on:click=move |_| go.run(View::Series)>
                        {move || lang.get().t().view_series}
                    </button>
                    <button class=move || if view.get() == View::Network { "active" } else { "" }
                            on:click=move |_| go.run(View::Network)>
                        {move || lang.get().t().view_network}
                    </button>
                    <button class=move || if view.get() == View::Methods { "active" } else { "" }
                            on:click=move |_| go.run(View::Methods)>
                        {move || lang.get().t().view_methods}
                    </button>
                    <button class=move || if view.get() == View::Limits { "active" } else { "" }
                            on:click=move |_| go.run(View::Limits)>
                        {move || lang.get().t().view_limits}
                    </button>
                    // UFP Model sits after Limits: Map→Limits all concern the
                    // city's monitoring program for regulated pollutants, while
                    // UFPs are modelled and not currently regulated.
                    <button class=move || if view.get() == View::Ufp { "active" } else { "" }
                            on:click=move |_| go.run(View::Ufp)>
                        {move || lang.get().t().view_ufp}
                    </button>
                    <button class=move || if view.get() == View::Links { "active" } else { "" }
                            on:click=move |_| go.run(View::Links)>
                        {move || lang.get().t().view_links}
                    </button>
                </div>

                <span class="data-status">{data_chip}</span>

                <button class="lang-toggle" title="Language / Langue"
                        on:click=move |_| set_lang.update(|l| *l = l.other())>
                    {move || lang.get().other().short_label()}
                </button>
            </header>

            <Sidebar
                view=view
                substance_options=substance_options
                selected_substance=selected_substance
                on_substance=on_substance
                stat=stat
                on_stat=on_stat
                map_date_from=map_date_from
                map_date_to=map_date_to
                on_map_date_from=on_map_date_from
                on_map_date_to=on_map_date_to
                on_map_date_preset=on_map_date_preset
                hour_from=hour_from
                hour_to=hour_to
                on_hour_from=on_hour_from
                on_hour_to=on_hour_to
                on_hour_range=on_hour_range
                day_type=day_type
                on_day_type=on_day_type
                show_names=show_names
                on_show_names=on_show_names
                stations=stations
                selected_station=selected_station
                on_station=on_station
                interval=interval
                on_interval=on_interval
                profile=profile
                on_profile=on_profile
                date_from=date_from
                date_to=date_to
                on_date_from=on_date_from
                on_date_to=on_date_to
                date_presets=date_presets
                on_date_preset=on_date_preset
                pinned_chips=pinned_chips
                can_pin=can_pin
                on_pin=on_pin
                on_unpin=on_unpin
                on_unpin_all=on_unpin_all
            />

            <main>
                {move || match view.get() {
                    View::Map => view! {
                        <RegionMap
                            stations=stations
                            daily_cache=daily_cache
                            hourly_cache=hourly_cache
                            iqa_dominance=iqa_dominance
                            date_from=map_date_from
                            date_to=map_date_to
                            substance=selected_substance
                            stat=stat
                            hour_from=hour_from
                            hour_to=hour_to
                            day_type=day_type
                            show_names=show_names
                        />
                    }.into_any(),
                    View::Series => view! {
                        <Chart series=display_series interval=interval y_title=y_title
                               thresholds=iqa_thresholds caption=chart_caption
                               coverage=chart_coverage.into() norm_note=normalized
                               profile=profile.into() x_range=x_range />
                    }.into_any(),
                    View::Ufp => view! { <UfpView surface=ufp_surface /> }.into_any(),
                    View::Network => view! { <InfoPage kind=InfoKind::Network /> }.into_any(),
                    View::Methods => view! { <InfoPage kind=InfoKind::Methods /> }.into_any(),
                    View::Limits => view! { <InfoPage kind=InfoKind::Limits /> }.into_any(),
                    View::Links => view! { <InfoPage kind=InfoKind::Links /> }.into_any(),
                }}
                // Discrete disclaimer footer on the data views only (Map / Series);
                // the article views carry their own sourcing, so it's omitted there.
                {move || {
                    let v = view.get();
                    matches!(v, View::Map | View::Series).then(|| {
                        let t = lang.get().t();
                        // The interpolated heatmap is Map-only, so note it there.
                        let interp = (v == View::Map).then_some(t.interp_note);
                        view! {
                            <p class="disclaimer">
                                {t.disclaimer}
                                {interp.map(|n| view! { " · "{n} })}
                            </p>
                        }
                    })
                }}
            </main>
        </div>
    }
}
