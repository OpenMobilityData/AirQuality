use chrono::NaiveDate;
use leptos::prelude::*;

use crate::data::pollutants;
use crate::data::types::{DayType, Interval, Profile, Stat, Station, View};
use crate::i18n::Lang;

/// The filter sidebar. Shows the substance/statistic pickers in Map view and
/// the station/substance/interval/date pickers in Time-series view.
#[component]
pub fn Sidebar(
    view: ReadSignal<View>,

    /// Substances offered in the current context (all measured ones in Map
    /// view; the selected station's substances in Series view).
    substance_options: Memo<Vec<String>>,
    selected_substance: ReadSignal<String>,
    on_substance: Callback<String>,

    stat: ReadSignal<Stat>,
    on_stat: Callback<Stat>,

    /// Map averaging window: an arbitrary date range (own signals, separate from
    /// the Series date range so each view keeps its own default).
    map_date_from: ReadSignal<NaiveDate>,
    map_date_to: ReadSignal<NaiveDate>,
    on_map_date_from: Callback<NaiveDate>,
    on_map_date_to: Callback<NaiveDate>,
    on_map_date_preset: Callback<(NaiveDate, NaiveDate)>,

    /// Map time-of-day window (inclusive local hours 0..23) and day-type filter.
    hour_from: ReadSignal<u8>,
    hour_to: ReadSignal<u8>,
    on_hour_from: Callback<u8>,
    on_hour_to: Callback<u8>,
    on_hour_range: Callback<(u8, u8)>,
    day_type: ReadSignal<DayType>,
    on_day_type: Callback<DayType>,

    /// Whether the map draws station names.
    show_names: ReadSignal<bool>,
    on_show_names: Callback<bool>,

    stations: ReadSignal<Vec<Station>>,
    selected_station: ReadSignal<Option<u32>>,
    on_station: Callback<u32>,

    interval: ReadSignal<Interval>,
    on_interval: Callback<Interval>,

    profile: ReadSignal<Option<Profile>>,
    on_profile: Callback<Profile>,

    date_from: ReadSignal<NaiveDate>,
    date_to: ReadSignal<NaiveDate>,
    on_date_from: Callback<NaiveDate>,
    on_date_to: Callback<NaiveDate>,

    date_presets: ReadSignal<Vec<(String, NaiveDate, NaiveDate)>>,
    on_date_preset: Callback<(NaiveDate, NaiveDate)>,
) -> impl IntoView {
    let lang = use_context::<ReadSignal<Lang>>().expect("Lang context not provided");

    // Reusable substance <select> (used by both views).
    let substance_picker = move || {
        let l = lang.get();
        let opts = substance_options.get();
        let sel = selected_substance.get();
        view! {
            <select class="substance-select"
                    on:change=move |e| on_substance.run(event_target_value(&e))>
                {opts.into_iter().map(|key| {
                    let label = pollutants::display_label(&key, l);
                    let is_sel = key == sel;
                    view! { <option value=key.clone() selected=is_sel>{label}</option> }
                }).collect_view()}
            </select>
        }
    };

    view! {
        <aside>
            // ── Map view: Date range + Substance + Statistic + Time filters ──
            <Show when=move || view.get() == View::Map>
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().date_range}</label>
                    // Same quick presets as the Series view (All / Last 10y / 5y /
                    // year / 3 months); no interval-based disabling here.
                    <div class="btn-group">
                        {move || {
                            date_presets.get().into_iter().map(|(label, from, to)| {
                                view! {
                                    <button on:click=move |_| on_map_date_preset.run((from, to))>
                                        {label}
                                    </button>
                                }
                            }).collect_view()
                        }}
                    </div>
                </div>
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().custom_range}</label>
                    <div class="date-range">
                        <input type="date"
                               prop:value=move || map_date_from.get().format("%Y-%m-%d").to_string()
                               on:input=move |e| {
                                   if let Ok(d) = NaiveDate::parse_from_str(&event_target_value(&e), "%Y-%m-%d") {
                                       on_map_date_from.run(d);
                                   }
                               }/>
                        <input type="date"
                               prop:value=move || map_date_to.get().format("%Y-%m-%d").to_string()
                               on:input=move |e| {
                                   if let Ok(d) = NaiveDate::parse_from_str(&event_target_value(&e), "%Y-%m-%d") {
                                       on_map_date_to.run(d);
                                   }
                               }/>
                    </div>
                </div>
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().substance}</label>
                    {substance_picker}
                </div>
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().statistic}</label>
                    <div class="btn-group">
                        {Stat::all().iter().map(|&s| {
                            view! {
                                <button
                                    class=move || if stat.get() == s { "active" } else { "" }
                                    on:click=move |_| on_stat.run(s)>
                                    {move || s.label(lang.get())}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </div>
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().time_of_day}</label>
                    // "All hours" resets to the full day (and avoids loading the
                    // detailed stats file until the user actually narrows the window).
                    <div class="btn-group">
                        {move || {
                            let active = hour_from.get() == 0 && hour_to.get() == 23;
                            view! {
                                <button class=if active { "active" } else { "" }
                                        on:click=move |_| on_hour_range.run((0, 23))>
                                    {lang.get().t().all_hours}
                                </button>
                            }
                        }}
                    </div>
                    // From start-of-hour → end-of-hour (labelled :59 so the window
                    // reads inclusively, e.g. 07:00 → 09:59 = the 7, 8 and 9 o'clock hours).
                    <div class="year-range">
                        <select class="substance-select"
                                on:change=move |e| {
                                    if let Ok(h) = event_target_value(&e).parse::<u8>() {
                                        on_hour_from.run(h);
                                    }
                                }>
                            {move || {
                                let sel = hour_from.get();
                                (0u8..24).map(|h| {
                                    view! { <option value=h.to_string() selected=h == sel>{format!("{h:02}:00")}</option> }
                                }).collect_view()
                            }}
                        </select>
                        <span class="year-range-sep">"→"</span>
                        <select class="substance-select"
                                on:change=move |e| {
                                    if let Ok(h) = event_target_value(&e).parse::<u8>() {
                                        on_hour_to.run(h);
                                    }
                                }>
                            {move || {
                                let sel = hour_to.get();
                                (0u8..24).map(|h| {
                                    view! { <option value=h.to_string() selected=h == sel>{format!("{h:02}:59")}</option> }
                                }).collect_view()
                            }}
                        </select>
                    </div>
                </div>
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().day_type}</label>
                    <div class="btn-group">
                        {DayType::all().iter().map(|&d| {
                            view! {
                                <button
                                    class=move || if day_type.get() == d { "active" } else { "" }
                                    on:click=move |_| on_day_type.run(d)>
                                    {move || d.label(lang.get())}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </div>
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().station_names}</label>
                    <div class="btn-group">
                        <button
                            class=move || if show_names.get() { "" } else { "active" }
                            on:click=move |_| on_show_names.run(false)>
                            {move || lang.get().t().hide}
                        </button>
                        <button
                            class=move || if show_names.get() { "active" } else { "" }
                            on:click=move |_| on_show_names.run(true)>
                            {move || lang.get().t().show}
                        </button>
                    </div>
                </div>
            </Show>

            // ── Series view: Station + Substance + Aggregation + Date ──
            <Show when=move || view.get() == View::Series>
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().station}</label>
                    <div class="source-list">
                        {move || {
                            let sel = selected_station.get();
                            stations.get().into_iter().map(|s| {
                                let id = s.id;
                                let is_sel = sel == Some(id);
                                let borough = s.borough.clone();
                                view! {
                                    <div
                                        class=if is_sel { "source-item selected" } else { "source-item" }
                                        on:click=move |_| on_station.run(id)>
                                        <span class="source-dot"></span>
                                        <div>
                                            <div class="source-name">{s.name.clone()}</div>
                                            <div class="source-sub">{borough}</div>
                                        </div>
                                    </div>
                                }
                            }).collect_view()
                        }}
                    </div>
                </div>

                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().substance}</label>
                    {substance_picker}
                </div>

                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().interval}</label>
                    <div class="btn-group">
                        {Interval::all().iter().map(|&iv| {
                            // A profile defines its own time base, so the interval is inert then.
                            let disabled = move || profile.get().is_some();
                            view! {
                                <button
                                    disabled=disabled
                                    class=move || if profile.get().is_none() && interval.get() == iv { "active" } else { "" }
                                    on:click=move |_| on_interval.run(iv)>
                                    {move || iv.label(lang.get())}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </div>

                // ── Averaging profile (Weekday / Weekend / Weekly) ──
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().profile}</label>
                    <div class="btn-group">
                        {Profile::all().iter().map(|&p| {
                            view! {
                                <button
                                    class=move || if profile.get() == Some(p) { "active" } else { "" }
                                    on:click=move |_| on_profile.run(p)>
                                    {move || p.label(lang.get())}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                </div>

                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().date_range}</label>
                    <div class="btn-group">
                        {move || {
                            let res = interval.get();
                            let l = lang.get();
                            let min_days: i64 = match res {
                                Interval::Hour | Interval::Day => 0,
                                Interval::Week => 14,
                                Interval::Month => 60,
                                Interval::Year => 400,
                            };
                            date_presets.get().into_iter().map(|(label, from, to)| {
                                let days = (to - from).num_days();
                                let disabled = days < min_days;
                                let title = if disabled {
                                    format!("{}", l.t().range_too_short)
                                } else { String::new() };
                                view! {
                                    <button disabled=disabled title=title
                                        on:click=move |_| on_date_preset.run((from, to))>
                                        {label}
                                    </button>
                                }
                            }).collect_view()
                        }}
                    </div>
                </div>

                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().custom_range}</label>
                    <div class="date-range">
                        <input type="date"
                               prop:value=move || date_from.get().format("%Y-%m-%d").to_string()
                               on:input=move |e| {
                                   if let Ok(d) = NaiveDate::parse_from_str(&event_target_value(&e), "%Y-%m-%d") {
                                       on_date_from.run(d);
                                   }
                               }/>
                        <input type="date"
                               prop:value=move || date_to.get().format("%Y-%m-%d").to_string()
                               on:input=move |e| {
                                   if let Ok(d) = NaiveDate::parse_from_str(&event_target_value(&e), "%Y-%m-%d") {
                                       on_date_to.run(d);
                                   }
                               }/>
                    </div>
                </div>
            </Show>
        </aside>
    }
}
