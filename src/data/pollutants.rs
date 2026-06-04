//! Static catalogue of RSQA pollutants: display names (EN/FR) and units.
//!
//! Keyed by the exact column key used in the source CSV / preprocessed JSON
//! (e.g. `"NO2"`, `"PM2.5"`). The UI only *offers* substances that appear in
//! the loaded data, but the catalogue carries metadata for every column the
//! dataset can contain so labels and units never fall back to a bare key.

use crate::i18n::Lang;

pub struct Pollutant {
    pub key: &'static str,
    pub name_en: &'static str,
    pub name_fr: &'static str,
    pub unit: &'static str,
}

impl Pollutant {
    pub fn name(&self, lang: Lang) -> &'static str {
        match lang {
            Lang::En => self.name_en,
            Lang::Fr => self.name_fr,
        }
    }
}

/// Full catalogue, in the dataset's column order. PST and PM10 are listed for
/// completeness but were not measured in 2024, so they never reach the UI.
///
/// `IQA` is the official Air Quality Index — a unitless composite the
/// preprocessor derives from the City's per-pollutant sub-indices (max per
/// station-hour). It has no measurement unit, so `unit` is empty.
pub static POLLUTANTS: &[Pollutant] = &[
    Pollutant { key: "IQA",          name_en: "Air Quality Index (IQA)",  name_fr: "Indice de la qualité de l'air (IQA)", unit: "" },
    Pollutant { key: "CO",           name_en: "Carbon monoxide",          name_fr: "Monoxyde de carbone",       unit: "ppb" },
    Pollutant { key: "H2S",          name_en: "Hydrogen sulfide",         name_fr: "Sulfure d'hydrogène",       unit: "ppb" },
    Pollutant { key: "NO",           name_en: "Nitric oxide",             name_fr: "Monoxyde d'azote",          unit: "ppb" },
    Pollutant { key: "NO2",          name_en: "Nitrogen dioxide",         name_fr: "Dioxyde d'azote",           unit: "ppb" },
    Pollutant { key: "PM2.5",        name_en: "Fine particles (PM2.5)",   name_fr: "Particules fines (PM2,5)",  unit: "µg/m³" },
    Pollutant { key: "PST",          name_en: "Total suspended particles", name_fr: "Particules en suspension",  unit: "µg/m³" },
    Pollutant { key: "PM10",         name_en: "Respirable particles (PM10)", name_fr: "Particules (PM10)",      unit: "µg/m³" },
    Pollutant { key: "O3",           name_en: "Ozone",                    name_fr: "Ozone",                     unit: "ppb" },
    Pollutant { key: "SO2",          name_en: "Sulfur dioxide",           name_fr: "Dioxyde de soufre",         unit: "ppb" },
    Pollutant { key: "BC1_370nm",    name_en: "Black carbon (370 nm)",    name_fr: "Carbone suie (370 nm)",     unit: "µg/m³" },
    Pollutant { key: "BC6_880nm",    name_en: "Black carbon (880 nm)",    name_fr: "Carbone suie (880 nm)",     unit: "µg/m³" },
    Pollutant { key: "PUF",          name_en: "Ultrafine particles",      name_fr: "Particules ultrafines",     unit: "part/cm³" },
    Pollutant { key: "Benzene",      name_en: "Benzene",                  name_fr: "Benzène",                   unit: "µg/m³" },
    Pollutant { key: "Toluene",      name_en: "Toluene",                  name_fr: "Toluène",                   unit: "µg/m³" },
    Pollutant { key: "Ethylbenzene", name_en: "Ethylbenzene",             name_fr: "Éthylbenzène",              unit: "µg/m³" },
    Pollutant { key: "MP-Xylene",    name_en: "m,p-Xylene",               name_fr: "m,p-Xylène",                unit: "µg/m³" },
    Pollutant { key: "O-Xylene",     name_en: "o-Xylene",                 name_fr: "o-Xylène",                  unit: "µg/m³" },
];

/// Look up a pollutant by its dataset key.
pub fn pollutant(key: &str) -> Option<&'static Pollutant> {
    POLLUTANTS.iter().find(|p| p.key == key)
}

/// Display name for a key, falling back to the raw key if uncatalogued.
pub fn name_of(key: &str, lang: Lang) -> String {
    pollutant(key).map(|p| p.name(lang).to_string()).unwrap_or_else(|| key.to_string())
}

/// Unit for a key, or empty string if uncatalogued or unitless (e.g. IQA).
pub fn unit_of(key: &str) -> &'static str {
    pollutant(key).map(|p| p.unit).unwrap_or("")
}

/// Display label "Name (unit)", dropping the parenthetical for unitless keys.
pub fn display_label(key: &str, lang: Lang) -> String {
    let name = name_of(key, lang);
    let unit = unit_of(key);
    if unit.is_empty() {
        name
    } else {
        format!("{name} ({unit})")
    }
}
