use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use serde::Deserialize;

use crate::data::types::{Interval, IqaDominance, Reading, Station};

// ── Fetch helpers ──────────────────────────────────────────────────────────

/// GET a same-origin JSON file and deserialize it. Mirrors BikeStat's
/// defensive shape: a non-2xx status is an explicit error rather than a panic.
///
/// `cache: no-cache` makes the browser always revalidate via a conditional
/// request: unchanged files come back as a tiny `304` (no re-download), but a
/// file changed by a deploy returns fresh bytes. The data files have stable
/// names and no `Cache-Control`, so without this a heuristically-cached old
/// copy could be served to a newer build — e.g. the format-changed daily tier,
/// which the new code then couldn't parse. The server already sends ETags, so
/// the revalidation is cheap.
async fn fetch_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, String> {
    let resp = gloo_net::http::Request::get(url)
        .cache(web_sys::RequestCache::NoCache)
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

/// One local-calendar-year bin of the diurnal-profile tier:
/// `[year, wd_sum[24], wd_cnt[24], we_sum[24], we_cnt[24]]` — per-hour value
/// sums and sample counts (Montréal-local hour-of-day), split weekday/weekend.
pub type ProfileYear = (i32, Vec<f64>, Vec<u32>, Vec<f64>, Vec<u32>);

/// On-disk shape of `series-profiles/station-<id>.json` — the precomputed
/// diurnal-profile tier spanning all years. Lets the Weekday/Weekend averaging
/// profiles cover the whole record over long date ranges without downloading
/// dozens of hourly station-year files (short ranges keep the exact hourly path).
#[derive(Debug, Clone, Deserialize)]
pub struct ProfileSeries {
    #[allow(dead_code)]
    pub id: u32,
    /// Per substance, sparse year bins sorted by year.
    pub substances: BTreeMap<String, Vec<ProfileYear>>,
}

pub async fn fetch_profile_series(station_id: u32) -> Result<ProfileSeries, String> {
    fetch_json(&format!("data/series-profiles/station-{station_id}.json")).await
}

/// On-disk shape of `ufp-surface.json` — the modelled ultrafine-particle grid
/// extracted from the Lloyd et al. (2023) combined-model output by
/// `scripts/extract-ufp-surface.py`. Row-major `ny × nx` values (pt/cm³, `None`
/// = outside the modelled area) over a uniform km grid anchored at `(x0, y0)`
/// with steps `(dx, dy)`; x runs west→east, y south→north. `cmin`/`cmax` are
/// the original figure's colour clamp, `zmin`/`zmax` the true value extremes.
#[derive(Debug, Clone, Deserialize)]
pub struct UfpSurface {
    pub nx: usize,
    pub ny: usize,
    #[allow(dead_code)]
    pub x0: f64,
    pub dx: f64,
    #[allow(dead_code)]
    pub y0: f64,
    pub dy: f64,
    pub cmin: f64,
    pub cmax: f64,
    pub zmin: f64,
    pub zmax: f64,
    pub z: Vec<Option<f32>>,
}

pub async fn fetch_ufp_surface() -> Result<UfpSurface, String> {
    let s: UfpSurface = fetch_json("data/ufp-surface.json").await?;
    if s.z.len() != s.nx * s.ny || s.nx < 2 || s.ny < 2 {
        return Err("ufp-surface.json: inconsistent grid shape".into());
    }
    Ok(s)
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

/// One day's summary in a daily series: `[day_index, mean, min, max, n]` — the
/// day's hourly mean, true hourly extremes, and sample count, sparse from
/// `start_date`. The Time-series view uses only the mean; the Map's default
/// (full-day) path uses all five to aggregate Mean/Min/Max over any date range.
pub type DailyCell = (i64, f64, f64, f64, u32);

/// On-disk shape of `series-daily/station-<id>.json` — daily cells per substance
/// spanning all years. Drives Day/Week/Month/Year intervals (Series view) and the
/// Map's full-day date-range averaging without touching the hourly tier.
#[derive(Debug, Clone, Deserialize)]
pub struct DailySeries {
    #[allow(dead_code)]
    pub id: u32,
    pub start_date: String,
    pub substances: BTreeMap<String, Vec<DailyCell>>,
}

pub async fn fetch_daily_series(station_id: u32) -> Result<DailySeries, String> {
    fetch_json(&format!("data/series-daily/station-{station_id}.json")).await
}

impl DailySeries {
    /// `start_date` parsed once; the Map maps each cell's day index off this.
    pub fn base_date(&self) -> Option<chrono::NaiveDate> {
        chrono::NaiveDate::parse_from_str(&self.start_date, "%Y-%m-%d").ok()
    }

    /// Expand the daily means into timestamped readings (Series view). Drops the
    /// min/max/n; the chart plots the daily mean.
    pub fn readings(&self, substance: &str) -> Vec<Reading> {
        let base = self
            .base_date()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|ndt| Utc.from_utc_datetime(&ndt))
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
        match self.substances.get(substance) {
            None => Vec::new(),
            Some(cells) => cells
                .iter()
                .map(|(idx, mean, ..)| Reading { timestamp: base + Duration::days(*idx), value: *mean })
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
