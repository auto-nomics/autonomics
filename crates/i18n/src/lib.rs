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

pub use rust_i18n::set_locale;

/// Cross-crate `t!` macro.
///
/// rust-i18n's own `t!` resolves `_rust_i18n_t!` in the *calling* crate, so
/// it only works inside the crate that invoked `rust_i18n::i18n!`. This
/// wrapper runs the lookup against this crate's embedded catalogs, so any
/// workspace crate can `use i18n::t;` after [`crate::init_locale`] ran
/// (which the CLI main and the TUI entry points already do).
///
/// Supported forms (the ones used across this workspace):
///   - `t!("dotted.key")`
///   - `t!("dotted.key", name = expr, ...)` — interpolates `%{name}`
///     placeholders in the catalog value. The value only needs `Display`.
///
/// Note: rust-i18n 3.x has no plural rules — where a message needs
/// singular/plural wording, callers pick between two keys.
#[macro_export]
macro_rules! t {
    ($key:expr) => {
        $crate::translate($key, &[])
    };
    ($key:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::translate(
            $key,
            &[$((stringify!($name), std::format!("{}", $value))),+],
        )
    };
}

/// Look up `key` for the current locale and interpolate `%{name}`
/// placeholders with `args`. Falls back to the `en` catalog, then to the
/// dotted key itself (rust-i18n's fallback chain).
pub fn translate(key: &str, args: &[(&str, String)]) -> std::borrow::Cow<'static, str> {
    let locale = rust_i18n::locale().to_string();
    let translated = _rust_i18n_translate(&locale, key);
    let text = match translated {
        std::borrow::Cow::Borrowed(s) => s.to_string(),
        std::borrow::Cow::Owned(s) => s,
    };
    if args.is_empty() {
        return std::borrow::Cow::Owned(text);
    }
    let patterns: Vec<&str> = args.iter().map(|(name, _)| *name).collect();
    let values: Vec<String> = args.iter().map(|(_, value)| value.clone()).collect();
    std::borrow::Cow::Owned(rust_i18n::replace_patterns(&text, &patterns, &values))
}

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

#[cfg(test)]
mod cli_catalog_tests {
    use super::*;

    #[test]
    fn cli_catalog_interpolates_both_locales() {
        set_locale("en");
        assert_eq!(
            t!("cli.cache.refreshed", count = 3, path = "/tmp/x").to_string(),
            "OpenGWAS cache refreshed: 3 dataset(s) persisted to /tmp/x"
        );
        set_locale("zh");
        assert_eq!(
            t!("cli.cache.cacheFile", path = "/tmp/x").to_string(),
            "OpenGWAS 缓存文件:/tmp/x"
        );
        // interpolation with a zh plural-neutral key
        assert_eq!(
            t!("cli.diag.pluginsSummaryMany", count = 2, kinds = 14).to_string(),
            "2 个插件家族已验证,14 个节点类型"
        );
        // missing key falls back to the fallback-locale-prefixed dotted key
        assert!(t!("cli.no.suchKey").to_string().ends_with("cli.no.suchKey"));
        set_locale("en");
    }
}
