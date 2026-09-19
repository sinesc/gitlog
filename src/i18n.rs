//! UI language: detection from the system locale and localized date
//! formatting. All other user-facing strings live in the locale JSON files
//! (`locales/{en,de,fr,es,it,nl,pt}.json`) and are looked up with the
//! `rust_i18n::t!` macro (initialized in main).
//!
//! Adding a language: one JSON file under `locales/`, one entry in
//! `locale_from_tag`, one `DateNames` table below, and a test.

use std::env;

/// Weekday and month abbreviations used by `format_date`.
/// `weekdays` is indexed by glib's day_of_week() (0 = Sunday .. 6 = Saturday),
/// `months` by month() - 1 (January first).
#[derive(Debug)]
struct DateNames {
    weekdays: [&'static str; 7],
    months: [&'static str; 12],
}

const EN: DateNames = DateNames {
    weekdays: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    months: [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
};
const DE: DateNames = DateNames {
    weekdays: ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"],
    months: [
        "Jan", "Feb", "Mär", "Apr", "Mai", "Jun",
        "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
    ],
};
const FR: DateNames = DateNames {
    weekdays: ["dim", "lun", "mar", "mer", "jeu", "ven", "sam"],
    months: [
        "janv.", "févr.", "mars", "avr.", "mai", "juin",
        "juil.", "août", "sept.", "oct.", "nov.", "déc.",
    ],
};
const ES: DateNames = DateNames {
    weekdays: ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"],
    months: [
        "ene", "feb", "mar", "abr", "may", "jun",
        "jul", "ago", "sep", "oct", "nov", "dic",
    ],
};
const IT: DateNames = DateNames {
    weekdays: ["dom", "lun", "mar", "mer", "gio", "ven", "sab"],
    months: [
        "gen", "feb", "mar", "apr", "mag", "giu",
        "lug", "ago", "set", "ott", "nov", "dic",
    ],
};
const NL: DateNames = DateNames {
    weekdays: ["zo", "ma", "di", "wo", "do", "vr", "za"],
    months: [
        "jan", "feb", "mrt", "apr", "mei", "jun",
        "jul", "aug", "sep", "okt", "nov", "dec",
    ],
};
const PT: DateNames = DateNames {
    weekdays: ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"],
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
/// to a supported UI language.
fn locale_from_tag(value: &str) -> Option<&'static str> {
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
    let weekday = names.weekdays[dt.day_of_week() as usize];
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_tag_parsing() {
        assert_eq!(locale_from_tag("de_DE.UTF-8"), Some("de")); // glibc style
        assert_eq!(locale_from_tag("de-DE.UTF-8"), Some("de")); // RFC 1766 style
        assert_eq!(locale_from_tag("de-AT"), Some("de"));
        assert_eq!(locale_from_tag("DE_de"), Some("de"));
        assert_eq!(locale_from_tag("fr_FR.UTF-8"), Some("fr"));
        assert_eq!(locale_from_tag("fr"), Some("fr"));
        assert_eq!(locale_from_tag("es_ES"), Some("es"));
        assert_eq!(locale_from_tag("it-IT.UTF-8"), Some("it"));
        assert_eq!(locale_from_tag("nl_NL.UTF-8"), Some("nl"));
        assert_eq!(locale_from_tag("pt_PT"), Some("pt"));
        assert_eq!(locale_from_tag("en_US.UTF-8"), None);
        assert_eq!(locale_from_tag("pl_PL.UTF-8"), None);
        assert_eq!(locale_from_tag(""), None);
    }

    #[test]
    fn date_monday_6_jan_2025_all_locales() {
        // 6 Jan 2025 is a Monday
        let dt = glib::DateTime::from_local(2025, 1, 6, 14, 30, 0.0).unwrap();
        assert_eq!(format_date(&dt, "en"), "Mon, 06 Jan 2025, 14:30");
        assert_eq!(format_date(&dt, "de"), "Mo, 06 Jan 2025, 14:30");
        assert_eq!(format_date(&dt, "fr"), "lun, 06 janv. 2025, 14:30");
        assert_eq!(format_date(&dt, "es"), "lun, 06 ene 2025, 14:30");
        assert_eq!(format_date(&dt, "it"), "lun, 06 gen 2025, 14:30");
        assert_eq!(format_date(&dt, "nl"), "ma, 06 jan 2025, 14:30");
        assert_eq!(format_date(&dt, "pt"), "seg, 06 jan 2025, 14:30");
        // unknown locale falls back to English
        assert_eq!(format_date(&dt, "pl"), "Mon, 06 Jan 2025, 14:30");
    }

    #[test]
    fn date_saturday_15_mar_2025() {
        let dt = glib::DateTime::from_local(2025, 3, 15, 8, 5, 0.0).unwrap();
        assert_eq!(format_date(&dt, "de"), "Sa, 15 Mär 2025, 08:05");
        assert_eq!(format_date(&dt, "fr"), "sam, 15 mars 2025, 08:05");
        assert_eq!(format_date(&dt, "pt"), "sáb, 15 mar 2025, 08:05");
    }

    #[test]
    fn timestamp_formats() {
        // 6 Jan 2025, 10:30 UTC; the local wall clock depends on the
        // machine's TZ, so only check the shape, not the exact string
        let s = format_timestamp(1736140200, "en");
        assert!(s.contains("Jan"));
        assert!(s.contains("2025"));
    }
}
