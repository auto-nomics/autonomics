//! Target–disease association types and pagination helpers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{Disease, ScoredComponent, Target};

/// A single target↔disease association as scored by Open Targets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssociatedDisease {
    /// Overall association score in `[0, 1]`.
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub novelty: Option<f64>,
    #[serde(default)]
    pub disease: Disease,
    #[serde(default)]
    pub datasource_scores: Vec<ScoredComponent>,
    #[serde(default)]
    pub datatype_scores: Vec<ScoredComponent>,
}

/// Mirror of [`AssociatedDisease`] for the disease→target direction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssociatedTarget {
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub novelty: Option<f64>,
    #[serde(default)]
    pub target: Target,
    #[serde(default)]
    pub datasource_scores: Vec<ScoredComponent>,
    #[serde(default)]
    pub datatype_scores: Vec<ScoredComponent>,
}

/// The `{ count, rows }` payload returned by `associatedDiseases` /
/// `associatedTargets`. `datasources` is left untyped.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssociationPage<T> {
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub rows: Vec<T>,
    #[serde(default)]
    pub datasources: Vec<Value>,
}

/// Zero-based page request mirroring the GraphQL `Pagination` input.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pagination {
    pub index: u32,
    pub size: u32,
}

impl Pagination {
    /// Create a page with the given 0-based index and size.
    pub const fn new(index: u32, size: u32) -> Self {
        Self { index, size }
    }
}
