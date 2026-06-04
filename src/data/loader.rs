use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use serde::Deserialize;

use crate::data::types::{Interval, IqaDominance, MapStat, Reading, Station};

// ── Fetch helpers ──────────────────────────────────────────────────────────

/// GET a same-origin JSON file and deserialize it. Mirrors BikeStat's
/// defensive shape: a non-2xx status is an explicit error rather than a panic.
async fn fetch_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, String> {
    let resp = gloo_net::http::Request::get(url)
        .send()
        .await
        .map_err(|e| format!("Network error: {e:?}"))?;
    if !resp.ok() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<T>().await.map_err(|e| format!("Parse error: {e:?}"))
}

pub async fn fetch_stations() -> Result<Vec<Station>, String> {
    fetch_json("data/stations.json").await
}

/// One year's slice of the map stats: `{ station -> { substance -> MapStat } }`.
pub type YearStats = BTreeMap<String, BTreeMap<String, MapStat>>;
/// `map-stats.json` → `{ year|"all" -> YearStats }`.
pub type MapStats = BTreeMap<String, YearStats>;

pub async fn fetch_map_stats() -> Result<MapStats, String> {
    fetch_json("data/map-stats.json").await
}

/// `iqa-dominance.json` → `{ year -> { station -> IqaDominance } }`. Optional
/// file; a fetch failure yields an empty map so the rest of the app is fine.
pub type IqaDominanceMap = BTreeMap<String, BTreeMap<String, IqaDominance>>;

pub async fn fetch_iqa_dominance() -> IqaDominanceMap {
    fetch_json("data/iqa-dominance.json").await.unwrap_or_default()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meta {
    pub years: Vec<i32>,
    pub min_year: i32,
    pub max_year: i32,
    #[allow(dead_code)]
    pub latest_year: i32,
    pub generated: String,
    #[allow(dead_code)]
    pub daily_start: String,
    #[allow(dead_code)]
    pub rows: u64,
    #[allow(dead_code)]
    pub stations: u32,
    pub substances: Vec<String>,
    pub source_url: String,
    #[allow(dead_code)]
    pub station_list_url: String,
}

pub async fn fetch_meta() -> Result<Meta, String> {
    fetch_json("data/meta.json").await
}

/// On-disk shape of `series/station-<id>-<year>.json` — hourly values for one
/// station-year, sparse `[hour_index, value]` pairs anchored at `start_utc`.
/// Loaded on demand only for the Hour interval.
#[derive(Debug, Clone, Deserialize)]
pub struct SeriesFile {
    #[allow(dead_code)]
    pub id: u32,
    #[allow(dead_code)]
    pub year: i32,
    pub start_utc: String,
    pub step_secs: i64,
    pub substances: BTreeMap<String, Vec<(i64, f64)>>,
}

pub async fn fetch_series(station_id: u32, year: i32) -> Result<SeriesFile, String> {
    fetch_json(&format!("data/series/station-{station_id}-{year}.json")).await
}

impl SeriesFile {
    /// Expand the sparse hourly pairs for `substance` into timestamped readings.
    pub fn readings(&self, substance: &str) -> Vec<Reading> {
        let base = DateTime::parse_from_rfc3339(&self.start_utc)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
        let step = self.step_secs;
        match self.substances.get(substance) {
            None => Vec::new(),
            Some(pairs) => pairs
                .iter()
                .map(|(idx, v)| Reading { timestamp: base + Duration::seconds(idx * step), value: *v })
                .collect(),
        }
    }
}

/// On-disk shape of `series-daily/station-<id>.json` — one daily mean per
/// substance spanning all years, sparse `[day_index, value]` from `start_date`.
/// Drives Day/Week/Month/Year intervals over long ranges without hourly loads.
#[derive(Debug, Clone, Deserialize)]
pub struct DailySeries {
    #[allow(dead_code)]
    pub id: u32,
    pub start_date: String,
    pub substances: BTreeMap<String, Vec<(i64, f64)>>,
}

pub async fn fetch_daily_series(station_id: u32) -> Result<DailySeries, String> {
    fetch_json(&format!("data/series-daily/station-{station_id}.json")).await
}

impl DailySeries {
    pub fn readings(&self, substance: &str) -> Vec<Reading> {
        let base = chrono::NaiveDate::parse_from_str(&self.start_date, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|ndt| Utc.from_utc_datetime(&ndt))
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
        match self.substances.get(substance) {
            None => Vec::new(),
            Some(pairs) => pairs
                .iter()
                .map(|(idx, v)| Reading { timestamp: base + Duration::days(*idx), value: *v })
                .collect(),
        }
    }
}

// ── Aggregation (bucket MEAN — concentrations, not counts) ──────────────────

/// Bucket `readings` by `interval` and return the **mean** value per bucket.
///
/// Leading/trailing partial Week/Month buckets are trimmed (same rule as
/// BikeStat) so the chart never shows a half-formed bucket that reads as a dip.
pub fn aggregate(readings: &[Reading], interval: Interval) -> Vec<(DateTime<Utc>, f64)> {
    let mut sums: BTreeMap<i64, (f64, u32)> = BTreeMap::new();
    let mut min_ts: Option<DateTime<Utc>> = None;
    let mut max_ts: Option<DateTime<Utc>> = None;
    for r in readings {
        min_ts = Some(min_ts.map_or(r.timestamp, |m| m.min(r.timestamp)));
        max_ts = Some(max_ts.map_or(r.timestamp, |m| m.max(r.timestamp)));
        let key = bucket_key(r.timestamp, interval);
        let e = sums.entry(key).or_insert((0.0, 0));
        e.0 += r.value;
        e.1 += 1;
    }
    let mut out: Vec<(DateTime<Utc>, f64)> = sums
        .into_iter()
        .filter_map(|(k, (sum, n))| {
            if n == 0 {
                None
            } else {
                DateTime::from_timestamp(k, 0).map(|dt| (dt, sum / n as f64))
            }
        })
        .collect();

    if matches!(interval, Interval::Week | Interval::Month | Interval::Year) {
        if let (Some(min), Some(max)) = (min_ts, max_ts) {
            if let Some((b_start, _)) = out.first().copied() {
                if min.date_naive() != b_start.date_naive() {
                    out.remove(0);
                }
            }
            if let Some((b_start, _)) = out.last().copied() {
                if max.date_naive() != bucket_last_day(b_start, interval) {
                    out.pop();
                }
            }
        }
    }

    out
}

fn bucket_last_day(b_start: DateTime<Utc>, interval: Interval) -> chrono::NaiveDate {
    match interval {
        Interval::Week => b_start.date_naive() + Duration::days(6),
        Interval::Month => {
            let nd = b_start.date_naive();
            let (y, m) = if nd.month() == 12 {
                (nd.year() + 1, 1)
            } else {
                (nd.year(), nd.month() + 1)
            };
            chrono::NaiveDate::from_ymd_opt(y, m, 1).unwrap() - Duration::days(1)
        }
        Interval::Year => chrono::NaiveDate::from_ymd_opt(b_start.year(), 12, 31).unwrap(),
        Interval::Hour | Interval::Day => b_start.date_naive(),
    }
}

fn bucket_key(ts: DateTime<Utc>, interval: Interval) -> i64 {
    match interval {
        Interval::Hour => ts
            .with_minute(0)
            .unwrap()
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap()
            .timestamp(),
        Interval::Day => ts.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
        Interval::Week => {
            let dow = ts.weekday().num_days_from_monday();
            (ts.date_naive() - Duration::days(dow as i64))
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp()
        }
        Interval::Month => ts
            .date_naive()
            .with_day(1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp(),
        Interval::Year => chrono::NaiveDate::from_ymd_opt(ts.year(), 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp(),
    }
}
