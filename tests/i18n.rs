//! Integration tests for the locale detection and the localized date
//! formatting in `gitlog::i18n`.

use gitlog::i18n::*;

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
fn date_sunday_5_jan_2025() {
    // 5 Jan 2025 is a Sunday; day_of_week() == 7 used to index out of bounds
    let dt = glib::DateTime::from_local(2025, 1, 5, 9, 0, 0.0).unwrap();
    assert_eq!(format_date(&dt, "en"), "Sun, 05 Jan 2025, 09:00");
    assert_eq!(format_date(&dt, "de"), "So, 05 Jan 2025, 09:00");
    assert_eq!(format_date(&dt, "fr"), "dim, 05 janv. 2025, 09:00");
    assert_eq!(format_date(&dt, "es"), "dom, 05 ene 2025, 09:00");
    assert_eq!(format_date(&dt, "it"), "dom, 05 gen 2025, 09:00");
    assert_eq!(format_date(&dt, "nl"), "zo, 05 jan 2025, 09:00");
    assert_eq!(format_date(&dt, "pt"), "dom, 05 jan 2025, 09:00");
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
