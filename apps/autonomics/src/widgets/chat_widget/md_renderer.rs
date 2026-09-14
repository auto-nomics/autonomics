//! Markdown renderer powered by pulldown-cmark.
//!
//! Walks pulldown-cmark events and produces `Vec<Line<'static>>` suitable
//! for ratatui widgets.  Supports headings, code blocks (with syntect
//! highlighting), inline code, bold/italic/strikethrough, links, ordered and
//! unordered (nested) lists, task lists, blockquotes, horizontal rules,
//! tables (with fair-share column widths and span-level wrapping), and
//! LaTeX math (inline + display).

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::md_highlight::highlight_code;
use super::md_math::latex_to_unicode;
use super::md_table::{self, TableRenderData};
use super::md_theme::MdTokens;
use super::text_layout;

/// Type alias shared between the renderer and the table module.
pub(crate) type CellSpans = Vec<Span<'static>>;

/// Render a markdown string into styled ratatui lines.
///
/// # Arguments
///
/// * `content`         – raw markdown source.
/// * `available_width` – terminal columns available (used for table layout).
pub(crate) fn render_markdown_to_lines(
    content: &str,
    available_width: usize,
) -> Vec<Line<'static>> {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH;
    let parser = Parser::new_ext(content, opts);
    let tokens = MdTokens::dark();
    let renderer = MdRenderer::new(tokens, available_width);
    renderer.render(parser)
}

// ── Internal renderer ────────────────────────────────────────────────

struct MdRenderer {
    /// All rendered lines accumulated so far.
    lines: Vec<Line<'static>>,
    /// Spans for the current line being built.
    current_spans: Vec<Span<'static>>,
    /// Style nesting stack for emphasis / links / headings.
    style_stack: Vec<Style>,
    list_depth: usize,
    list_counters: Vec<Option<u64>>,
    in_code_block: bool,
    code_block_lang: Option<String>,
    code_block_content: Vec<String>,
    in_heading: bool,
    heading_level: u8,
    heading_text: String,
    in_blockquote: bool,
    in_table: bool,
    table_alignments: Vec<pulldown_cmark::Alignment>,
    table_row: Vec<CellSpans>,
    table_rows: Vec<Vec<CellSpans>>,
    table_header_row: Option<Vec<CellSpans>>,
    table_header: bool,
    /// Terminal columns available for table layout.
    available_width: usize,
    /// Design tokens (colours).
    tokens: MdTokens,
    /// Syntect theme name for code-block highlighting.
    syntax_theme_name: &'static str,
}

impl MdRenderer {
    fn new(tokens: MdTokens, available_width: usize) -> Self {
        Self {
            lines: Vec::new(),
            current_spans: Vec::new(),
            style_stack: vec![Style::default().fg(tokens.text.primary)],
            list_depth: 0,
            list_counters: Vec::new(),
            in_code_block: false,
            code_block_lang: None,
            code_block_content: Vec::new(),
            in_heading: false,
            heading_level: 0,
            heading_text: String::new(),
            in_blockquote: false,
            in_table: false,
            table_alignments: Vec::new(),
            table_row: Vec::new(),
            table_rows: Vec::new(),
            table_header_row: None,
            table_header: false,
            available_width,
            tokens,
            syntax_theme_name: MdTokens::syntax_theme_name(),
        }
    }

    fn current_style(&self) -> Style {
        self.style_stack.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, modifier: Style) {
        let base = self.current_style();
        self.style_stack.push(base.patch(modifier));
    }

    fn pop_style(&mut self) {
        if self.style_stack.len() > 1 {
            self.style_stack.pop();
        }
    }

    fn flush_line(&mut self) {
        if self.in_table {
            return;
        }
        let spans = std::mem::take(&mut self.current_spans);
        if self.in_blockquote && !self.in_code_block {
            let mut bq_spans = vec![Span::styled(
                "│ ".to_string(),
                Style::default().fg(self.tokens.list.block_quote_border),
            )];
            bq_spans.extend(spans);
            self.lines.push(Line::from(bq_spans));
        } else {
            self.lines.push(Line::from(spans));
        }
    }

    fn push_blank_line(&mut self) {
        if self.in_table {
            return;
        }
        self.lines.push(Line::from(""));
    }

    /// Drive the render loop.
    fn render(mut self, parser: Parser) -> Vec<Line<'static>> {
        for (event, _span) in parser.into_offset_iter() {
            match event {
                Event::Start(tag) => self.start_tag(tag),
                Event::End(tag) => self.end_tag(tag),
                Event::Text(text) => self.handle_text(&text),
                Event::Code(code) => {
                    let style = self
                        .current_style()
                        .fg(self.tokens.syntax.inline_code)
                        .add_modifier(Modifier::BOLD);
                    self.current_spans
                        .push(Span::styled(format!("`{code}`"), style));
                    if self.in_heading {
                        self.heading_text.push_str(&code);
                    }
                }
                Event::SoftBreak => {
                    let in_list = self.list_depth > 0;
                    if self.in_table || in_list {
                        self.current_spans
                            .push(Span::styled(" ".to_string(), self.current_style()));
                    } else {
                        self.flush_line();
                    }
                }
                Event::HardBreak => {
                    self.flush_line();
                }
                Event::Rule => {
                    self.flush_line();
                    self.lines.push(Line::from(Span::styled(
                        "─".repeat(60),
                        Style::default().fg(self.tokens.text.muted),
                    )));
                    self.push_blank_line();
                }
                Event::TaskListMarker(checked) => {
                    let marker = if checked { "☑ " } else { "☐ " };
                    self.current_spans.push(Span::styled(
                        marker.to_string(),
                        Style::default().fg(self.tokens.list.task_marker),
                    ));
                }
                Event::InlineMath(math) => {
                    let rendered = latex_to_unicode(&math);
                    let style = self
                        .current_style()
                        .fg(self.tokens.syntax.inline_code)
                        .add_modifier(Modifier::ITALIC);
                    self.current_spans.push(Span::styled(rendered, style));
                }
                Event::DisplayMath(math) => {
                    let rendered = latex_to_unicode(&math);
                    self.flush_line();
                    let border_style = Style::default().fg(self.tokens.syntax.code_border);
                    let math_style = Style::default()
                        .fg(self.tokens.syntax.code_fg)
                        .bg(self.tokens.surface.raised)
                        .add_modifier(Modifier::ITALIC);
                    let math_lines: Vec<&str> = rendered.lines().collect();
                    let max_width = math_lines
                        .iter()
                        .map(|l| UnicodeWidthStr::width(*l))
                        .max()
                        .unwrap_or(0)
                        .max(20);
                    let inner_width = max_width + 1;
                    let label = " math ";

                    self.push_blank_line();
                    // Top border with "math" label.
                    self.lines.push(Line::from(vec![
                        Span::styled("╭".to_string(), border_style),
                        Span::styled(
                            label.to_string(),
                            Style::default()
                                .fg(self.tokens.syntax.inline_code)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(
                                "{}╮",
                                "─".repeat(inner_width + 1 - label.len().min(inner_width))
                            ),
                            border_style,
                        ),
                    ]));
                    // Content lines.
                    for line in &math_lines {
                        self.lines.push(Line::from(vec![
                            Span::styled(
                                "│ ".to_string(),
                                Style::default()
                                    .fg(self.tokens.syntax.code_border)
                                    .bg(self.tokens.surface.raised),
                            ),
                            Span::styled(format!("{line:<inner_width$}"), math_style),
                            Span::styled(
                                "│".to_string(),
                                Style::default()
                                    .fg(self.tokens.syntax.code_border)
                                    .bg(self.tokens.surface.raised),
                            ),
                        ]));
                    }
                    // Bottom border.
                    self.lines.push(Line::from(Span::styled(
                        format!("╰{}╯", "─".repeat(inner_width + 1)),
                        border_style,
                    )));
                    self.push_blank_line();
                }
                _ => {}
            }
        }
        // Flush any remaining spans.
        if !self.current_spans.is_empty() {
            self.flush_line();
        }
        self.lines
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => {
                self.in_heading = true;
                self.heading_level = level as u8;
                self.heading_text.clear();
                let color = match level {
                    pulldown_cmark::HeadingLevel::H1 => self.tokens.heading.h1,
                    pulldown_cmark::HeadingLevel::H2 => self.tokens.heading.h2,
                    pulldown_cmark::HeadingLevel::H3 => self.tokens.heading.h3,
                    _ => self.tokens.heading.other,
                };
                let mut style = Style::default().fg(color).add_modifier(Modifier::BOLD);
                if level == pulldown_cmark::HeadingLevel::H1 {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                self.push_style(style);
                let prefix = match level {
                    pulldown_cmark::HeadingLevel::H1 => "█ ",
                    pulldown_cmark::HeadingLevel::H2 => "▌ ",
                    pulldown_cmark::HeadingLevel::H3 => "▎ ",
                    _ => "  ",
                };
                self.current_spans
                    .push(Span::styled(prefix.to_string(), self.current_style()));
            }
            Tag::Paragraph => {}
            Tag::BlockQuote(_) => {
                self.in_blockquote = true;
                self.push_style(Style::default().fg(self.tokens.list.block_quote_fg));
            }
            Tag::CodeBlock(kind) => {
                self.in_code_block = true;
                self.code_block_lang = match &kind {
                    CodeBlockKind::Fenced(lang) => {
                        let s = lang.trim().to_lowercase();
                        if s.is_empty() { None } else { Some(s) }
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code_block_content.clear();
                if !self.current_spans.is_empty() {
                    self.flush_line();
                }
            }
            Tag::List(start) => {
                self.list_depth += 1;
                self.list_counters.push(start);
            }
            Tag::Item => {
                if !self.current_spans.is_empty() {
                    self.flush_line();
                }
                let indent = "  ".repeat(self.list_depth.saturating_sub(1));
                let bullet = if let Some(counter) = self.list_counters.last_mut() {
                    if let Some(n) = counter {
                        let bullet = format!("{indent}{n}. ");
                        *n += 1;
                        bullet
                    } else {
                        let marker = match self.list_depth {
                            1 => "•",
                            2 => "◦",
                            _ => "▪",
                        };
                        format!("{indent}{marker} ")
                    }
                } else {
                    format!("{indent}• ")
                };
                self.current_spans.push(Span::styled(
                    bullet,
                    Style::default().fg(self.tokens.list.marker),
                ));
            }
            Tag::Emphasis => {
                self.push_style(Style::default().add_modifier(Modifier::ITALIC));
            }
            Tag::Strong => {
                self.push_style(Style::default().add_modifier(Modifier::BOLD));
            }
            Tag::Strikethrough => {
                self.push_style(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                self.push_style(
                    Style::default()
                        .fg(self.tokens.accent.link)
                        .add_modifier(Modifier::UNDERLINED),
                );
                // We don't collect LinkInfo, but we need to pop the style on
                // TagEnd::Link. Store the URL so we can at least render the
                // link text with a clickable hint.
                let _ = dest_url;
            }
            Tag::Table(alignments) => {
                self.in_table = true;
                self.table_alignments = alignments;
                self.table_rows.clear();
                self.table_header_row = None;
                self.flush_line();
            }
            Tag::TableHead => {
                self.table_header = true;
                self.table_row.clear();
            }
            Tag::TableRow => {
                self.table_row.clear();
            }
            Tag::TableCell => {}
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.pop_style();
                self.flush_line();
                self.push_blank_line();
                self.in_heading = false;
                self.heading_text.clear();
            }
            TagEnd::Paragraph => {
                self.flush_line();
                self.push_blank_line();
            }
            TagEnd::BlockQuote(_) => {
                self.in_blockquote = false;
                self.pop_style();
                self.push_blank_line();
            }
            TagEnd::CodeBlock => {
                self.render_code_block();
                self.in_code_block = false;
                self.code_block_lang = None;
            }
            TagEnd::List(_) => {
                self.list_depth = self.list_depth.saturating_sub(1);
                self.list_counters.pop();
                if self.list_depth == 0 {
                    self.push_blank_line();
                }
            }
            TagEnd::Item => {
                self.flush_line();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.pop_style();
            }
            TagEnd::Link => {
                self.pop_style();
            }
            TagEnd::Table => {
                self.emit_table_block();
                self.in_table = false;
            }
            TagEnd::TableHead => {
                self.table_header_row = Some(self.table_row.clone());
                self.table_header = false;
            }
            TagEnd::TableRow if !self.table_header => {
                self.table_rows.push(self.table_row.clone());
            }
            TagEnd::TableCell => {
                let cell_spans: CellSpans = self.current_spans.drain(..).collect();
                self.table_row.push(cell_spans);
            }
            _ => {}
        }
    }

    fn handle_text(&mut self, text: &str) {
        if self.in_code_block {
            for line in text.split('\n') {
                self.code_block_content.push(line.to_string());
            }
            if self.code_block_content.last().is_some_and(String::is_empty) {
                self.code_block_content.pop();
            }
        } else {
            if self.in_heading {
                self.heading_text.push_str(text);
            }
            self.current_spans
                .push(Span::styled(text.to_string(), self.current_style()));
        }
    }

    fn render_code_block(&mut self) {
        let border_style = Style::default().fg(self.tokens.syntax.code_border);

        let max_width = self
            .code_block_content
            .iter()
            .map(|l| UnicodeWidthStr::width(l.as_str()))
            .max()
            .unwrap_or(0)
            .max(20);
        let inner_width = max_width + 1;

        // Join and highlight.
        let source = self.code_block_content.join("\n");
        let token_lines = highlight_code(
            &source,
            self.code_block_lang.as_deref(),
            self.syntax_theme_name,
            self.tokens.syntax.code_fg,
            self.tokens.surface.raised,
        );

        self.push_blank_line();

        // Top border.
        self.lines.push(Line::from(Span::styled(
            format!("╭{}╮", "─".repeat(inner_width + 1)),
            border_style,
        )));

        // Content lines.
        for (i, (src_line, token_line)) in self
            .code_block_content
            .iter()
            .zip(token_lines.iter())
            .enumerate()
        {
            let line_width = UnicodeWidthStr::width(src_line.as_str());
            let pad_len = inner_width.saturating_sub(line_width);

            let mut spans: Vec<Span<'static>> = Vec::with_capacity(token_line.len() + 3);

            spans.push(Span::styled(
                "│ ".to_string(),
                Style::default()
                    .fg(self.tokens.syntax.code_border)
                    .bg(self.tokens.surface.raised),
            ));

            for (text, style) in token_line {
                spans.push(Span::styled(text.clone(), *style));
            }

            if pad_len > 0 {
                spans.push(Span::styled(
                    " ".repeat(pad_len),
                    Style::default().bg(self.tokens.surface.raised),
                ));
            }

            spans.push(Span::styled(
                "│".to_string(),
                Style::default()
                    .fg(self.tokens.syntax.code_border)
                    .bg(self.tokens.surface.raised),
            ));

            self.lines.push(Line::from(spans));

            let _ = i;
            let _ = src_line;
        }

        // Bottom border.
        self.lines.push(Line::from(Span::styled(
            format!("╰{}╯", "─".repeat(inner_width + 1)),
            border_style,
        )));

        self.code_block_content.clear();
        self.push_blank_line();
    }

    fn emit_table_block(&mut self) {
        let headers = self.table_header_row.take().unwrap_or_default();
        let rows = std::mem::take(&mut self.table_rows);
        let alignments = std::mem::take(&mut self.table_alignments);

        let num_cols = headers
            .len()
            .max(rows.iter().map(Vec::len).max().unwrap_or(0));

        if num_cols == 0 {
            return;
        }

        let mut natural_widths = vec![0usize; num_cols];
        for (i, cell) in headers.iter().enumerate() {
            natural_widths[i] = natural_widths[i].max(text_layout::measure(cell) as usize);
        }
        for row in &rows {
            for (i, cell) in row.iter().enumerate() {
                if i < num_cols {
                    natural_widths[i] = natural_widths[i].max(text_layout::measure(cell) as usize);
                }
            }
        }
        for w in &mut natural_widths {
            *w = (*w).max(1);
        }

        let table = TableRenderData {
            headers,
            rows,
            alignments,
            natural_widths,
        };

        let table_lines = md_table::layout_table(&table, self.available_width as u16, &self.tokens);
        self.lines.extend(table_lines);
        self.push_blank_line();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(lines: &[ratatui::text::Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn code_fence_renders_in_box() {
        let md = "```bash\necho hello\nls /tmp\n```";
        let lines = render_markdown_to_lines(md, 80);
        let out = joined(&lines);

        assert!(
            out.contains('╭'),
            "code block must render inside a box:\n{out}"
        );
    }
}
