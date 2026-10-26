//! Integration tests for the date field validation of the rebase window
//! (`gitlog::gitlog::is_date_string`).

use gitlog::gitlog::is_date_string;

#[test]
fn date_structure() {
    assert!(is_date_string("2026-09-20 17:36:12"));
    assert!(!is_date_string(""));
    assert!(!is_date_string("2026-9-20 17:36:12"));
    assert!(!is_date_string("2026-09-20T17:36:12"));
    assert!(!is_date_string("2026-09-20 17:36"));
    assert!(!is_date_string("abcd-ef-gh ij:kl:mn"));
}
