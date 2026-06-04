mod components;
mod data;
mod i18n;

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use components::chart::{Chart, Series};
use components::controls::Sidebar;
use components::info::{InfoKind, InfoPage};
use components::map::RegionMap;
use data::loader::{self, DailySeries, IqaDominanceMap, MapStats, Meta, SeriesFile};
use data::pollutants;
use data::types::{Interval, Stat, Station, View};
use i18n::Lang;

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// Preferred default substance when the current one is unavailable: PM2.5
/// (measured at every station, all years), then the IQA index, else the first.
fn default_substance(opts: &[String]) -> String {
    opts.iter()
        .find(|s| s.as_str() == "PM2.5")
        .or_else(|| opts.iter().find(|s| s.as_str() == "IQA"))
        .or_else(|| opts.first())
        .cloned()
        .unwrap_or_default()
}

#[component]
fn App() -> impl IntoView {
    // ── Language (persisted; provided as context) ──
    let (lang, set_lang) = signal(Lang::from_browser());
    Effect::new(move |_| lang.get().store());
    provide_context(lang);

    // ── Core data ──
    let (stations, set_stations) = signal::<Vec<Station>>(vec![]);
    let (map_stats, set_map_stats) = signal::<MapStats>(BTreeMap::new());
    let (iqa_dominance, set_iqa_dominance) = signal::<IqaDominanceMap>(BTreeMap::new());
    let (meta, set_meta) = signal::<Option<Meta>>(None);
    let (active_subs, set_active_subs) = signal::<Vec<String>>(vec![]);
    let (years, set_years) = signal::<Vec<i32>>(vec![]);
    // Daily tier (all years, one file per station) drives Day/Week/Month/Year;
    // hourly tier (per station-year) is loaded on demand for the Hour interval.
    let (daily_cache, set_daily_cache) = signal::<BTreeMap<u32, DailySeries>>(BTreeMap::new());
    let (hourly_cache, set_hourly_cache) =
        signal::<BTreeMap<(u32, i32), SeriesFile>>(BTreeMap::new());

    // ── UI state ──
    let (view, set_view) = signal(View::Map);
    let (selected_substance, set_selected_substance) = signal(String::from("PM2.5"));
    let (stat, set_stat) = signal(Stat::Mean);
    // Map summary range: an inclusive [from, to] window of years. Defaults to
    // the whole record (set from meta), so the map opens on the full overview.
    let (year_from, set_year_from) = signal(1986);
    let (year_to, set_year_to) = signal(2024);
    let (selected_station, set_selected_station) = signal::<Option<u32>>(None);
    let (interval, set_interval) = signal(Interval::Month);
    let (date_from, set_date_from) = signal(NaiveDate::from_ymd_opt(1986, 1, 1).unwrap());
    let (date_to, set_date_to) = signal(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap());
    let (date_presets, set_date_presets) = signal::<Vec<(String, NaiveDate, NaiveDate)>>(vec![]);
    let (sidebar_open, set_sidebar_open) = signal(false);

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
        match loader::fetch_map_stats().await {
            Ok(ms) => set_map_stats.set(ms),
            Err(e) => web_sys::console::error_1(&format!("map-stats: {e}").into()),
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
                    set_date_from.set(s);
                    set_date_to.set(e);
                }
                // Map defaults to the latest year (a single-year snapshot) rather
                // than the whole record — the most common entry point.
                set_year_from.set(m.max_year);
                set_year_to.set(m.max_year);
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
            View::Map | View::Network | View::Methods => subs,
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

    // ── Fetch hourly per-year files on demand (Hour interval only) ──
    Effect::new(move |_| {
        if view.get() != View::Series || interval.get() != Interval::Hour {
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

    // ── Derived chart series (single station × substance), tiered by interval ──
    let build_series = move || -> Vec<Series> {
        let Some(sid) = selected_station.get() else { return vec![] };
        let sub = selected_substance.get();
        let iv = interval.get();

        // Collect raw readings from the appropriate tier.
        let readings = if iv == Interval::Hour {
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
                None => return vec![],
            }
        };
        if readings.is_empty() {
            return vec![];
        }
        let mut pts = loader::aggregate(&readings, iv);

        let from_dt = date_from.get().and_hms_opt(0, 0, 0).map(|n| Utc.from_utc_datetime(&n));
        let to_dt = date_to.get().and_hms_opt(23, 59, 59).map(|n| Utc.from_utc_datetime(&n));
        pts.retain(|(dt, _)| {
            from_dt.map_or(true, |f| *dt >= f) && to_dt.map_or(true, |t| *dt <= t)
        });
        if pts.is_empty() {
            return vec![];
        }

        let l = lang.get();
        let name = stations
            .get()
            .iter()
            .find(|s| s.id == sid)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        vec![Series {
            label: format!("{} — {}", name, pollutants::name_of(&sub, l)),
            color: "#4a9eff".to_string(),
            dash: String::new(),
            points: pts,
        }]
    };
    let (chart_series, set_chart_series) = signal::<Vec<Series>>(vec![]);
    Effect::new(move |_| set_chart_series.set(build_series()));

    let y_title = Signal::derive(move || {
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
        let iv = interval.get().label(l);
        format!(
            "{name} · {sub} · {iv} · {} → {}",
            date_from.get().format("%Y-%m-%d"),
            date_to.get().format("%Y-%m-%d"),
        )
    });

    // IQA acceptability thresholds, drawn as reference lines on the chart when
    // the index is selected (empty for ordinary concentrations).
    let iqa_thresholds = Signal::derive(move || {
        if selected_substance.get() == "IQA" {
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
    let on_substance = Callback::new(move |s: String| set_selected_substance.set(s));
    let on_stat = Callback::new(move |s: Stat| set_stat.set(s));
    let on_year_from = Callback::new(move |y: i32| {
        set_year_from.set(y);
        if year_to.get_untracked() < y {
            set_year_to.set(y);
        }
    });
    let on_year_to = Callback::new(move |y: i32| {
        set_year_to.set(y);
        if year_from.get_untracked() > y {
            set_year_from.set(y);
        }
    });
    // Quick-range presets set both bounds at once (e.g. "Latest" → a single year).
    let on_year_range = Callback::new(move |(f, t): (i32, i32)| {
        set_year_from.set(f);
        set_year_to.set(t);
    });
    let on_station = Callback::new(move |id: u32| set_selected_station.set(Some(id)));
    let on_interval = Callback::new(move |iv: Interval| set_interval.set(iv));
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
                <h1>"AirQuality"</h1>
                <span class="subtitle">{move || lang.get().t().subtitle}</span>

                <div class="view-toggle">
                    <button class=move || if view.get() == View::Map { "active" } else { "" }
                            on:click=move |_| set_view.set(View::Map)>
                        {move || lang.get().t().view_map}
                    </button>
                    <button class=move || if view.get() == View::Series { "active" } else { "" }
                            on:click=move |_| set_view.set(View::Series)>
                        {move || lang.get().t().view_series}
                    </button>
                    <button class=move || if view.get() == View::Network { "active" } else { "" }
                            on:click=move |_| set_view.set(View::Network)>
                        {move || lang.get().t().view_network}
                    </button>
                    <button class=move || if view.get() == View::Methods { "active" } else { "" }
                            on:click=move |_| set_view.set(View::Methods)>
                        {move || lang.get().t().view_methods}
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
                years=years
                year_from=year_from
                year_to=year_to
                on_year_from=on_year_from
                on_year_to=on_year_to
                on_year_range=on_year_range
                stations=stations
                selected_station=selected_station
                on_station=on_station
                interval=interval
                on_interval=on_interval
                date_from=date_from
                date_to=date_to
                on_date_from=on_date_from
                on_date_to=on_date_to
                date_presets=date_presets
                on_date_preset=on_date_preset
            />

            <main>
                {move || match view.get() {
                    View::Map => view! {
                        <RegionMap
                            stations=stations
                            map_stats=map_stats
                            iqa_dominance=iqa_dominance
                            year_from=year_from
                            year_to=year_to
                            substance=selected_substance
                            stat=stat
                        />
                    }.into_any(),
                    View::Series => view! {
                        <Chart series=chart_series interval=interval y_title=y_title
                               thresholds=iqa_thresholds caption=chart_caption />
                    }.into_any(),
                    View::Network => view! { <InfoPage kind=InfoKind::Network /> }.into_any(),
                    View::Methods => view! { <InfoPage kind=InfoKind::Methods /> }.into_any(),
                }}
            </main>
        </div>
    }
}
