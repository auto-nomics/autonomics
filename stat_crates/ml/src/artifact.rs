//! Model artifact — serialised fitted model for fit/predict decoupling.
//!
//! A [`ModelArtifact`] wraps any fitted model's parameters as opaque bincode
//! bytes, tagged with a `kind` string (e.g. `"kmeans:v1"`) for version-aware
//! deserialization. The node layer persists artifacts to opendal storage and
//! passes URIs through DAG ports as single-row `model_uri` string columns.

use serde::{Deserialize, Serialize};

/// A versioned, self-describing serialised model.
///
/// Fit-class nodes produce this; predict-class nodes consume it.
/// The `fitted` blob is opaque bincode — callers must match `kind` to the
/// expected model type before deserialising.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelArtifact {
    /// Model kind tag with version, e.g. `"kmeans:v1"`, `"random_forest:v1"`.
    pub kind: String,
    /// Bincode-serialised fitted parameters.
    #[serde(with = "serde_bytes_compat")]
    pub fitted: Vec<u8>,
    /// Feature column names the model was trained on (in order).
    pub feature_names: Vec<String>,
    /// Training metadata as JSON string (bincode-compatible).
    pub training_meta: String,
}

impl ModelArtifact {
    /// Create a new artifact, bincode-serialising the fitted model.
    pub fn new<T: Serialize>(
        kind: impl Into<String>,
        fitted: &T,
        feature_names: Vec<String>,
        training_meta: serde_json::Value,
    ) -> Result<Self, bincode::Error> {
        Ok(Self {
            kind: kind.into(),
            fitted: bincode::serialize(fitted)?,
            feature_names,
            training_meta: serde_json::to_string(&training_meta).unwrap_or_else(|_| "{}".into()),
        })
    }

    /// Deserialize the training metadata back into a `serde_json::Value`.
    pub fn training_meta_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.training_meta).unwrap_or(serde_json::json!({}))
    }

    /// Deserialize the fitted parameters back into `T`.
    ///
    /// Caller is responsible for ensuring `T` matches `self.kind`.
    pub fn deserialize_fitted<T: for<'de> Deserialize<'de>>(&self) -> Result<T, bincode::Error> {
        bincode::deserialize(&self.fitted)
    }

    /// Serialize the entire artifact to bincode bytes for storage.
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// Deserialize an artifact from bincode bytes loaded from storage.
    pub fn from_bytes(data: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(data)
    }
}

/// Serde adapter for `Vec<u8>` that serialises as a byte array rather than
/// a sequence of integers. Uses `serde_bytes` semantics without the extra
/// dependency.
mod serde_bytes_compat {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        bytes.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        Vec::<u8>::deserialize(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct DummyModel {
        centroids: Vec<Vec<f64>>,
        k: usize,
    }

    #[test]
    fn roundtrip_artifact() {
        let model = DummyModel {
            centroids: vec![vec![1.0, 2.0], vec![3.0, 4.0]],
            k: 2,
        };
        let artifact = ModelArtifact::new(
            "kmeans:v1",
            &model,
            vec!["x".into(), "y".into()],
            serde_json::json!({"n_samples": 100, "inertia": 42.0}),
        )
        .unwrap();

        let bytes = artifact.to_bytes().unwrap();
        let restored = ModelArtifact::from_bytes(&bytes).unwrap();
        assert_eq!(restored.kind, "kmeans:v1");
        assert_eq!(restored.feature_names, vec!["x", "y"]);

        let model2: DummyModel = restored.deserialize_fitted().unwrap();
        assert_eq!(model, model2);
    }
}
