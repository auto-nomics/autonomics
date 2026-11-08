//! Canvas dimensions declared by a deck's theme stylesheet.
//!
//! The top-level `:root` or `:scope` rule may declare both
//! `--deck-width` and `--deck-height` as integer px values. Other rules,
//! strings, comments and per-slide CSS cannot change physical page size.
use crate::{Error, Result};
use cssparser::{Parser, ParserInput, ToCss, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasSize {
    pub width: u32,
    pub height: u32,
}

/// Starter file reflects the deck's initial Markdown dimensions.
pub fn starter_css(meta: &ppt_types::DeckMeta) -> String {
    css_for_size(CanvasSize { width: meta.canvas(), height: meta.canvas_height() })
}

/// Default design with the caller's existing physical page dimensions.
pub fn css_for_size(size: CanvasSize) -> String {
    if !(16..=8192).contains(&size.width) || !(16..=8192).contains(&size.height) {
        // Preserve legacy Markdown dimensions outside the supported CSS range.
        return include_str!("deck/default-theme.css").to_string();
    }
    format!(
        ":root {{\n  --deck-width: {}px;\n  --deck-height: {}px;\n}}\n\n{}",
        size.width,
        size.height,
        include_str!("deck/default-theme.css")
    )
}

pub fn canvas_size(css: &str, fallback: CanvasSize) -> Result<CanvasSize> {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut prelude = String::new();
    let mut width: Option<(u32, bool)> = None;
    let mut height: Option<(u32, bool)> = None;
    let mut invalid = false;
    while let Ok(token) = parser.next().cloned() {
        match token {
            Token::CurlyBracketBlock => {
                let selectors: Vec<_> = prelude.split(',').collect();
                let matches_root = selectors.iter().any(|s| *s == ":root" || *s == ":scope");
                let _ = parser.parse_nested_block::<_, _, ()>(|block| {
                    let mut declaration = Vec::new();
                    loop {
                        match block.next().cloned() {
                            Ok(Token::Semicolon) => {
                                if matches_root {
                                    read_declaration(
                                        &declaration,
                                        &mut width,
                                        &mut height,
                                        &mut invalid,
                                    );
                                }
                                declaration.clear();
                            }
                            Ok(t) => declaration.push(t),
                            Err(_) => {
                                if matches_root {
                                    read_declaration(
                                        &declaration,
                                        &mut width,
                                        &mut height,
                                        &mut invalid,
                                    );
                                }
                                break;
                            }
                        }
                    }
                    Ok(())
                });
                prelude.clear();
            }
            Token::Semicolon => prelude.clear(),
            other => prelude.push_str(&other.to_css_string()),
        }
    }
    if invalid || width.is_some() != height.is_some() {
        return Err(Error::InvalidThemeCss("在顶层 :root 或 :scope 中同时设置 --deck-width 和 --deck-height，值须为 16–8192 的整数 px。".into()));
    }
    Ok(match (width, height) {
        (Some((width, _)), Some((height, _))) => CanvasSize { width, height },
        _ => fallback,
    })
}

fn read_declaration(
    tokens: &[Token<'_>],
    width: &mut Option<(u32, bool)>,
    height: &mut Option<(u32, bool)>,
    invalid: &mut bool,
) {
    let Some(Token::Ident(name)) = tokens.first() else {
        return;
    };
    let target = match name.as_ref() {
        "--deck-width" => width,
        "--deck-height" => height,
        _ => return,
    };
    let [
        _,
        Token::Colon,
        Token::Dimension {
            int_value: Some(value),
            unit,
            ..
        },
        rest @ ..,
    ] = tokens
    else {
        *invalid = true;
        return;
    };
    let important = match rest {
        [] => false,
        [Token::Delim('!'), Token::Ident(word)] if word.eq_ignore_ascii_case("important") => true,
        _ => {
            *invalid = true;
            return;
        }
    };
    if !unit.eq_ignore_ascii_case("px") || !(16..=8192).contains(value) {
        *invalid = true;
        return;
    }
    if target.is_none_or(|(_, prior_important)| important || !prior_important) {
        *target = Some((*value as u32, important));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn starter_theme_has_design_and_respects_canvas_dimensions() {
        let deck = crate::deck::assemble("---\ntitle: Test\naspectRatio: 4/3\ncanvasWidth: 1200\n---\n\n# Test", "d", "", "");
        let css = starter_css(&deck.meta);
        assert!(css.contains("--bio-accent"));
        assert_eq!(canvas_size(&css, CanvasSize { width: 1, height: 1 }).unwrap(), CanvasSize { width: 1200, height: 900 });
    }
    #[test]
    fn root_dimensions_respect_comments_cascade_and_ignore_slide_css() {
        let default = CanvasSize {
            width: 980,
            height: 551,
        };
        let css = "/* --deck-width: 2px */ .example { --deck-width: 12px } :root { --deck-width: 1280px !important; --deck-height: 720px; } :scope { --deck-width: 400px; }";
        assert_eq!(
            canvas_size(css, default).unwrap(),
            CanvasSize {
                width: 1280,
                height: 720
            }
        );
        assert_eq!(
            canvas_size(".x { content: '--deck-width: 1px'; }", default).unwrap(),
            default
        );
        assert!(canvas_size(":root { --deck-width: 1280px }", default).is_err());
        assert!(
            canvas_size(
                ":root { --deck-width: calc(2px); --deck-height: 720px }",
                default
            )
            .is_err()
        );
    }
}
