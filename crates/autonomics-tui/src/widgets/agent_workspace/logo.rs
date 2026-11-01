use std::sync::LazyLock;

use ratatui::{
    layout::{Alignment, Rect},
    prelude::{Buffer, Widget},
    style::{Color, Style},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

const LOGO: &str = include_str!("logo.txt");

static LOGO_DIMENSIONS: LazyLock<(u16, u16)> = LazyLock::new(|| {
    let width = LOGO
        .lines()
        .map(|line| line.width())
        .max()
        .unwrap_or_default();
    let height = LOGO.lines().count();
    (
        u16::try_from(width).unwrap_or(u16::MAX),
        u16::try_from(height).unwrap_or(u16::MAX),
    )
});

/// Brand mark rendered as terminal-native text.
pub(super) struct Logo;

impl Logo {
    pub(super) fn dimensions() -> (u16, u16) {
        *LOGO_DIMENSIONS
    }

    pub(super) fn fits(area: Rect) -> bool {
        let (width, height) = Self::dimensions();
        area.width >= width && area.height >= height
    }
}

impl Widget for Logo {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (width, height) = Self::dimensions();
        let logo_area = Rect {
            x: area.x + area.width.saturating_sub(width) / 2,
            y: area.y + area.height.saturating_sub(height) / 2,
            width: width.min(area.width),
            height: height.min(area.height),
        };
        let lines = LOGO
            .lines()
            .map(|line| ratatui::text::Line::raw(line.to_owned()))
            .collect::<Vec<_>>();

        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Cyan))
            .render(logo_area, buf);
    }
}
