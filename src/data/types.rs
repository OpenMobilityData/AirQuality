use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;

/// One air-quality monitoring station, as published in `stations.json`.
/// `substances` lists the pollutant keys actually measured here in the
/// loaded year (so the UI only offers substances a station can answer for).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Station {
    pub id: u32,
    pub name: String,
    pub address: String,
    pub borough: String,
    pub lat: f64,
    pub lon: f64,
    /// Calendar years this station reported data (across the loaded archive).
    pub years: Vec<i32>,
    pub substances: Vec<String>,
}

/// A single hourly concentration reading (analogue of BikeStat's `CountRecord`).
#[derive(Debug, Clone)]
pub struct Reading {
    pub timestamp: DateTime<Utc>,
    pub value: f64,
}

/// Precomputed per-station/per-substance summary for the map view — one cell
/// of `map-stats.json`. Avoids parsing the hourly series just to draw the map.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct MapStat {
    pub mean: f64,
    pub median: f64,
    pub min: f64,
    pub max: f64,
    /// Number of valid hourly samples behind the summary (data-contract field;
    /// retained for tooltips/QA even though the map doesn't render it yet).
    #[allow(dead_code)]
    pub n: u32,
}

/// Per-station IQA dominant-pollutant summary (one entry of `iqa-dominance.json`).
/// `shares` lists each pollutant's fraction of hours it drove the index, sorted
/// descending; `peak_pollutant` drove the single worst hour (`peak_iqa`).
#[derive(Debug, Clone, Deserialize)]
pub struct IqaDominance {
    pub peak_pollutant: String,
    #[allow(dead_code)]
    pub peak_iqa: f64,
    pub shares: Vec<(String, f64)>,
}

/// Which summary statistic the map paints for the selected substance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stat {
    Mean,
    Median,
    Max,
    Min,
}

impl Stat {
    pub fn all() -> &'static [Stat] {
        &[Stat::Mean, Stat::Median, Stat::Max, Stat::Min]
    }
    pub fn label(self, lang: Lang) -> &'static str {
        let t = lang.t();
        match self {
            Stat::Mean => t.stat_mean,
            Stat::Median => t.stat_median,
            Stat::Max => t.stat_max,
            Stat::Min => t.stat_min,
        }
    }
    /// Pull this statistic out of a `MapStat`.
    pub fn value(self, s: &MapStat) -> f64 {
        match self {
            Stat::Mean => s.mean,
            Stat::Median => s.median,
            Stat::Max => s.max,
            Stat::Min => s.min,
        }
    }
}

/// Time-series bucket size (analogue of BikeStat's `Resolution`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interval {
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl Interval {
    pub fn all() -> &'static [Interval] {
        &[Interval::Hour, Interval::Day, Interval::Week, Interval::Month, Interval::Year]
    }
    pub fn label(self, lang: Lang) -> &'static str {
        let t = lang.t();
        match self {
            Interval::Hour => t.hour,
            Interval::Day => t.day,
            Interval::Week => t.week,
            Interval::Month => t.month,
            Interval::Year => t.year,
        }
    }
}

/// Averaging profile for the time-series view: fold the selected date range onto
/// a short repeating time base. Weekday/Weekend produce a 24-hour diurnal mean
/// (from hourly data); Weekly produces a 7-point day-of-week mean (from the daily
/// tier, so it spans the whole record).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Profile {
    Weekday,
    Weekend,
    Weekly,
}

impl Profile {
    pub fn all() -> &'static [Profile] {
        &[Profile::Weekday, Profile::Weekend, Profile::Weekly]
    }
    pub fn label(self, lang: Lang) -> &'static str {
        let t = lang.t();
        match self {
            Profile::Weekday => t.prof_weekday,
            Profile::Weekend => t.prof_weekend,
            Profile::Weekly => t.prof_weekly,
        }
    }
    /// True for the diurnal profiles, which need the hourly tier.
    pub fn needs_hourly(self) -> bool {
        matches!(self, Profile::Weekday | Profile::Weekend)
    }
}

/// Which top-level view is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Map,
    Series,
    /// Background on the RSQA monitoring network.
    Network,
    /// Data sources and processing methodology.
    Methods,
    /// Air-quality limits and guidelines reference.
    Limits,
}

impl View {
    /// True for the read-only article views, which hide the filter sidebar.
    pub fn is_info(self) -> bool {
        matches!(self, View::Network | View::Methods | View::Limits)
    }
}
