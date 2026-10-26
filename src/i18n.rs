//! UI language: detection from the system locale and localized date
//! formatting. All other user-facing strings live in the locale JSON files
//! (`locales/{en,de,fr,es,it,nl,pt}.json`) and are looked up with the
//! `rust_i18n::t!` macro (initialized in main).
//!
//! Adding a language: one JSON file under `locales/`, one entry in
//! `locale_from_tag`, one `DateNames` table below, and the tests in
//! `tests/i18n.rs` (dates) and `tests/translations.rs` (strings).

use std::env;

/// Weekday and month abbreviations used by `format_date`.
/// `weekdays` is indexed by glib's day_of_week() - 1 (1 = Monday .. 7 = Sunday),
/// `months` by month() - 1 (January first).
#[derive(Debug)]
struct DateNames {
    weekdays: [&'static str; 7],
    months: [&'static str; 12],
}

const EN: DateNames = DateNames {
    weekdays: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    months: [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
};
const DE: DateNames = DateNames {
    weekdays: ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"],
    months: [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun",
        "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ],
};
const FR: DateNames = DateNames {
    weekdays: ["lun", "mar", "mer", "jeu", "ven", "sam", "dim"],
    months: [
        "janv.", "févr.", "mars", "avr.", "mai", "juin",
        "juil.", "août", "sept.", "oct.", "nov.", "déc.",
    ],
};
const ES: DateNames = DateNames {
    weekdays: ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"],
    months: [
        "ene", "feb", "mar", "abr", "may", "jun",
        "jul", "ago", "sep", "oct", "nov", "dic",
    ],
};
const IT: DateNames = DateNames {
    weekdays: ["lun", "mar", "mer", "gio", "ven", "sab", "dom"],
    months: [
        "gen", "feb", "mar", "apr", "mag", "giu",
        "lug", "ago", "set", "ott", "nov", "dic",
    ],
};
const NL: DateNames = DateNames {
    weekdays: ["ma", "di", "wo", "do", "vr", "za", "zo"],
    months: [
        "jan", "feb", "mrt", "apr", "mei", "jun",
        "jul", "aug", "sep", "okt", "nov", "dec",
    ],
};
const PT: DateNames = DateNames {
    weekdays: ["seg", "ter", "qua", "qui", "sex", "sáb", "dom"],
    months: [
        "jan", "fev", "mar", "abr", "mai", "jun",
        "jul", "ago", "set", "out", "nov", "dez",
    ],
};

fn date_names(locale: &str) -> &'static DateNames {
    match locale {
        "de" => &DE,
        "fr" => &FR,
        "es" => &ES,
        "it" => &IT,
        "nl" => &NL,
        "pt" => &PT,
        _ => &EN,
    }
}

/// Map a raw locale value (e.g. "fr_FR.UTF-8" or "de_DE.UTF-8", glibc style)
/// to a supported UI language. Public for the tests in `tests/i18n.rs`.
pub fn locale_from_tag(value: &str) -> Option<&'static str> {
    let primary = value.split(['.', '-', '_']).next()?.to_lowercase();
    match primary.as_str() {
        "de" => Some("de"),
        "fr" => Some("fr"),
        "es" => Some("es"),
        "it" => Some("it"),
        "nl" => Some("nl"),
        "pt" => Some("pt"),
        _ => None,
    }
}

/// The first set variable of LC_ALL / LC_MESSAGES / LANG decides;
/// a language without locale files falls back to English.
pub fn detect_locale() -> &'static str {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = env::var(var) {
            return locale_from_tag(&value).unwrap_or("en");
        }
    }
    "en"
}

/// "ddd, dd Mon yyyy, HH:MM" with the weekday and month names in `locale`.
pub fn format_date(dt: &glib::DateTime, locale: &str) -> String {
    let names = date_names(locale);
    // day_of_week() is 1 = Monday .. 7 = Sunday; the table is Monday-first.
    let weekday = names.weekdays[dt.day_of_week() as usize - 1];
    let month = names.months[dt.month() as usize - 1];
    format!(
        "{weekday}, {:02} {month} {:04}, {:02}:{:02}",
        dt.day_of_month(),
        dt.year(),
        dt.hour(),
        dt.minute()
    )
}

/// Convenience wrapper for unix timestamps as stored in `gitlog::Commit`.
pub fn format_timestamp(ts: i64, locale: &str) -> String {
    match glib::DateTime::from_unix_local(ts) {
        Ok(dt) => format_date(&dt, locale),
        Err(_) => ts.to_string(),
    }
}
