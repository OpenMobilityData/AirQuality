use chrono::NaiveDate;
use leptos::prelude::*;

use crate::data::pollutants;
use crate::data::types::{Interval, Profile, Stat, Station, View};
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

    years: ReadSignal<Vec<i32>>,
    year_from: ReadSignal<i32>,
    year_to: ReadSignal<i32>,
    on_year_from: Callback<i32>,
    on_year_to: Callback<i32>,
    on_year_range: Callback<(i32, i32)>,

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
            // ── Map view: Year range + Substance + Statistic ──
            <Show when=move || view.get() == View::Map>
                <div class="control-group">
                    <label class="section-label">{move || lang.get().t().year_range}</label>
                    // Quick ranges so common picks (newest year, full record) are one click,
                    // avoiding a scroll through 39 years in the From dropdown.
                    <div class="btn-group">
                        {move || {
                            let l = lang.get();
                            let ys = years.get();
                            let (mn, mx) = match (ys.iter().min(), ys.iter().max()) {
                                (Some(&a), Some(&b)) => (a, b),
                                _ => return ().into_any(),
                            };
                            let (f, t) = (year_from.get(), year_to.get());
                            // (label, from, to)
                            let presets = [
                                (l.t().all_years.to_string(), mn, mx),
                                (l.t().last_10_years.to_string(), (mx - 9).max(mn), mx),
                                (l.t().last_5_years.to_string(), (mx - 4).max(mn), mx),
                                (format!("{} ({})", l.t().latest_year, mx), mx, mx),
                            ];
                            presets.into_iter().map(|(label, pf, pt)| {
                                let active = f == pf && t == pt;
                                view! {
                                    <button class=if active { "active" } else { "" }
                                            on:click=move |_| on_year_range.run((pf, pt))>
                                        {label}
                                    </button>
                                }
                            }).collect_view().into_any()
                        }}
                    </div>
                    <div class="year-range">
                        <select class="substance-select"
                                on:change=move |e| {
                                    if let Ok(y) = event_target_value(&e).parse::<i32>() {
                                        on_year_from.run(y);
                                    }
                                }>
                            {move || {
                                let sel = year_from.get();
                                let mut ys = years.get();
                                ys.sort_unstable_by(|a, b| b.cmp(a));
                                ys.into_iter().map(|y| {
                                    view! { <option value=y.to_string() selected=y == sel>{y.to_string()}</option> }
                                }).collect_view()
                            }}
                        </select>
                        <span class="year-range-sep">"→"</span>
                        <select class="substance-select"
                                on:change=move |e| {
                                    if let Ok(y) = event_target_value(&e).parse::<i32>() {
                                        on_year_to.run(y);
                                    }
                                }>
                            {move || {
                                let sel = year_to.get();
                                let mut ys = years.get();
                                ys.sort_unstable_by(|a, b| b.cmp(a));
                                ys.into_iter().map(|y| {
                                    view! { <option value=y.to_string() selected=y == sel>{y.to_string()}</option> }
                                }).collect_view()
                            }}
                        </select>
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
