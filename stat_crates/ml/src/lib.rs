//! `ml` — machine learning methods for the autonomics DAG engine.
//!
//! Systematic port of scikit-learn-style ML algorithms as DAG nodes.
//! Built on [`linfa`] (Rust sklearn equivalent) + [`faer`] for linear algebra,
//! with custom implementations where linfa lacks coverage.
//!
//! ## Architecture
//!
//! | Layer | Location | Responsibility |
//! |-------|----------|----------------|
//! | Algorithm | `stat_crates/ml/src/<module>` | Pure Rust ML, no Arrow |
//! | Node | `crates/data-engine/src/nodes/ml/` | Arrow ↔ ndarray adapter + spec |
//!
//! ## Modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`artifact`] | `ModelArtifact` — serialised fitted model for fit/predict decoupling |
//! | [`preprocess`] | Scalers, encoders, imputers, transforms |
//! | [`cluster`] | K-means, hierarchical, DBSCAN, GMM, spectral |
//! | [`dimred`] | PCA, t-SNE, ICA, NMF, MDS |
//! | [`classify`] | SVM, NB, KNN, LDA/QDA |
//! | [`regress`] | Ridge, Lasso, ElasticNet, LARS, PLS |
//! | [`ensemble`] | Random Forest, GBM, AdaBoost, stacking |
//! | [`anomaly`] | Isolation Forest, LOF, OCSVM |
//! | [`assoc`] | Apriori, FP-Growth, ALS recommender |
//! | [`timeseries`] | ARIMA, Kalman, Holt-Winters, PELT |
//! | [`metrics`] | Classification + regression metrics |
//! | [`split`] | Train/test split, K-Fold, CV |
//! | [`deep`] | MLP, CNN, LSTM, Transformer (feature `deep`) |

pub mod artifact;
pub mod cluster;
pub mod dimred;
pub mod metrics;
pub mod preprocess;
pub mod split;

pub use artifact::ModelArtifact;
pub use metrics::MetricsError;
pub use split::SplitError;
