//! In-app i18n for English / French.
//!
//! Every user-facing string lives as a `&'static str` field on `T`, with one
//! `const` instance per language (`EN`, `FR`). Lookup is `lang.t().field` —
//! typos and missing fields are compile errors. Pollutant display names come
//! from `data::pollutants`, not from here.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    En,
    Fr,
}

impl Lang {
    const STORAGE_KEY: &'static str = "airquality-lang";

    /// Initial language: stored preference if set, else navigator.language
    /// (anything starting with "fr" → French), else English.
    pub fn from_browser() -> Self {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                if let Ok(Some(s)) = storage.get_item(Self::STORAGE_KEY) {
                    if s == "fr" {
                        return Self::Fr;
                    }
                    if s == "en" {
                        return Self::En;
                    }
                }
            }
            let nav_lang = window.navigator().language().unwrap_or_default();
            if nav_lang.to_lowercase().starts_with("fr") {
                return Self::Fr;
            }
        }
        Self::En
    }

    pub fn store(self) {
        if let Some(window) = web_sys::window() {
            if let Ok(Some(storage)) = window.local_storage() {
                let _ = storage.set_item(Self::STORAGE_KEY, self.code());
            }
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::En => Self::Fr,
            Self::Fr => Self::En,
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::En => "EN",
            Self::Fr => "FR",
        }
    }

    pub fn t(self) -> &'static T {
        match self {
            Self::En => &EN,
            Self::Fr => &FR,
        }
    }
}

pub struct T {
    // Header / chrome
    pub subtitle: &'static str,
    pub view_map: &'static str,
    pub view_series: &'static str,
    pub view_ufp: &'static str,
    pub view_network: &'static str,
    pub view_methods: &'static str,
    pub view_limits: &'static str,
    pub view_links: &'static str,
    pub mobile_filters: &'static str,
    pub mobile_close: &'static str,
    pub data_prefix: &'static str,
    pub generated: &'static str,
    pub latest_year_label: &'static str,
    /// Discrete footer disclaimer on the Map and Time-series views.
    pub disclaimer: &'static str,
    /// Extra footer clause on the Map view: the between-station surface is interpolated.
    pub interp_note: &'static str,

    // Section labels
    pub substance: &'static str,
    pub statistic: &'static str,
    pub station: &'static str,
    pub interval: &'static str,
    pub date_range: &'static str,
    pub custom_range: &'static str,
    pub profile: &'static str,
    pub prof_weekday: &'static str,
    pub prof_weekend: &'static str,
    pub prof_weekly: &'static str,
    pub time_of_day: &'static str,
    pub all_hours: &'static str,
    pub day_type: &'static str,
    pub days_all: &'static str,
    /// Weekday short names, Monday-first, for the weekly profile's axis.
    pub dow: [&'static str; 7],
    pub year: &'static str,
    pub all_years: &'static str,

    // Stats
    pub stat_mean: &'static str,
    pub stat_median: &'static str,
    pub stat_max: &'static str,
    pub stat_min: &'static str,
    pub stat_mean_daily_max: &'static str,

    // Intervals
    pub hour: &'static str,
    pub day: &'static str,
    pub week: &'static str,
    pub month: &'static str,

    // Date presets
    pub last_year: &'static str,
    pub last_5_years: &'static str,
    pub last_10_years: &'static str,
    pub last_3_months: &'static str,
    pub range_too_short: &'static str,

    // Map
    pub loading_stations: &'static str,
    pub click_marker: &'static str,
    /// Shown when a time-of-day filter is active: the map reads the hourly tier
    /// and bounds very long ranges to the most recent years.
    pub map_hour_note: &'static str,
    pub no_data_substance: &'static str,
    pub stations_measuring: &'static str,
    pub map_avg: &'static str,
    pub station_names: &'static str,
    pub show: &'static str,
    pub hide: &'static str,
    pub iqa_main_driver: &'static str,
    pub iqa_peak_driver: &'static str,
    pub iqa_good: &'static str,
    pub iqa_acceptable: &'static str,
    pub iqa_poor: &'static str,
    pub iqa_higher_worse: &'static str,

    // UFP surface view
    pub ufp_title: &'static str,
    pub ufp_loading: &'static str,
    pub ufp_hint: &'static str,
    /// Colour-bar title, e.g. "UFP (particles/cm³)".
    pub ufp_legend_title: &'static str,
    /// Note under the colour bar explaining the reference grid spacing.
    pub ufp_grid_note: &'static str,
    /// Attribution sentence fragments around the linked paper title.
    pub ufp_source_label: &'static str,
    pub ufp_data_credit: &'static str,
    pub ufp_modelled_note: &'static str,

    // Placeholders
    pub select_station_desktop: &'static str,
    pub select_station_mobile: &'static str,

    // Chart legend / export
    pub leg_mean: &'static str,
    pub leg_min: &'static str,
    pub leg_max: &'static str,
    pub download_chart_png: &'static str,
    pub copy_chart_to_clipboard: &'static str,
    pub copied_to_clipboard: &'static str,
}

pub const EN: T = T {
    subtitle: "Historical air-quality visualizer",
    view_map: "Map",
    view_series: "Time series",
    view_ufp: "UFP Model",
    view_network: "Stations",
    view_methods: "Methodology",
    view_limits: "Limits",
    view_links: "Further reading",
    mobile_filters: "Filters",
    mobile_close: "Close",
    data_prefix: "Historical RSQA data",
    generated: "Generated",
    latest_year_label: "Latest available year",
    disclaimer: "Independent project · not affiliated with the Ville de Montréal · historical data through 2024, not real-time",
    interp_note: "the interpolated surface between stations is illustrative, not exact",

    substance: "Substance",
    statistic: "Statistic",
    station: "Station",
    interval: "Aggregation",
    date_range: "Date range",
    custom_range: "Custom range",
    profile: "Averaging profile",
    prof_weekday: "Weekday",
    prof_weekend: "Weekend",
    prof_weekly: "Weekly",
    time_of_day: "Time of day",
    all_hours: "All hours",
    day_type: "Day type",
    days_all: "All days",
    dow: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    year: "Year",
    all_years: "All years",

    stat_mean: "Mean",
    stat_median: "Median",
    stat_max: "Maximum",
    stat_min: "Minimum",
    stat_mean_daily_max: "Mean daily max",

    hour: "Hour",
    day: "Day",
    week: "Week",
    month: "Month",

    last_year: "Last Year",
    last_5_years: "Last 5 Years",
    last_10_years: "Last 10 Years",
    last_3_months: "Last 3 Months",
    range_too_short: "Range too short for interval",

    loading_stations: "Loading stations…",
    click_marker: "Hover over a station for more info",
    map_hour_note: "Time-of-day filter uses hourly data (most recent years for long ranges)",
    no_data_substance: "not measured at this station",
    stations_measuring: "stations measuring",
    map_avg: "Station average",
    station_names: "Station names",
    show: "Show",
    hide: "Hide",
    iqa_main_driver: "Main driver",
    iqa_peak_driver: "Peak driver",
    iqa_good: "Good",
    iqa_acceptable: "Acceptable",
    iqa_poor: "Poor",
    iqa_higher_worse: "Higher = worse air quality",

    ufp_title: "Modelled ultrafine particles (UFP) — Montréal 2020 · height = concentration",
    ufp_loading: "Loading surface…",
    ufp_hint: "Drag to rotate · scroll or pinch to zoom · double-click to reset",
    ufp_legend_title: "UFP (particles/cm³)",
    ufp_grid_note: "Ground grid: 5 km",
    ufp_source_label: "Source:",
    ufp_data_credit: "(Environment International, 2023). Raw model data kindly provided by Scott Weichenthal, the study's corresponding author.",
    ufp_modelled_note: "The values shown are modelled estimates derived from an experimental measurement technique — not direct measurements at this resolution.",

    select_station_desktop: "Select a station and substance from the sidebar to view its time series",
    select_station_mobile: "Select a station and substance from the Filters menu to view its time series",

    leg_mean: "mean",
    leg_min: "min",
    leg_max: "max",
    download_chart_png: "Download chart as PNG",
    copy_chart_to_clipboard: "Copy chart to clipboard",
    copied_to_clipboard: "Copied!",
};

pub const FR: T = T {
    subtitle: "Visualiseur historique de la qualité de l'air",
    view_map: "Carte",
    view_series: "Série temporelle",
    view_ufp: "Modèle PUF",
    view_network: "Stations",
    view_methods: "Méthodologie",
    view_limits: "Limites",
    view_links: "Pour aller plus loin",
    mobile_filters: "Filtres",
    mobile_close: "Fermer",
    data_prefix: "Données RSQA historiques",
    generated: "Généré",
    latest_year_label: "Dernière année disponible",
    disclaimer: "Projet indépendant · non affilié à la Ville de Montréal · données historiques jusqu'en 2024, non en temps réel",
    interp_note: "la surface interpolée entre les stations est illustrative, non exacte",

    substance: "Substance",
    statistic: "Statistique",
    station: "Station",
    interval: "Agrégation",
    date_range: "Plage de dates",
    custom_range: "Plage personnalisée",
    profile: "Profil moyen",
    prof_weekday: "Semaine",
    prof_weekend: "Fin de semaine",
    prof_weekly: "Hebdomadaire",
    time_of_day: "Heure de la journée",
    all_hours: "Toutes les heures",
    day_type: "Type de jour",
    days_all: "Tous les jours",
    dow: ["lun", "mar", "mer", "jeu", "ven", "sam", "dim"],
    year: "Année",
    all_years: "Toutes les années",

    stat_mean: "Moyenne",
    stat_median: "Médiane",
    stat_max: "Maximum",
    stat_min: "Minimum",
    stat_mean_daily_max: "Moy. max. quotidiens",

    hour: "Heure",
    day: "Jour",
    week: "Semaine",
    month: "Mois",

    last_year: "Année dernière",
    last_5_years: "5 dernières années",
    last_10_years: "10 dernières années",
    last_3_months: "3 derniers mois",
    range_too_short: "Plage trop courte pour l'agrégation",

    loading_stations: "Chargement des stations…",
    click_marker: "Survolez une station pour plus d'infos",
    map_hour_note: "Le filtre horaire utilise les données horaires (années récentes pour les longues plages)",
    no_data_substance: "non mesurée à cette station",
    stations_measuring: "stations mesurant",
    map_avg: "Moyenne des stations",
    station_names: "Noms des stations",
    show: "Afficher",
    hide: "Masquer",
    iqa_main_driver: "Polluant dominant",
    iqa_peak_driver: "Polluant au pic",
    iqa_good: "Bon",
    iqa_acceptable: "Acceptable",
    iqa_poor: "Mauvais",
    iqa_higher_worse: "Valeur élevée = air plus pollué",

    ufp_title: "Particules ultrafines (PUF) modélisées — Montréal 2020 · hauteur = concentration",
    ufp_loading: "Chargement de la surface…",
    ufp_hint: "Glissez pour pivoter · molette ou pincement pour zoomer · double-clic pour réinitialiser",
    ufp_legend_title: "PUF (particules/cm³)",
    ufp_grid_note: "Grille au sol : 5 km",
    ufp_source_label: "Source :",
    ufp_data_credit: "(Environment International, 2023). Données brutes du modèle aimablement fournies par Scott Weichenthal, auteur correspondant de l'étude.",
    ufp_modelled_note: "Les valeurs affichées sont des estimations modélisées, dérivées d'une technique de mesure expérimentale — et non des mesures directes à cette résolution.",

    select_station_desktop: "Sélectionnez une station et une substance dans la barre latérale pour afficher la série temporelle",
    select_station_mobile: "Sélectionnez une station et une substance dans le menu Filtres pour afficher la série temporelle",

    leg_mean: "moy",
    leg_min: "min",
    leg_max: "max",
    download_chart_png: "Télécharger le graphique en PNG",
    copy_chart_to_clipboard: "Copier le graphique dans le presse-papiers",
    copied_to_clipboard: "Copié !",
};
