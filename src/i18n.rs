//! UI language: detection from the system locale and localized date
//! formatting. All other user-facing strings live in `locales/{en,de}.json`
//! and are looked up with the `rust_i18n::t!` macro (initialized in main).

use std::env;

// glib's day_of_week(): 0 = Sunday .. 6 = Saturday; month(): 1 = January
const WEEKDAYS_EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun",
    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS_DE: [&str; 7] = ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"];
const MONTHS_DE: [&str; 12] = [
    "Jan", "Feb", "Mär", "Apr", "Mai", "Jun",
    "Jul", "Aug", "Sep", "Okt", "Nov", "Dez",
];

/// Map a raw locale value (e.g. "de_DE.UTF-8") to a supported UI language.
fn parse_locale(value: &str) -> Option<&'static str> {
    let primary = value.split(['.', '-']).next()?;
    if primary.to_lowercase().starts_with("de") {
        Some("de")
    } else {
        None
    }
}

/// The first set variable of LC_ALL / LC_MESSAGES / LANG decides;
/// German locales ("de*") select "de", everything else falls back to "en".
pub fn detect_locale() -> &'static str {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = env::var(var) {
            return parse_locale(&value).unwrap_or("en");
        }
    }
    "en"
}

/// "ddd, dd Mon yyyy, HH:MM" with the weekday and month names in `locale`.
pub fn format_date(dt: &glib::DateTime, locale: &str) -> String {
    let (weekdays, months) = match locale {
        "de" => (WEEKDAYS_DE, MONTHS_DE),
        _ => (WEEKDAYS_EN, MONTHS_EN),
    };
    let weekday = weekdays[dt.day_of_week() as usize];
    let month = months[dt.month() as usize - 1];
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
    fn locale_parsing() {
        assert_eq!(parse_locale("de_DE.UTF-8"), Some("de"));
        assert_eq!(parse_locale("de-AT"), Some("de"));
        assert_eq!(parse_locale("DE_de"), Some("de"));
        assert_eq!(parse_locale("en_US.UTF-8"), None);
        assert_eq!(parse_locale("fr"), None);
        assert_eq!(parse_locale(""), None);
    }

    #[test]
    fn date_english() {
        // 6 Jan 2025 is a Monday
        let dt = glib::DateTime::from_local(2025, 1, 6, 14, 30, 0.0).unwrap();
        assert_eq!(format_date(&dt, "en"), "Mon, 06 Jan 2025, 14:30");
    }

    #[test]
    fn date_german() {
        // 6 Jan 2025 is a Monday, 15 Mar 2025 is a Saturday
        let dt = glib::DateTime::from_local(2025, 1, 6, 14, 30, 0.0).unwrap();
        assert_eq!(format_date(&dt, "de"), "Mo, 06 Jan 2025, 14:30");
        let dt = glib::DateTime::from_local(2025, 3, 15, 8, 5, 0.0).unwrap();
        assert_eq!(format_date(&dt, "de"), "Sa, 15 Mär 2025, 08:05");
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
