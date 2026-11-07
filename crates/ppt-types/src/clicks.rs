//! Progressive-reveal ("click") model, ported from Slidev.
//!
//! Ground truth: `slidev/packages/client/composables/useClicks.ts` and
//! `slidev/packages/types/src/clicks.ts` (v53.1.0, MIT).
//!
//! Semantics preserved exactly:
//!
//! - Element `at` values normalize to *disabled*, *absolute* (`>= 1`, values
//!   `<= 0` clamp to 1), or *relative* (`"+1"` / `"-1"` strings) — booleans
//!   map like Slidev (`false`/`"false"` → disabled; `true`/`"true`" → `+1`).
//! - Elements are folded in *document order*: a running offset accumulates
//!   the `delta` of every processed element (`currentOffset = sum of prior
//!   deltas`); a relative `at` resolves against that offset.
//! - Single (`since`) elements: `start = offset + v`, `max = start + size - 1`,
//!   `delta = v + size - 1`, `end = ∞` (they stay visible once revealed).
//! - Range elements `[a, b]`: start/end resolve independently (relative ones
//!   against the offset / against `start`), `max = end`,
//!   `delta = (a if relative) + (b if relative)`.
//! - `total = frontmatter.clicks ?? max(0, max of all elements)`.
//!
//! Rust computes the whole plan; the frontend only consumes the resulting
//! [`ClickPlan`] — no click algebra lives in TypeScript.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Raw single `at` value after normalization (Slidev `NormalizedSingleClickValue`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SingleAt {
    /// `"+2"` / `"-1"` — resolved against the running offset at fold time.
    Relative(String),
    /// Absolute click number (`>= 1`; `<= 0` clamped to 1 at normalization).
    Absolute(u32),
}

/// Normalized `at` value (Slidev `NormalizedAtValue`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NormalizedAt {
    Single(SingleAt),
    Range(Box<SingleAt>, Box<SingleAt>),
    /// `v-click="false"` — element never participates in the click sequence.
    Disabled,
}

/// Convenience alias mirroring Slidev's `RawAtValue` naming.
pub type AtValue = NormalizedAt;

/// Normalize a raw JSON `at` prop into [`NormalizedAt`].
///
/// Faithful port of `normalizeSingleAtValue` / `normalizeRangeAtValue`:
/// - `false` / `"false"` → disabled
/// - `null` / `true` / `"true"` → `"+1"`
/// - string starting with `+`/`-` → relative
/// - otherwise numeric: unparseable → disabled; `<= 0` → clamped to `1`
///   (Slidev warns and clamps; we cannot warn server-side, so the clamp is
///   the observable behavior)
/// - arrays normalize element-wise into a range
pub fn normalize_at(raw: &Value) -> NormalizedAt {
    if let Value::Array(arr) = raw {
        if arr.len() == 2 {
            let a = normalize_single(&arr[0]);
            let b = normalize_single(&arr[1]);
            if let (Some(a), Some(b)) = (a, b) {
                return NormalizedAt::Range(Box::new(a), Box::new(b));
            }
        }
        return NormalizedAt::Disabled;
    }
    match normalize_single(raw) {
        Some(at) => NormalizedAt::Single(at),
        None => NormalizedAt::Disabled,
    }
}

fn normalize_single(raw: &Value) -> Option<SingleAt> {
    match raw {
        Value::Bool(false) => None,
        Value::Null => Some(SingleAt::Relative("+1".into())),
        Value::Bool(true) => Some(SingleAt::Relative("+1".into())),
        Value::String(s) => {
            if s == "false" {
                None
            } else if s == "true" {
                Some(SingleAt::Relative("+1".into()))
            } else if s.starts_with('+') || s.starts_with('-') {
                Some(SingleAt::Relative(s.clone()))
            } else {
                numeric(s.parse::<f64>().ok(), s)
            }
        }
        Value::Number(n) => numeric(n.as_f64(), &n.to_string()),
        _ => None,
    }
}

fn numeric(v: Option<f64>, orig: &str) -> Option<SingleAt> {
    let Some(v) = v else {
        // Slidev console.errors and disables the element.
        return None;
    };
    if v <= 0.0 {
        // Slidev warns "must be greater than 0 … has been set to 1".
        return Some(SingleAt::Absolute(1));
    }
    // Fractional values have no click semantics; truncate like JS `+at`
    // consumers effectively do when comparing integers.
    let _ = orig;
    Some(SingleAt::Absolute(v as u32))
}

/// One click-marked element in document order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClickDirective {
    /// Document order within the slide (0-based).
    pub order: usize,
    /// Raw `at` value (JSON): null / bool / string / number / `[a, b]` pair.
    #[serde(default)]
    pub at: Value,
    /// How many clicks this element occupies (Slidev `size`, default 1).
    #[serde(default, skip_serializing_if = "is_one")]
    pub size: u32,
}

fn is_one(v: &u32) -> bool {
    *v == 1
}

impl ClickDirective {
    pub fn new(order: usize, at: Value) -> Self {
        Self {
            order,
            at,
            size: 1,
        }
    }

    pub fn with_size(mut self, size: u32) -> Self {
        self.size = size.max(1);
        self
    }
}

/// Resolved click state of one element.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementClickState {
    /// Document order (0-based), mirrors the originating directive.
    pub order: usize,
    /// Absolute click number at which the element becomes current.
    pub start: i64,
    /// Absolute click number at which the element hides again
    /// (range elements); `None` = stays visible (`end = ∞`).
    pub end: Option<i64>,
    /// Highest click number this element requires (drives `total`).
    pub max: i64,
    /// Amount this element adds to the running relative offset.
    pub delta: i64,
}

impl ElementClickState {
    /// Slidev `isActive`: for `since` elements `current >= start`;
    /// for range elements `start <= current < end`.
    pub fn is_active(&self, current: i64) -> bool {
        match self.end {
            None => current >= self.start,
            Some(end) => current >= self.start && current < end,
        }
    }

    /// Slidev `isCurrent`: the element is *the* current reveal.
    pub fn is_current(&self, current: i64) -> bool {
        current == self.start
    }
}

/// Computed click plan for one slide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClickPlan {
    /// `frontmatter.clicks ?? max(0, max of all elements)`.
    pub total: u32,
    /// `frontmatter.clicksStart` (default 0); current is clamped into
    /// `[clicksStart, total]`.
    pub clicks_start: u32,
    /// Per-element resolved states, in document order.
    pub elements: Vec<ElementClickState>,
}

impl ClickPlan {
    /// Clamp a raw current value into the valid range (Slidev `get current`).
    pub fn clamp_current(&self, current: i64) -> i64 {
        current.clamp(self.clicks_start as i64, self.total as i64)
    }

    /// Plan for a slide with no click directives.
    pub fn none(clicks_start: u32) -> Self {
        Self {
            total: 0,
            clicks_start,
            elements: Vec::new(),
        }
    }
}

/// Fold click directives in document order into a [`ClickPlan`].
///
/// `override_total` is `frontmatter.clicks` (manual total override);
/// `clicks_start` is `frontmatter.clicksStart` (default 0).
///
/// Port notes:
/// - The running offset accumulates **deltas** (`sum` of `relativeSizeMap`),
///   not maxima — matching `useClicks.ts` `currentOffset`.
/// - Disabled elements register nothing and add no delta.
/// - Relative values that resolve below 1 are kept as-is (Slidev does not
///   clamp resolved starts; the frontend's `isActive` comparison simply
///   never fires for them in practice).
pub fn compute_plan(
    directives: &[ClickDirective],
    override_total: Option<u32>,
    clicks_start: u32,
) -> ClickPlan {
    let mut offset: i64 = 0;
    let mut max_of_max: i64 = 0;
    let mut elements = Vec::with_capacity(directives.len());

    for d in directives {
        let at = normalize_at(&d.at);
        let size = i64::from(d.size.max(1));
        let state = match &at {
            NormalizedAt::Disabled => continue,
            NormalizedAt::Single(SingleAt::Relative(s)) => {
                let v: i64 = s.parse().unwrap_or(0);
                let start = offset + v;
                let max = start + size - 1;
                let delta = v + size - 1;
                ElementClickState {
                    order: d.order,
                    start,
                    end: None,
                    max,
                    delta,
                }
            }
            NormalizedAt::Single(SingleAt::Absolute(a)) => {
                let start = i64::from(*a);
                let max = start + size - 1;
                ElementClickState {
                    order: d.order,
                    start,
                    end: None,
                    max,
                    delta: 0,
                }
            }
            NormalizedAt::Range(a, b) => {
                let mut delta: i64 = 0;
                let start = match a.as_ref() {
                    SingleAt::Relative(s) => {
                        let v: i64 = s.parse().unwrap_or(0);
                        delta += v;
                        offset + v
                    }
                    SingleAt::Absolute(v) => i64::from(*v),
                };
                let end = match b.as_ref() {
                    SingleAt::Relative(s) => {
                        let v: i64 = s.parse().unwrap_or(0);
                        delta += v;
                        start + v
                    }
                    SingleAt::Absolute(v) => i64::from(*v),
                };
                ElementClickState {
                    order: d.order,
                    start,
                    end: Some(end),
                    max: end,
                    delta,
                }
            }
        };
        offset += state.delta;
        max_of_max = max_of_max.max(state.max);
        elements.push(state);
    }

    let total = override_total.unwrap_or_else(|| max_of_max.max(0) as u32);
    ClickPlan {
        total,
        clicks_start,
        elements,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(directives: &[ClickDirective]) -> ClickPlan {
        compute_plan(directives, None, 0)
    }

    fn d(order: usize, at: Value) -> ClickDirective {
        ClickDirective::new(order, at)
    }

    // ---- normalize_at -----------------------------------------------------

    #[test]
    fn normalize_single_table() {
        // false / "false" → disabled
        assert_eq!(normalize_at(&json!(false)), NormalizedAt::Disabled);
        assert_eq!(normalize_at(&json!("false")), NormalizedAt::Disabled);
        // null / true / "true" → +1
        assert_eq!(
            normalize_at(&Value::Null),
            NormalizedAt::Single(SingleAt::Relative("+1".into()))
        );
        assert_eq!(
            normalize_at(&json!(true)),
            NormalizedAt::Single(SingleAt::Relative("+1".into()))
        );
        assert_eq!(
            normalize_at(&json!("true")),
            NormalizedAt::Single(SingleAt::Relative("+1".into()))
        );
        // relative strings
        assert_eq!(
            normalize_at(&json!("+2")),
            NormalizedAt::Single(SingleAt::Relative("+2".into()))
        );
        assert_eq!(
            normalize_at(&json!("-1")),
            NormalizedAt::Single(SingleAt::Relative("-1".into()))
        );
        // absolute numbers
        assert_eq!(
            normalize_at(&json!(3)),
            NormalizedAt::Single(SingleAt::Absolute(3))
        );
        assert_eq!(
            normalize_at(&json!("3")),
            NormalizedAt::Single(SingleAt::Absolute(3))
        );
        // <= 0 clamps to 1
        assert_eq!(
            normalize_at(&json!(0)),
            NormalizedAt::Single(SingleAt::Absolute(1))
        );
        assert_eq!(
            normalize_at(&json!(-4)),
            NormalizedAt::Single(SingleAt::Absolute(1))
        );
        // unparseable → disabled
        assert_eq!(normalize_at(&json!("abc")), NormalizedAt::Disabled);
        assert_eq!(normalize_at(&json!({"a":1})), NormalizedAt::Disabled);
    }

    #[test]
    fn normalize_range_table() {
        // [1, 3] absolute range
        assert_eq!(
            normalize_at(&json!([1, 3])),
            NormalizedAt::Range(
                Box::new(SingleAt::Absolute(1)),
                Box::new(SingleAt::Absolute(3))
            )
        );
        // ["+1", 3] mixed
        assert_eq!(
            normalize_at(&json!(["+1", 3])),
            NormalizedAt::Range(
                Box::new(SingleAt::Relative("+1".into())),
                Box::new(SingleAt::Absolute(3))
            )
        );
        // [1, "+2"] end relative to start
        assert_eq!(
            normalize_at(&json!([1, "+2"])),
            NormalizedAt::Range(
                Box::new(SingleAt::Absolute(1)),
                Box::new(SingleAt::Relative("+2".into()))
            )
        );
        // wrong arity → disabled
        assert_eq!(normalize_at(&json!([1])), NormalizedAt::Disabled);
        assert_eq!(normalize_at(&json!([1, 2, 3])), NormalizedAt::Disabled);
    }

    // ---- compute_plan -----------------------------------------------------

    #[test]
    fn empty_plan_has_zero_total() {
        let p = plan(&[]);
        assert_eq!(p.total, 0);
        assert_eq!(p.elements.len(), 0);
        assert_eq!(p.clamp_current(5), 0);
    }

    #[test]
    fn sequential_relative_folds_offsets() {
        // Three plain v-clicks: [click1][click2][click3]
        let p = plan(&[d(0, Value::Null), d(1, Value::Null), d(2, Value::Null)]);
        assert_eq!(p.total, 3);
        let starts: Vec<i64> = p.elements.iter().map(|e| e.start).collect();
        assert_eq!(starts, [1, 2, 3]);
        // All stay visible after reveal (end = ∞)
        assert!(p.elements.iter().all(|e| e.end.is_none()));
        assert_eq!(p.elements.iter().map(|e| e.delta).collect::<Vec<_>>(), [1, 1, 1]);
    }

    #[test]
    fn absolute_does_not_advance_offset() {
        // v-click, then v-click[5] (absolute), then v-click
        let p = plan(&[
            d(0, Value::Null),      // start 1, offset → 1
            d(1, json!(5)),         // start 5, delta 0
            d(2, Value::Null),      // start 1+1 = 2, offset → 2
        ]);
        assert_eq!(p.total, 5); // max of element maxima
        assert_eq!(p.elements[1].start, 5);
        assert_eq!(p.elements[1].delta, 0);
        assert_eq!(p.elements[2].start, 2);
    }

    #[test]
    fn relative_negative_waits() {
        // v-click["+2"] then v-click["-1"]
        let p = plan(&[d(0, json!("+2")), d(1, json!("-1"))]);
        assert_eq!(p.elements[0].start, 2); // offset 0 + 2
        assert_eq!(p.elements[0].delta, 2);
        assert_eq!(p.elements[1].start, 1); // offset 2 - 1
        assert_eq!(p.total, 2);
    }

    #[test]
    fn ranges_absolute_and_relative() {
        // [3,5] absolute; then ["+1","+2"]
        let p = plan(&[d(0, json!([3, 5])), d(1, json!(["+1", "+2"]))]);
        let e0 = &p.elements[0];
        assert_eq!((e0.start, e0.end, e0.max, e0.delta), (3, Some(5), 5, 0));
        let e1 = &p.elements[1];
        // offset after e0 = 0; start = 1, end = 1 + 2 = 3
        assert_eq!((e1.start, e1.end, e1.max, e1.delta), (1, Some(3), 3, 3));
        assert_eq!(p.total, 5);
    }

    #[test]
    fn end_relative_to_start() {
        // [2, "+3"] → start 2, end 2 + 3 = 5
        let p = plan(&[d(0, json!([2, "+3"]))]);
        let e = &p.elements[0];
        assert_eq!((e.start, e.end), (2, Some(5)));
        assert_eq!(e.delta, 3);
        assert_eq!(p.total, 5);
    }

    #[test]
    fn disabled_elements_are_skipped() {
        let p = plan(&[d(0, json!(false)), d(1, Value::Null), d(2, json!("false"))]);
        assert_eq!(p.total, 1);
        assert_eq!(p.elements.len(), 1);
        assert_eq!(p.elements[0].order, 1);
    }

    #[test]
    fn size_extends_max_and_delta() {
        // size=3 element: max = start + size - 1, delta = v + size - 1
        let p = plan(&[d(0, Value::Null).with_size(3), d(1, Value::Null)]);
        assert_eq!(p.elements[0].max, 3);
        assert_eq!(p.elements[0].delta, 3);
        assert_eq!(p.elements[1].start, 4); // offset 3 + 1
        assert_eq!(p.total, 4);
    }

    #[test]
    fn override_total_wins() {
        let p = compute_plan(&[d(0, Value::Null)], Some(10), 0);
        assert_eq!(p.total, 10);
    }

    #[test]
    fn clicks_start_clamps_current() {
        let p = compute_plan(&[d(0, Value::Null)], None, 1);
        assert_eq!(p.clamp_current(0), 1);
        assert_eq!(p.clamp_current(99), 1);
    }

    #[test]
    fn is_active_semantics() {
        // Range [2,4] has delta 0, so the following relative element
        // resolves at offset 0 + 1 = 1.
        let p = plan(&[d(0, json!([2, 4])), d(1, Value::Null)]);
        let range = &p.elements[0];
        let since = &p.elements[1];
        assert_eq!(range.start, 2);
        assert_eq!(range.end, Some(4));
        assert!(!range.is_active(1));
        assert!(range.is_active(2));
        assert!(range.is_active(3));
        assert!(!range.is_active(4)); // end exclusive
        assert_eq!(since.start, 1);
        assert!(since.is_active(1));
        assert!(since.is_active(4)); // since elements stay active
        assert!(!since.is_current(2));
        assert!(since.is_current(1));

        // A range with a relative end advances the offset for successors:
        // [2, "+2"] → start 2, end 4, delta 2; next since → start 3.
        let p2 = plan(&[d(0, json!([2, "+2"])), d(1, Value::Null)]);
        assert_eq!((p2.elements[0].end, p2.elements[0].delta), (Some(4), 2));
        assert_eq!(p2.elements[1].start, 3);
        assert!(p2.elements[1].is_current(3));
    }

    // ---- wire format ------------------------------------------------------

    #[test]
    fn click_plan_serializes_camel_case() {
        let p = plan(&[d(0, Value::Null)]);
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("total").is_some());
        assert!(v.get("clicksStart").is_some());
        assert!(v.get("elements").is_some());
        let e = &v["elements"][0];
        assert!(e.get("start").is_some());
        assert!(e.get("end").is_some());
        assert!(e.get("max").is_some());
        assert!(e.get("delta").is_some());
        assert!(e.get("order").is_some());
    }
}
