//! Hierarchical agent path — a validated, slash-delimited identifier for
//! agents in the multi-agent topology.
//!
//! Inspired by Codex's `AgentPath`. All paths start with `/root`; child
//! segments are lowercase `[a-z0-9_]`, e.g. `/root/researcher/worker`.

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

const ROOT_STR: &str = "/root";
const ROOT_SEGMENT: &str = "root";
const MAX_SEGMENT_LEN: usize = 32;

/// Errors that can arise when constructing or resolving an [`AgentPath`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PathError {
    #[error("agent path must not be empty")]
    Empty,
    #[error("agent path must start with `/root`, got `{0}`")]
    InvalidRoot(String),
    #[error(
        "agent name segment `{segment}` must use only lowercase letters, digits, and underscores"
    )]
    InvalidSegmentChar { segment: String },
    #[error("agent name segment `{0}` is reserved")]
    ReservedSegment(String),
    #[error("agent name segment must not be empty")]
    EmptySegment,
    #[error("agent name segment `{0}` exceeds {MAX_SEGMENT_LEN} characters")]
    SegmentTooLong(String),
    #[error("agent path must not end with `/`")]
    TrailingSlash,
}

/// A hierarchical agent path, e.g. `/root/researcher/worker`.
///
/// All paths start with `/root`. Each segment after `root` is validated to
/// contain only `[a-z0-9_]`, be 1–32 chars, and not be `.` or `..`.
///
/// Use [`AgentPath::root`] for the root agent, [`AgentPath::join`] to append
/// a child segment, and [`AgentPath::resolve`] to resolve a relative or
/// absolute reference against this path.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentPath(String);

impl AgentPath {
    /// The root agent path: `/root`.
    pub fn root() -> Self {
        Self(ROOT_STR.to_string())
    }

    /// Returns `true` if this is the root path (`/root`).
    pub fn is_root(&self) -> bool {
        self.0 == ROOT_STR
    }

    /// The full path as a string slice, e.g. `/root/researcher/worker`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment of the path — the agent's short name.
    /// Returns `"root"` for the root path.
    pub fn name(&self) -> &str {
        if self.is_root() {
            return ROOT_SEGMENT;
        }
        self.0
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(ROOT_SEGMENT)
    }

    /// The parent path, or `None` if this is root.
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        let idx = self.0.rfind('/')?;
        // Don't strip below `/root`.
        let parent = if idx == 0 { ROOT_STR } else { &self.0[..idx] };
        Some(Self(parent.to_string()))
    }

    /// All segments including `root`, e.g. `["root", "researcher", "worker"]`.
    pub fn segments(&self) -> Vec<&str> {
        self.0.split('/').filter(|s| !s.is_empty()).collect()
    }

    /// Append a validated segment to this path.
    ///
    /// ```
    /// # use agentik_types::AgentPath;
    /// let child = AgentPath::root().join("researcher").unwrap();
    /// assert_eq!(child.as_str(), "/root/researcher");
    /// ```
    pub fn join(&self, segment: &str) -> Result<Self, PathError> {
        validate_segment(segment)?;
        Ok(Self(format!("{}/{}", self.0, segment)))
    }

    /// Resolve an input string relative to this path.
    ///
    /// - If `input` starts with `/root`, it is treated as an absolute path.
    /// - Otherwise, it is treated as a relative reference (one or more `/`-
    ///   separated segments) and appended to this path.
    ///
    /// ```
    /// # use agentik_types::AgentPath;
    /// let me = AgentPath::try_from("/root/researcher").unwrap();
    /// assert_eq!(me.resolve("worker").unwrap().as_str(), "/root/researcher/worker");
    /// assert_eq!(me.resolve("/root/writer").unwrap().as_str(), "/root/writer");
    /// ```
    pub fn resolve(&self, input: &str) -> Result<Self, PathError> {
        if input.is_empty() {
            return Err(PathError::Empty);
        }
        if input == ROOT_STR {
            return Ok(Self::root());
        }
        if input.starts_with('/') {
            return Self::try_from(input);
        }
        // Relative: validate each segment, then append.
        for segment in input.split('/') {
            validate_segment(segment)?;
        }
        Ok(Self(format!("{}/{}", self.0, input)))
    }

    /// Validate that a string is a well-formed absolute agent path.
    pub fn validate(path: &str) -> Result<(), PathError> {
        if path.is_empty() {
            return Err(PathError::Empty);
        }
        if path == ROOT_STR {
            return Ok(());
        }
        let Some(stripped) = path.strip_prefix('/') else {
            return Err(PathError::InvalidRoot(path.to_string()));
        };
        let mut segments = stripped.split('/');
        let first = segments.next().unwrap_or("");
        if first != ROOT_SEGMENT {
            return Err(PathError::InvalidRoot(path.to_string()));
        }
        if path.ends_with('/') {
            return Err(PathError::TrailingSlash);
        }
        for seg in segments {
            validate_segment(seg)?;
        }
        Ok(())
    }
}

impl TryFrom<String> for AgentPath {
    type Error = PathError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::validate(&value)?;
        Ok(Self(value))
    }
}

impl TryFrom<&str> for AgentPath {
    type Error = PathError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::validate(value)?;
        Ok(Self(value.to_string()))
    }
}

impl FromStr for AgentPath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl From<AgentPath> for String {
    fn from(path: AgentPath) -> Self {
        path.0
    }
}

impl AsRef<str> for AgentPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ── Segment validation ───────────────────────────────────────────

pub fn validate_segment(segment: &str) -> Result<(), PathError> {
    if segment.is_empty() {
        return Err(PathError::EmptySegment);
    }
    if segment == ROOT_SEGMENT {
        return Err(PathError::ReservedSegment(segment.to_string()));
    }
    if segment == "." || segment == ".." {
        return Err(PathError::ReservedSegment(segment.to_string()));
    }
    if segment.len() > MAX_SEGMENT_LEN {
        return Err(PathError::SegmentTooLong(segment.to_string()));
    }
    if !segment
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        return Err(PathError::InvalidSegmentChar {
            segment: segment.to_string(),
        });
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ──

    #[test]
    fn root() {
        let root = AgentPath::root();
        assert_eq!(root.as_str(), "/root");
        assert!(root.is_root());
        assert_eq!(root.name(), "root");
    }

    #[test]
    fn try_from_valid() {
        let path = AgentPath::try_from("/root/researcher").unwrap();
        assert_eq!(path.as_str(), "/root/researcher");
        assert!(!path.is_root());
        assert_eq!(path.name(), "researcher");
    }

    #[test]
    fn try_from_deeply_nested() {
        let path = AgentPath::try_from("/root/a/b/c_d/e2").unwrap();
        assert_eq!(path.segments(), vec!["root", "a", "b", "c_d", "e2"]);
        assert_eq!(path.name(), "e2");
    }

    // ── Validation ──

    #[test]
    fn reject_empty() {
        assert_eq!(AgentPath::try_from(""), Err(PathError::Empty));
    }

    #[test]
    fn reject_no_leading_slash() {
        assert!(matches!(
            AgentPath::try_from("root/researcher"),
            Err(PathError::InvalidRoot(_))
        ));
    }

    #[test]
    fn reject_wrong_root() {
        assert!(matches!(
            AgentPath::try_from("/home/user"),
            Err(PathError::InvalidRoot(_))
        ));
    }

    #[test]
    fn reject_uppercase() {
        assert!(matches!(
            AgentPath::try_from("/root/Researcher"),
            Err(PathError::InvalidSegmentChar { .. })
        ));
    }

    #[test]
    fn reject_dot_segment() {
        assert!(matches!(
            AgentPath::try_from("/root/./child"),
            Err(PathError::ReservedSegment(_))
        ));
    }

    #[test]
    fn reject_dotdot_segment() {
        assert!(matches!(
            AgentPath::try_from("/root/.."),
            Err(PathError::ReservedSegment(_))
        ));
    }

    #[test]
    fn reject_reserved_root_segment() {
        assert_eq!(
            AgentPath::root().join("root"),
            Err(PathError::ReservedSegment("root".to_string()))
        );
    }

    #[test]
    fn reject_hyphen() {
        assert!(matches!(
            AgentPath::try_from("/root/my-agent"),
            Err(PathError::InvalidSegmentChar { .. })
        ));
    }

    #[test]
    fn reject_trailing_slash() {
        assert_eq!(
            AgentPath::try_from("/root/researcher/"),
            Err(PathError::TrailingSlash)
        );
    }

    #[test]
    fn reject_segment_too_long() {
        let long = "a".repeat(33);
        assert!(matches!(
            AgentPath::root().join(&long),
            Err(PathError::SegmentTooLong(_))
        ));
    }

    #[test]
    fn accept_max_segment_len() {
        let max = "a".repeat(32);
        assert!(AgentPath::root().join(&max).is_ok());
    }

    #[test]
    fn accept_underscore_and_digits() {
        assert!(AgentPath::try_from("/root/agent_2_worker").is_ok());
    }

    // ── Join ──

    #[test]
    fn join_creates_child() {
        let child = AgentPath::root().join("researcher").unwrap();
        assert_eq!(child.as_str(), "/root/researcher");
    }

    #[test]
    fn join_nested() {
        let parent = AgentPath::try_from("/root/researcher").unwrap();
        let child = parent.join("worker").unwrap();
        assert_eq!(child.as_str(), "/root/researcher/worker");
    }

    #[test]
    fn join_rejects_invalid() {
        assert!(AgentPath::root().join("Bad Name").is_err());
        assert!(AgentPath::root().join("a/b").is_err());
    }

    // ── Resolve ──

    #[test]
    fn resolve_absolute() {
        let me = AgentPath::try_from("/root/researcher").unwrap();
        let resolved = me.resolve("/root/writer").unwrap();
        assert_eq!(resolved.as_str(), "/root/writer");
    }

    #[test]
    fn resolve_relative() {
        let me = AgentPath::try_from("/root/researcher").unwrap();
        let resolved = me.resolve("worker").unwrap();
        assert_eq!(resolved.as_str(), "/root/researcher/worker");
    }

    #[test]
    fn resolve_multi_segment_relative() {
        let me = AgentPath::try_from("/root/researcher").unwrap();
        let resolved = me.resolve("sub/deep").unwrap();
        assert_eq!(resolved.as_str(), "/root/researcher/sub/deep");
    }

    #[test]
    fn resolve_root_keyword() {
        let me = AgentPath::try_from("/root/researcher").unwrap();
        assert_eq!(me.resolve("/root").unwrap(), AgentPath::root());
    }

    #[test]
    fn resolve_empty_rejected() {
        let me = AgentPath::try_from("/root/researcher").unwrap();
        assert_eq!(me.resolve(""), Err(PathError::Empty));
    }

    // ── Parent ──

    #[test]
    fn parent_of_root_is_none() {
        assert!(AgentPath::root().parent().is_none());
    }

    #[test]
    fn parent_of_child() {
        let child = AgentPath::try_from("/root/researcher/worker").unwrap();
        assert_eq!(child.parent().unwrap().as_str(), "/root/researcher");
    }

    #[test]
    fn parent_of_direct_child() {
        let child = AgentPath::try_from("/root/researcher").unwrap();
        assert_eq!(child.parent().unwrap(), AgentPath::root());
    }

    // ── Display / serde ──

    #[test]
    fn display() {
        let path = AgentPath::try_from("/root/researcher").unwrap();
        assert_eq!(format!("{path}"), "/root/researcher");
    }

    #[test]
    fn serde_round_trip() {
        let path = AgentPath::try_from("/root/researcher/worker").unwrap();
        let json = serde_json::to_string(&path).unwrap();
        assert_eq!(json, "\"/root/researcher/worker\"");
        let back: AgentPath = serde_json::from_str(&json).unwrap();
        assert_eq!(path, back);
    }

    #[test]
    fn serde_root() {
        let json = serde_json::to_string(&AgentPath::root()).unwrap();
        assert_eq!(json, "\"/root\"");
    }
}
