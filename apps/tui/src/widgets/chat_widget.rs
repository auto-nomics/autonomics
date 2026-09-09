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

const MAX_PARAGRAPH_SCROLL: usize = u16::MAX as usize;

fn visual_row_count(line: &Line<'_>, width: u16) -> usize {
    if width == 0 {
        return 1;
    }

    // Most cached chat lines are already pre-wrapped. Ratatui's Paragraph is
    // authoritative for the remaining long lines, where whitespace handling
    // can produce a different row count than a simple width division.
    if line.width() <= width as usize {
        return 1;
    }

    Paragraph::new(std::slice::from_ref(line))
        .wrap(Wrap { trim: false })
        .line_count(width)
}

fn virtual_window(
    lines: &[Line<'_>],
    mut skip_rows: usize,
    viewport_rows: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let mut window = Vec::new();
    if viewport_rows == 0 {
        return window;
    }

    for line in lines {
        let rows = visual_row_count(line, width);
        if skip_rows >= rows {
            skip_rows -= rows;
            continue;
        }

        let wrapped = text_layout::wrap_spans(&line.spans, width);
        let window_start = window.len();
        let exact_remaining = (rows - skip_rows).min(viewport_rows - window_start);
        window.extend(
            wrapped
                .into_iter()
                .skip(skip_rows)
                .take(exact_remaining)
                .map(|row| row.to_ratatui_line()),
        );
        while window.len() < window_start + exact_remaining {
            window.push(Line::default());
        }
        skip_rows = 0;

        if window.len() >= viewport_rows {
            break;
        }
    }

    window
}

impl StatefulWidget for ChatWidget<'_> {
    type State = ChatWidgetState;

    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        state.viewport_height = area.height;

        // `trim: false` preserves leading whitespace so that pretty-printed JSON
        // (and indented markdown code blocks) keep their indentation when rendered.
        state.total_lines = self
            .lines
            .iter()
            .map(|line| visual_row_count(line, area.width))
            .sum();

        // Paragraph's vertical scroll is a u16. Virtualize the visible rows
        // once the logical scroll offset exceeds that API's representable range.
        if state.scroll_offset > MAX_PARAGRAPH_SCROLL {
            let window = virtual_window(
                self.lines,
                state.scroll_offset,
                area.height as usize,
                area.width,
            );
            Paragraph::new(window).render(area, buf);
        } else {
            let paragraph = Paragraph::new(self.lines.to_vec())
                .wrap(Wrap { trim: false })
                .scroll((state.scroll_offset as u16, 0));
            paragraph.render(area, buf);
        }

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

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    #[test]
    fn visual_row_count_matches_paragraph_wrapping() {
        let cases = [
            ("", 10),
            ("aaaaaaaaaaaaaaaa", 4),
            ("aaa bbb ccc ddd", 5),
            ("aaaaaaaaaa        b", 10),
        ];

        for (text, width) in cases {
            let line = Line::raw(text);
            let paragraph = Paragraph::new(line).wrap(Wrap { trim: false });
            assert_eq!(
                visual_row_count(&Line::raw(text), width),
                paragraph.line_count(width),
                "mismatch for {text:?} at width {width}"
            );
        }
    }

    #[test]
    fn renders_the_actual_bottom_past_u16_scroll_limit() {
        const LINE_COUNT: usize = u16::MAX as usize + 3;
        let lines: Vec<Line<'static>> = (0..LINE_COUNT)
            .map(|i| Line::raw(format!("line {i:06}")))
            .collect();
        let mut terminal = Terminal::new(TestBackend::new(20, 1)).unwrap();

        terminal
            .draw(|frame| {
                let mut state = ChatWidgetState::new(LINE_COUNT - 1);
                ChatWidget { lines: &lines }.render(frame.area(), frame.buffer_mut(), &mut state);
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(
            rendered.contains(&format!("line {:06}", LINE_COUNT - 1)),
            "expected the final logical line, got: {rendered}"
        );
    }
}
