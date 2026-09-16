use serde::{Deserialize, Serialize};

use crate::Usage;

/// Cumulative resource consumption for one agent session.
///
/// `time_consume_ms` counts active agent work only. A turn paused in
/// `Waiting` for a background tool does not accumulate wall-clock time while
/// no agent work is running.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SessionTelemetry {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    /// Raw token units reported by providers, including cache reads/writes.
    pub total_tokens: u64,
    pub llm_call_count: u64,
    pub turn_count: u64,
    pub total_tool_use: u64,
    pub tool_result_count: u64,
    pub failed_tool_result_count: u64,
    pub time_consume_ms: u64,
}

/// Resource consumption attributable to one explicitly tracked turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TurnTelemetry {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub total_tokens: u64,
    pub llm_call_count: u64,
    pub total_tool_use: u64,
    pub tool_result_count: u64,
    pub failed_tool_result_count: u64,
    pub time_consume_ms: u64,
}

impl SessionTelemetry {
    pub fn record_usage(&mut self, usage: &Usage) {
        self.input_tokens += usage.input_tokens;
        self.output_tokens += usage.output_tokens;
        self.cache_read_input_tokens += usage.cache_read_input_tokens.unwrap_or(0);
        self.cache_creation_input_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
        self.total_tokens += usage.input_tokens
            + usage.output_tokens
            + usage.cache_read_input_tokens.unwrap_or(0)
            + usage.cache_creation_input_tokens.unwrap_or(0);
        self.llm_call_count += 1;
    }

    pub fn record_tool_use(&mut self, count: u64) {
        self.total_tool_use += count;
    }

    pub fn record_tool_results(&mut self, total: u64, failed: u64) {
        self.tool_result_count += total;
        self.failed_tool_result_count += failed;
    }

    pub fn record_elapsed(&mut self, elapsed_ms: u64) {
        self.time_consume_ms = self.time_consume_ms.saturating_add(elapsed_ms);
    }

    fn delta_since(&self, baseline: &Self) -> TurnTelemetry {
        TurnTelemetry {
            input_tokens: self.input_tokens.saturating_sub(baseline.input_tokens),
            output_tokens: self.output_tokens.saturating_sub(baseline.output_tokens),
            cache_read_input_tokens: self
                .cache_read_input_tokens
                .saturating_sub(baseline.cache_read_input_tokens),
            cache_creation_input_tokens: self
                .cache_creation_input_tokens
                .saturating_sub(baseline.cache_creation_input_tokens),
            total_tokens: self.total_tokens.saturating_sub(baseline.total_tokens),
            llm_call_count: self.llm_call_count.saturating_sub(baseline.llm_call_count),
            total_tool_use: self.total_tool_use.saturating_sub(baseline.total_tool_use),
            tool_result_count: self
                .tool_result_count
                .saturating_sub(baseline.tool_result_count),
            failed_tool_result_count: self
                .failed_tool_result_count
                .saturating_sub(baseline.failed_tool_result_count),
            time_consume_ms: 0,
        }
    }
}

impl TurnTelemetry {
    #[must_use]
    pub fn from_session_delta(
        current: &SessionTelemetry,
        baseline: &SessionTelemetry,
        elapsed_ms: u64,
    ) -> Self {
        let mut telemetry = current.delta_since(baseline);
        telemetry.time_consume_ms = elapsed_ms;
        telemetry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_totals_include_cache_tokens() {
        let usage = Usage {
            input_tokens: 11,
            output_tokens: 6,
            cache_read_input_tokens: Some(3),
            cache_creation_input_tokens: Some(2),
            ..Default::default()
        };
        let mut telemetry = SessionTelemetry::default();
        telemetry.record_usage(&usage);

        assert_eq!(telemetry.total_tokens, 22);
        assert_eq!(telemetry.llm_call_count, 1);
    }

    #[test]
    fn turn_telemetry_is_the_delta_from_turn_baseline() {
        let baseline = SessionTelemetry {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            llm_call_count: 1,
            total_tool_use: 1,
            ..Default::default()
        };
        let mut session = baseline;
        session.record_usage(&Usage {
            input_tokens: 2,
            output_tokens: 3,
            ..Default::default()
        });
        session.record_tool_use(1);
        session.record_tool_results(1, 1);
        session.record_elapsed(250);

        let turn = TurnTelemetry::from_session_delta(&session, &baseline, 250);
        assert_eq!(turn.input_tokens, 2);
        assert_eq!(turn.output_tokens, 3);
        assert_eq!(turn.total_tokens, 5);
        assert_eq!(turn.llm_call_count, 1);
        assert_eq!(turn.total_tool_use, 1);
        assert_eq!(turn.failed_tool_result_count, 1);
        assert_eq!(turn.time_consume_ms, 250);
    }
}
