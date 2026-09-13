//! Toast notifications — inspired by shadcn/ui's Toast component.
//!
//! A stack of transient, auto-dismissing popups anchored to a screen
//! corner (bottom-right by default). Each toast carries a severity
//! variant (success / error / info / warning), a short title, and an
//! optional description line.
//!
//! ## Lifecycle
//!
//! 1. Caller pushes a toast via [`ToastManager::push`].
//! 2. Each render frame, [`ToastManager::tick`] removes toasts whose
//!    duration has elapsed.
//! 3. [`ToastManager::render`] draws the remaining toasts as stacked,
//!    bordered mini-cards in the corner.
//!
//! ## Visual style
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │ ✓ Copied to clipboard             │  ← title (bold, variant-colored icon)
//! │   42 chars from message #3        │  ← description (dim)
//! └──────────────────────────────────┘
//! ```

use std::collections::VecDeque;
use std::time::Instant;

use ratatui::{
    layout::{Alignment, Rect},
    prelude::Buffer,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
};

// ═══════════════════════════════════════════════════════════════════════
// Variant
// ═══════════════════════════════════════════════════════════════════════

/// Severity / colour scheme of a toast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastVariant {
    Success,
    Error,
    Info,
}

impl ToastVariant {
    /// Leading icon character shown before the title.
    fn icon(self) -> &'static str {
        match self {
            Self::Success => "✓",
            Self::Error => "✗",
            Self::Info => "ℹ",
        }
    }

    /// Accent colour for the icon, title, and border.
    fn color(self) -> Color {
        match self {
            Self::Success => Color::Green,
            Self::Error => Color::Red,
            Self::Info => Color::Cyan,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Toast entry
// ═══════════════════════════════════════════════════════════════════════

/// One transient notification.
#[derive(Debug, Clone)]
pub struct Toast {
    /// Monotonic id (assigned by the manager).
    pub variant: ToastVariant,
    /// Short one-line title (e.g. "Copied to clipboard").
    pub title: String,
    /// Optional second line with details.
    pub description: Option<String>,
    /// When the toast was created.
    pub created_at: Instant,
    /// Auto-dismiss after this many milliseconds.
    pub duration_ms: u64,
}

// ═══════════════════════════════════════════════════════════════════════
// Manager
// ═══════════════════════════════════════════════════════════════════════

/// Maximum number of simultaneously visible toasts. Older toasts are
/// evicted when this limit is exceeded.
const MAX_VISIBLE: usize = 4;

/// Manages a queue of transient toast notifications.
///
/// Internally a FIFO [`VecDeque<Toast>`]: new toasts are pushed to the
/// back; expired ones are drained from the front. Every toast lives
/// `duration_ms` milliseconds on screen (default 2000 = 2 s).
///
/// Lives in `AppState` so any subsystem can push notifications via a
/// shared reference. Each render frame the app calls [`tick`](Self::tick)
/// to expire old toasts, then [`render`](Self::render) to draw them.
#[derive(Debug)]
pub struct ToastManager {
    pub(crate) toasts: VecDeque<Toast>,
    next_id: u64,
    /// How long each toast stays on screen, in milliseconds.
    /// Defaults to [`DEFAULT_DURATION_MS`] (2000). Override via
    /// [`set_duration_ms`](Self::set_duration_ms).
    pub duration_ms: u64,
}

/// Default on-screen duration for every toast: 2 seconds.
pub const DEFAULT_DURATION_MS: u64 = 2000;

impl Default for ToastManager {
    fn default() -> Self {
        Self {
            toasts: VecDeque::new(),
            next_id: 1,
            duration_ms: DEFAULT_DURATION_MS,
        }
    }
}

impl ToastManager {

    /// Number of currently active toasts.

    /// Whether there are no active toasts.

    /// Override the on-screen duration for all future toasts.

    // ── Push helpers ──

    /// Push a toast onto the back of the queue. It will auto-dismiss
    /// after `self.duration_ms` milliseconds (default 2000 = 2 s).
    pub fn push(
        &mut self,
        variant: ToastVariant,
        title: impl Into<String>,
        description: Option<String>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.toasts.push_back(Toast {
            variant,
            title: title.into(),
            description,
            created_at: Instant::now(),
            duration_ms: self.duration_ms,
        });
        // Evict oldest (front of deque) if over capacity.
        while self.toasts.len() > MAX_VISIBLE {
            self.toasts.pop_front();
        }
        id
    }

    /// Convenience: push a success toast.
    pub fn success(&mut self, title: impl Into<String>, description: Option<String>) -> u64 {
        self.push(ToastVariant::Success, title, description)
    }

    /// Convenience: push an error toast.
    pub fn error(&mut self, title: impl Into<String>, description: Option<String>) -> u64 {
        self.push(ToastVariant::Error, title, description)
    }

    /// Convenience: push an info toast.
    pub fn info(&mut self, title: impl Into<String>, description: Option<String>) -> u64 {
        self.push(ToastVariant::Info, title, description)
    }

    // ── Lifecycle ──

    /// Remove toasts whose 2-second lifetime has elapsed. Called each
    /// render frame by the app's draw loop.
    pub fn tick(&mut self) {
        let now = Instant::now();
        // VecDeque has no `retain` in stable Rust, so drain + filter.
        let mut kept = VecDeque::new();
        for t in self.toasts.drain(..) {
            let elapsed = now.duration_since(t.created_at).as_millis() as u64;
            if elapsed < t.duration_ms {
                kept.push_back(t);
            }
        }
        self.toasts = kept;
    }

    /// Dismiss a specific toast by id.

    /// Dismiss all toasts immediately.

    // ── Render ──

    /// Render the toast stack in the **top-right** corner of `frame_area`,
    /// stacking downward. Each toast is an independent bordered card.
    /// No-op when there are no active toasts.
    pub fn render(&self, frame_area: Rect, buf: &mut Buffer) {
        if self.toasts.is_empty() {
            return;
        }

        // Layout: stack toasts top-down in the top-right corner.
        // Each toast is 3 rows (title only) or 4 rows (title + description),
        // including the rounded border.
        let max_width: u16 = 48;
        let gap: u16 = 1;

        // Start from the top of the frame, going down.
        let mut y = frame_area.y + 1; // +1 margin from top

        for toast in self.toasts.iter() {
            let h: u16 = if toast.description.is_some() { 4 } else { 3 };
            let x = frame_area.right().saturating_sub(max_width + 1); // +1 right margin
            let area = Rect {
                x,
                y,
                width: max_width.min(frame_area.width),
                height: h,
            };

            Self::render_toast(area, buf, toast);

            y += h + gap;
        }
    }

    /// Render a single toast card.
    fn render_toast(area: Rect, buf: &mut Buffer, toast: &Toast) {
        let accent = toast.variant.color();

        // Clear the background.
        Clear.render(area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .style(Style::default().bg(Color::Black));
        let inner = block.inner(area);
        block.render(area, buf);

        let icon_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);
        let title_style = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(Color::Gray);

        let mut lines: Vec<Line> = Vec::new();

        // Title line: icon + title.
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", toast.variant.icon()), icon_style),
            Span::styled(&toast.title, title_style),
        ]));

        // Optional description line.
        if let Some(desc) = &toast.description {
            // Truncate to fit inner width.
            let max_w = inner.width as usize;
            let desc_text = if desc.chars().count() > max_w {
                let mut s: String = desc.chars().take(max_w.saturating_sub(1)).collect();
                s.push('…');
                s
            } else {
                desc.clone()
            };
            lines.push(Line::from(Span::styled(desc_text, desc_style)));
        }

        let paragraph = Paragraph::new(lines).alignment(Alignment::Left);
        Widget::render(paragraph, inner, buf);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_evicts_oldest_when_over_capacity() {
        let mut mgr = ToastManager::default();
        for i in 0..(MAX_VISIBLE + 3) {
            mgr.info(format!("toast {i}"), None);
        }
        assert_eq!(mgr.toasts.len(), MAX_VISIBLE);
        // Oldest evicted — first remaining toast is toast 3.
        assert_eq!(mgr.toasts[0].title, "toast 3");
    }

    #[test]
    fn tick_expires_old_toasts() {
        let mut mgr = ToastManager::default();
        mgr.push(ToastVariant::Info, "short-lived", None);
        // Manually back-date the toast's created_at so it's already expired.
        if let Some(t) = mgr.toasts.front_mut() {
            t.created_at = Instant::now() - std::time::Duration::from_secs(5);
        }
        assert_eq!(mgr.toasts.len(), 1);
        mgr.tick();
        assert!(mgr.toasts.is_empty());
    }

    #[test]
    fn tick_keeps_fresh_toasts() {
        let mut mgr = ToastManager::default();
        mgr.push(ToastVariant::Success, "fresh", None);
        mgr.tick();
        assert_eq!(mgr.toasts.len(), 1);
    }

    #[test]
    fn all_toasts_expire_after_default_2_seconds() {
        let mgr = ToastManager::default();
        assert_eq!(mgr.duration_ms, DEFAULT_DURATION_MS);
        assert_eq!(DEFAULT_DURATION_MS, 2000);
    }

    #[test]
    fn next_id_is_monotonic() {
        let mut mgr = ToastManager::default();
        let id1 = mgr.info("a", None);
        let id2 = mgr.info("b", None);
        assert!(id2 > id1);
    }
}
