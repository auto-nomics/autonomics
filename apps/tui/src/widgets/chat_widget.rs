mod md_highlight;
mod md_math;
mod md_renderer;
mod md_table;
mod md_theme;
pub(crate) mod render;
pub(crate) mod text_layout;

use ratatui::{
    layout::Rect,
    prelude::{StatefulWidget, Widget},
    style::{Color, Style},
    text::Line,
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

/// State for [`ChatWidget`].
pub struct ChatWidgetState {
    pub total_lines: usize,
    pub viewport_height: u16,
    pub scroll_offset: usize,
}

impl ChatWidgetState {
    pub fn new(scroll_offset: usize) -> Self {
        Self {
            total_lines: 0,
            viewport_height: 0,
            scroll_offset,
        }
    }
}

/// Chat message list with scrolling and scrollbar support.
///
/// Receives **pre-rendered** lines (assembled and cached by the caller) so the
/// expensive markdown-parse / layout pass is never repeated for unchanged
/// messages. See `AgentTabWidget` for the per-message incremental cache.
pub struct ChatWidget<'a> {
    pub lines: &'a [Line<'static>],
}

impl StatefulWidget for ChatWidget<'_> {
    type State = ChatWidgetState;

    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        state.viewport_height = area.height;

        // `trim: false` preserves leading whitespace so that pretty-printed JSON
        // (and indented markdown code blocks) keep their indentation when rendered.
        let paragraph = Paragraph::new(self.lines.to_vec()).wrap(Wrap { trim: false });

        state.total_lines = paragraph.line_count(area.width);

        let paragraph = paragraph.scroll((state.scroll_offset as u16, 0));
        paragraph.render(area, buf);

        // Render scrollbar overlaid on the right edge of the chat area
        if state.total_lines > area.height as usize {
            let mut scrollbar_state = ScrollbarState::new(state.total_lines - area.height as usize)
                .position(state.scroll_offset)
                .viewport_content_length(area.height as usize);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_style(Style::default().fg(Color::DarkGray))
                .track_style(Style::default().fg(Color::Rgb(40, 40, 40)));
            scrollbar.render(area, buf, &mut scrollbar_state);
        }
    }
}
