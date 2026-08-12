//! DLModelArtifact — versioned, self-describing serialised DL model.
//!
//! Wraps a trained model's parameters as JSON bytes, tagged with architecture
//! type, task type, and training metadata.  This is the unit that flows
//! through DAG ports between training and prediction nodes.

use serde::{Deserialize, Serialize};

/// Architecture identifier for a DL model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    Mlp,
    Transformer,
    Deepsurv,
    Deephit,
    TransformerSurvival,
    Autoencoder,
    Rnn,
}

impl Architecture {
    pub fn as_str(&self) -> &'static str {
        match self {
            Architecture::Mlp => "mlp",
            Architecture::Transformer => "transformer",
            Architecture::Deepsurv => "deepsurv",
            Architecture::Deephit => "deephit",
            Architecture::TransformerSurvival => "transformer_survival",
            Architecture::Autoencoder => "autoencoder",
            Architecture::Rnn => "rnn",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "mlp" => Some(Architecture::Mlp),
            "transformer" => Some(Architecture::Transformer),
            "deepsurv" => Some(Architecture::Deepsurv),
            "deephit" => Some(Architecture::Deephit),
            "transformer_survival" => Some(Architecture::TransformerSurvival),
            "autoencoder" => Some(Architecture::Autoencoder),
            "rnn" => Some(Architecture::Rnn),
            _ => None,
        }
    }
}

/// Task type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactTaskType {
    Classification,
    Regression,
    Survival,
    Reconstruction,
}

/// A versioned, self-describing serialised deep learning model.
///
/// Training nodes produce this; prediction nodes consume it.
/// The `checkpoint_json` field contains the full model state (architecture
/// config + weights) serialised as JSON, so no external framework is needed
/// to reconstruct the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DLModelArtifact {
    /// Backend that produced this model (always "faer" for now).
    pub backend: String,
    /// Architecture identifier.
    pub architecture: Architecture,
    /// Task type (classification, regression, survival, reconstruction).
    pub task_type: ArtifactTaskType,
    /// Full model state as JSON (architecture config + weights).
    pub checkpoint_json: String,
    /// Feature column names in training order.
    pub feature_names: Vec<String>,
    /// Label/time/event column names.
    pub label_column: Option<String>,
    pub time_column: Option<String>,
    pub event_column: Option<String>,
    /// Scaler state as JSON (if features were standardised).
    pub scaler_json: Option<String>,
    /// Training metadata.
    pub training_meta: TrainingMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrainingMeta {
    pub n_epochs_run: usize,
    pub best_epoch: Option<usize>,
    pub best_val_metric: Option<f64>,
    pub total_params: usize,
}

impl DLModelArtifact {
    /// Serialise to bytes for storage.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialise from bytes loaded from storage.
    pub fn from_bytes(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_roundtrip() {
        let artifact = DLModelArtifact {
            backend: "faer".into(),
            architecture: Architecture::Mlp,
            task_type: ArtifactTaskType::Classification,
            checkpoint_json: r#"{"layers":[]}"#.into(),
            feature_names: vec!["age".into(), "sbp".into()],
            label_column: Some("cvd".into()),
            time_column: None,
            event_column: None,
            scaler_json: Some(r#"{"mean":[1.0],"std":[2.0]}"#.into()),
            training_meta: TrainingMeta {
                n_epochs_run: 100,
                best_epoch: Some(42),
                best_val_metric: Some(0.85),
                total_params: 1234,
            },
        };
        let bytes = artifact.to_bytes().unwrap();
        let restored = DLModelArtifact::from_bytes(&bytes).unwrap();
        assert_eq!(restored.backend, "faer");
        assert_eq!(restored.architecture, Architecture::Mlp);
        assert_eq!(restored.feature_names, vec!["age", "sbp"]);
        assert_eq!(restored.training_meta.total_params, 1234);
    }
}
