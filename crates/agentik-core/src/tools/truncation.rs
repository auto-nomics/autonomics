//! Unified tool output truncation.
//!
//! Output is bounded by both line count and character count. Character count
//! is used instead of byte count so multibyte UTF-8 content is always split
//! safely and consistently for callers.

/// Default maximum lines per tool output.
pub const DEFAULT_MAX_LINES: usize = 2_000;
/// Default maximum characters per tool output.
pub const DEFAULT_MAX_CHARS: usize = 50_000;

/// Limits for tool output truncation.
#[derive(Debug, Clone, Copy)]
pub struct TruncationConfig {
    /// Maximum number of lines to retain.
    pub max_lines: usize,
    /// Maximum number of characters to retain.
    pub max_chars: usize,
}

impl Default for TruncationConfig {
    fn default() -> Self {
        Self {
            max_lines: DEFAULT_MAX_LINES,
            max_chars: DEFAULT_MAX_CHARS,
        }
    }
}

/// Result of truncating a tool output string.
#[derive(Debug, Clone)]
pub struct TruncatedOutput {
    /// The possibly truncated content.
    pub content: String,
    /// Whether truncation was applied.
    pub truncated: bool,
}

/// Truncate output using a head/tail preservation strategy.
///
/// Line overflow keeps the first and last lines. Any result above the
/// character limit is then truncated on UTF-8 character boundaries, including
/// the truncation marker in the configured budget.
pub fn truncate_tool_output(content: &str, config: &TruncationConfig) -> TruncatedOutput {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= config.max_lines && content.chars().count() <= config.max_chars {
        return TruncatedOutput {
            content: content.to_string(),
            truncated: false,
        };
    }

    let mut result = String::new();
    if lines.len() > config.max_lines {
        let head_lines = config.max_lines.div_ceil(2);
        let tail_lines = config.max_lines / 2;
        let head_end = head_lines.min(lines.len());
        let tail_start = lines.len().saturating_sub(tail_lines);
        let omitted_lines = lines.len() - head_end - tail_lines.min(lines.len() - head_end);

        for line in &lines[..head_end] {
            result.push_str(line);
            result.push('\n');
        }
        result.push_str(&format!(
            "\n... [output truncated: omitted {omitted_lines} lines] ...\n\
             Use 'read' tool with offset/limit to view the full content.\n"
        ));
        for line in &lines[tail_start..] {
            result.push_str(line);
            result.push('\n');
        }
    } else {
        result.push_str(content);
    }

    if result.chars().count() > config.max_chars {
        result = truncate_characters(&result, config.max_chars);
    }

    TruncatedOutput {
        content: result,
        truncated: true,
    }
}

fn truncate_characters(content: &str, max_chars: usize) -> String {
    let marker = "\n\n... [output truncated] ...\n\
                  Use 'read' tool with offset/limit to view the full content.\n\n";
    let marker_chars = marker.chars().count();
    if marker_chars >= max_chars {
        return marker.chars().take(max_chars).collect();
    }

    let content_budget = max_chars - marker_chars;
    let head_chars = content_budget.div_ceil(2);
    let tail_chars = content_budget - head_chars;
    let total_chars = content.chars().count();
    let head: String = content.chars().take(head_chars).collect();
    let tail_start = total_chars.saturating_sub(tail_chars);
    let tail: String = content.chars().skip(tail_start).collect();

    let mut truncated = String::with_capacity(max_chars);
    truncated.push_str(&head);
    truncated.push_str(marker);
    truncated.push_str(&tail);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_truncation_for_short_content() {
        let output = truncate_tool_output("hello world", &TruncationConfig::default());
        assert!(!output.truncated);
        assert_eq!(output.content, "hello world");
    }

    #[test]
    fn no_truncation_within_limits() {
        let config = TruncationConfig {
            max_lines: 100,
            max_chars: 10_000,
        };
        let content = "line\n".repeat(50);
        let output = truncate_tool_output(&content, &config);
        assert!(!output.truncated);
        assert_eq!(output.content, content);
    }

    #[test]
    fn truncates_by_lines_and_preserves_head_tail() {
        let config = TruncationConfig {
            max_lines: 10,
            max_chars: 1_000_000,
        };
        let content = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = truncate_tool_output(&content, &config);
        assert!(output.truncated);
        assert!(output.content.starts_with("line 0"));
        assert!(output.content.contains("line 19"));
        assert!(output.content.contains("omitted 10 lines"));
    }

    #[test]
    fn truncates_by_characters() {
        let config = TruncationConfig {
            max_lines: 1_000_000,
            max_chars: 100,
        };
        let output = truncate_tool_output(&"a".repeat(500), &config);
        assert!(output.truncated);
        assert!(output.content.contains("[output truncated]"));
        assert_eq!(output.content.chars().count(), 100);
    }

    #[test]
    fn character_truncation_is_utf8_safe() {
        let config = TruncationConfig {
            max_lines: 1_000_000,
            max_chars: 101,
        };
        let output = truncate_tool_output(&"世".repeat(500), &config);
        assert!(output.truncated);
        assert!(output.content.contains("[output truncated]"));
        assert_eq!(output.content.chars().count(), 101);
    }

    #[test]
    fn empty_content_is_unchanged() {
        let output = truncate_tool_output("", &TruncationConfig::default());
        assert!(!output.truncated);
        assert_eq!(output.content, "");
    }
}
