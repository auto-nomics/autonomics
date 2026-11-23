//! Runtime proof that the embedded `tui.*` catalog resolves in both
//! locales (guards against a catalog-format regression silently returning
//! the raw key).

use i18n::{set_locale, t};

#[test]
fn tui_catalog_resolves_both_locales() {
    set_locale("en");
    assert_eq!(
        t!("tui.status.idle").to_string(),
        "idle",
        "en tui.* lookups must return the catalog value, not the key"
    );
    assert_eq!(
        t!("tui.chat.moreLines", count = 3).to_string(),
        "  … +3 more lines"
    );

    set_locale("zh");
    assert_eq!(t!("tui.status.idle").to_string(), "空闲");
    assert_eq!(
        t!("tui.chat.moreLines", count = 3).to_string(),
        "  … 另有 3 行"
    );

    set_locale("en");
}
