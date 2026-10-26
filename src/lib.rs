//! Shared crate code for the `gitlog` and `gitcommit` binaries:
//! git data access (`gitlog`) and UI language handling (`i18n`).

pub mod gitlog;
pub mod i18n;

// Locale files for the `t!` macro used by the shared code below; the
// binaries initialize the same set for their own strings.
rust_i18n::i18n!("locales", fallback = "en");
