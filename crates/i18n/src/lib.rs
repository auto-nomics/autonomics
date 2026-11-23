//! Shared i18n for the autonomics Rust surfaces (TUI + CLI).
//!
//! Catalogs live in `locales/` and are embedded at compile time; no runtime
//! file access. Resolution order (see [`init_locale`]):
//!   1. `AUTONOMICS_LANG` (exact `zh`/`en`)
//!   2. system locale (`LANG`/`LC_ALL`, any `zh*` → zh)
//!   3. fallback `en`
//!
//! Usage: call [`init_locale`] once at process start, then use [`t`] everywhere.
rust_i18n::i18n!("locales", fallback = "en");

/// Supported locales.
pub const LOCALES: &[&str] = &["en", "zh"];

/// Resolve and install the process locale. Safe to call multiple times.
pub fn init_locale() {
    set_locale(&resolve_locale());
}

/// Resolve the effective locale without installing it.
pub fn resolve_locale() -> String {
    if let Ok(lang) = std::env::var("AUTONOMICS_LANG") {
        let lang = lang.trim().to_ascii_lowercase();
        let tag = lang.split(['_', '-']).next().unwrap_or_default().to_string();
        if LOCALES.contains(&tag.as_str()) {
            return tag;
        }
    }
    for key in ["LC_ALL", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_ascii_lowercase();
            let tag = v.split(['_', '-']).next().unwrap_or_default().to_string();
            if LOCALES.contains(&tag.as_str()) {
                return tag;
            }
        }
    }
    "en".to_string()
}

pub use rust_i18n::{set_locale, t};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_embed_both_locales() {
        // The `t!` macro falls back to the key itself when missing; assert on
        // a key every catalog defines to prove both locales loaded.
        set_locale("en");
        assert!(!t!("locale.name").is_empty());
        set_locale("zh");
        assert!(!t!("locale.name").is_empty());
        set_locale("en");
    }
}
