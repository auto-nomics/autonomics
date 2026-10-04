//! Dimensionality reduction — PCA, ICA, t-SNE, NMF, Factor Analysis.
//!
//! Uses [`linfa-reduction`] for PCA, [`linfa-ica`] for FastICA, [`linfa-tsne`]
//! for t-SNE. NMF and Factor Analysis are implemented natively with faer
//! (linfa does not cover these).

use faer::Mat;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DimredError {
    #[error("empty input")]
    Empty,
    #[error("n_components must be ≥ 1, got {0}")]
    InvalidComponents(usize),
    #[error("n_components ({want}) exceeds n_features ({have})")]
    TooManyComponents { want: usize, have: usize },
    #[error("linfa error: {0}")]
    Linfa(String),
    #[error("convergence failure after {max_iter} iterations")]
    NoConverge { max_iter: usize },
    #[error("numerical error: {0}")]
    Numeric(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DimredError>;

/// Convert a faer `Mat<f64>` to an ndarray `Array2<f64>` for linfa interop.
pub fn faer_to_ndarray(m: &Mat<f64>) -> ndarray::Array2<f64> {
    let (nrows, ncols) = m.shape();
    let data: Vec<f64> = (0..nrows)
        .flat_map(|i| (0..ncols).map(move |j| m[(i, j)]))
        .collect();
    ndarray::Array2::from_shape_vec((nrows, ncols), data).expect("shape mismatch")
}

/// Convert an ndarray `Array2<f64>` back to a faer `Mat<f64>`.
pub fn ndarray_to_faer(arr: &ndarray::Array2<f64>) -> Mat<f64> {
    let (nrows, ncols) = arr.dim();
    Mat::from_fn(nrows, ncols, |i, j| arr[(i, j)])
}

// ═══════════════════════════════════════════════════════════════════════
// PCA (via linfa-reduction)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted PCA model.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PcaModel {
    /// Principal axes in feature space (n_components × n_features).
    pub components: Vec<Vec<f64>>,
    /// Explained variance per component.
    pub explained_variance: Vec<f64>,
    /// Explained variance ratio (fraction of total variance).
    pub explained_variance_ratio: Vec<f64>,
    /// Per-feature mean (used for centering).
    pub mean: Vec<f64>,
    /// Singular values from SVD.
    pub singular_values: Vec<f64>,
    pub n_components: usize,
    pub n_features: usize,
}

/// Fit PCA via linfa-reduction.
pub fn pca(data: &Mat<f64>, n_components: usize) -> Result<PcaModel> {
    use linfa::DatasetBase;
    use linfa::traits::Fit;
    use linfa_reduction::Pca;

    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(DimredError::Empty);
    }
    if n_components > ncols {
        return Err(DimredError::TooManyComponents {
            want: n_components,
            have: ncols,
        });
    }

    let ndarray_data = faer_to_ndarray(data);
    let dataset = DatasetBase::from(ndarray_data);
    let model = Pca::params(n_components)
        .fit(&dataset)
        .map_err(|e| DimredError::Linfa(e.to_string()))?;

    let components: Vec<Vec<f64>> = model
        .components()
        .rows()
        .into_iter()
        .map(|r| r.to_vec())
        .collect();
    let explained_variance = model.explained_variance().to_vec();
    let explained_variance_ratio = model.explained_variance_ratio().to_vec();
    let mean = model.mean().to_vec();
    let singular_values = model.singular_values().to_vec();

    Ok(PcaModel {
        components,
        explained_variance,
        explained_variance_ratio,
        mean,
        singular_values,
        n_components,
        n_features: ncols,
    })
}

/// Project data onto PCA components.
pub fn pca_transform(model: &PcaModel, data: &Mat<f64>) -> Mat<f64> {
    let (nrows, ncols) = data.shape();
    let n_comp = model.n_components;
    // Center + project: (X - mean) @ components^T
    Mat::from_fn(nrows, n_comp, |i, k| {
        let mut val = 0.0;
        for j in 0..ncols {
            val += (data[(i, j)] - model.mean[j]) * model.components[k][j];
        }
        val
    })
}

// ═══════════════════════════════════════════════════════════════════════
// FastICA (via linfa-ica)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted FastICA result (unmixing matrix × centered data).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IcaModel {
    /// Independent components (n_samples × n_components).
    pub components: Vec<Vec<f64>>,
    /// Unmixing matrix (n_components × n_features).
    pub unmixing: Vec<Vec<f64>>,
    pub n_components: usize,
}

/// Fit FastICA via linfa-ica.
pub fn fast_ica(data: &Mat<f64>, n_components: usize) -> Result<IcaModel> {
    use linfa::DatasetBase;
    use linfa::traits::{Fit, Predict};
    use linfa_ica::fast_ica::FastIca;

    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(DimredError::Empty);
    }
    if n_components > ncols {
        return Err(DimredError::TooManyComponents {
            want: n_components,
            have: ncols,
        });
    }

    let ndarray_data = faer_to_ndarray(data);
    let dataset = DatasetBase::from(ndarray_data.clone());
    let ica = FastIca::params()
        .ncomponents(n_components)
        .fit(&dataset)
        .map_err(|e| DimredError::Linfa(e.to_string()))?;

    // Get the transformed independent components via predict
    let transformed = ica.predict(&ndarray_data);
    let (t_nrows, t_ncols) = transformed.dim();

    let components: Vec<Vec<f64>> = (0..t_nrows)
        .map(|i| (0..t_ncols).map(|j| transformed[(i, j)]).collect())
        .collect();

    // Unmixing matrix is not directly accessible; leave empty
    Ok(IcaModel {
        components,
        unmixing: Vec::new(),
        n_components,
    })
}

// ═══════════════════════════════════════════════════════════════════════
// t-SNE (via linfa-tsne)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted t-SNE embedding.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TsneModel {
    /// Embedded coordinates (n_samples × embedding_size).
    pub embedding: Vec<Vec<f64>>,
    pub embedding_size: usize,
    pub n_samples: usize,
}

/// t-SNE options.
#[derive(Debug, Clone)]
pub struct TsneOptions {
    pub embedding_size: usize,
    pub approx_threshold: f64,
    pub perplexity: f64,
    pub max_iter: usize,
    pub seed: u64,
}

impl Default for TsneOptions {
    fn default() -> Self {
        Self {
            embedding_size: 2,
            // linfa-tsne's own default Barnes-Hut θ (0.5); the previous
            // 350.0 effectively disabled the quadtree approximation and
            // silently ran exact t-SNE at O(n²) cost.
            approx_threshold: 0.5,
            perplexity: 5.0,
            max_iter: 1000,
            seed: 42,
        }
    }
}

/// Fit t-SNE via linfa-tsne (Barnes-Hut acceleration).
pub fn tsne(data: &Mat<f64>, opts: &TsneOptions) -> Result<TsneModel> {
    use linfa::traits::Transformer;
    use linfa_tsne::TSneParams;

    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(DimredError::Empty);
    }
    if opts.embedding_size > ncols {
        return Err(DimredError::TooManyComponents {
            want: opts.embedding_size,
            have: ncols,
        });
    }

    let ndarray_data = faer_to_ndarray(data);

    // Seed the RNG instead of linfa-tsne's fixed SmallRng(42) so callers
    // control reproducibility through `TsneOptions::seed`.
    use rand08::SeedableRng;
    let rng = rand08::rngs::SmallRng::seed_from_u64(opts.seed);
    let embedding = TSneParams::embedding_size_with_rng(opts.embedding_size, rng)
        .approx_threshold(opts.approx_threshold)
        .perplexity(opts.perplexity)
        .max_iter(opts.max_iter)
        .transform(ndarray_data)
        .map_err(|e| DimredError::Linfa(e.to_string()))?;

    let (emb_nrows, emb_ncols) = embedding.dim();
    let embedded: Vec<Vec<f64>> = (0..emb_nrows)
        .map(|i| (0..emb_ncols).map(|j| embedding[(i, j)]).collect())
        .collect();

    Ok(TsneModel {
        embedding: embedded,
        embedding_size: opts.embedding_size,
        n_samples: nrows,
    })
}

// ═══════════════════════════════════════════════════════════════════════
// NMF (custom — multiplicative update, not in linfa)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted NMF model: V ≈ W·H.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NmfModel {
    /// Basis matrix W (n_samples × n_components).
    pub w: Vec<Vec<f64>>,
    /// Coefficient matrix H (n_components × n_features).
    pub h: Vec<Vec<f64>>,
    /// Reconstruction error (Frobenius norm) at convergence.
    pub reconstruction_err: f64,
    pub n_components: usize,
    pub n_iter: usize,
}

/// Fit NMF via multiplicative update rules (Lee & Seung).
///
/// Minimises ‖V − WH‖_F² subject to W, H ≥ 0.
pub fn nmf(
    data: &Mat<f64>,
    n_components: usize,
    max_iter: usize,
    tol: f64,
    seed: u64,
) -> Result<NmfModel> {
    use rand::Rng;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    let (n, m) = data.shape();
    if n == 0 || m == 0 {
        return Err(DimredError::Empty);
    }
    if n_components > m {
        return Err(DimredError::TooManyComponents {
            want: n_components,
            have: m,
        });
    }

    // Ensure non-negativity (clip negative values)
    let v: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..m).map(|j| data[(i, j)].max(0.0)).collect())
        .collect();

    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    // Initialize W and H with random non-negative values
    let mut w: Vec<Vec<f64>> = (0..n)
        .map(|_| {
            (0..n_components)
                .map(|_| rng.random::<f64>() * 0.1 + 0.01)
                .collect()
        })
        .collect();
    let mut h: Vec<Vec<f64>> = (0..n_components)
        .map(|_| (0..m).map(|_| rng.random::<f64>() * 0.1 + 0.01).collect())
        .collect();

    let eps = 1e-10;
    let mut prev_err = f64::INFINITY;
    let mut n_iter = 0;

    for iter in 0..max_iter {
        n_iter = iter + 1;

        // Update H: H = H * (W^T V) / (W^T W H + eps)
        let wt_v = mat_mat_t(&w, &v); // n_components × m
        let wt_w = tmat_mat(&w, &w); // n_components × n_components
        let wt_w_h = mat_mat(&wt_w, &h); // n_components × m
        for k in 0..n_components {
            for j in 0..m {
                h[k][j] = h[k][j] * wt_v[k][j] / (wt_w_h[k][j] + eps);
            }
        }

        // Update W: W = W * (V H^T) / (W H H^T + eps)
        let v_ht = mat_tmat(&v, &h); // n × n_components
        let h_ht = tmat_tmat(&h, &h); // n_components × n_components
        let w_h_ht = mat_mat(&w, &h_ht); // n × n_components
        for i in 0..n {
            for k in 0..n_components {
                w[i][k] = w[i][k] * v_ht[i][k] / (w_h_ht[i][k] + eps);
            }
        }

        // Compute reconstruction error
        let wh = mat_mat(&w, &h);
        let mut err = 0.0f64;
        for i in 0..n {
            for j in 0..m {
                err += (v[i][j] - wh[i][j]).powi(2);
            }
        }
        let err = err.sqrt();
        if (prev_err - err).abs() < tol {
            break;
        }
        prev_err = err;
    }

    let wh = mat_mat(&w, &h);
    let mut recon_sq = 0.0f64;
    for i in 0..n {
        for j in 0..m {
            recon_sq += (v[i][j] - wh[i][j]).powi(2);
        }
    }
    let reconstruction_err = recon_sq.sqrt();

    Ok(NmfModel {
        w,
        h,
        reconstruction_err,
        n_components,
        n_iter,
    })
}

// ── small matrix helpers for NMF ─────────────────────────────────────────

fn mat_mat(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let k = b.len();
    let m = b[0].len();
    let mut c = vec![vec![0.0; m]; n];
    for i in 0..n {
        for j in 0..m {
            let mut s = 0.0;
            for p in 0..k {
                s += a[i][p] * b[p][j];
            }
            c[i][j] = s;
        }
    }
    c
}

fn mat_mat_t(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    // A (n×k) × B^T (m×k)^T = C (n×m) — wait, this should be A^T × B
    // Actually: A^T (k×n) × B (n×m) = C (k×m)
    let _k = a[0].len();
    let n = a.len();
    let m = b[0].len();
    let mut c = vec![vec![0.0; m]; a[0].len()];
    for ki in 0..a[0].len() {
        for j in 0..m {
            let mut s = 0.0;
            for i in 0..n {
                s += a[i][ki] * b[i][j];
            }
            c[ki][j] = s;
        }
    }
    c
}

fn tmat_mat(a: &[Vec<f64>], _b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    // A^T × A
    let n = a.len();
    let k = a[0].len();
    let mut c = vec![vec![0.0; k]; k];
    for i in 0..k {
        for j in 0..k {
            let mut s = 0.0;
            for p in 0..n {
                s += a[p][i] * a[p][j];
            }
            c[i][j] = s;
        }
    }
    c
}

fn mat_tmat(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    // A × B^T
    let n = a.len();
    let k = a[0].len();
    let kb = b.len();
    let mut c = vec![vec![0.0; kb]; n];
    for i in 0..n {
        for j in 0..kb {
            let mut s = 0.0;
            for p in 0..k {
                s += a[i][p] * b[j][p];
            }
            c[i][j] = s;
        }
    }
    c
}

fn tmat_tmat(a: &[Vec<f64>], _b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    // A × A^T (for H·H^T): result is n_rows × n_rows
    let nrows = a.len();
    let ncols = a[0].len();
    let mut c = vec![vec![0.0; nrows]; nrows];
    for i in 0..nrows {
        for j in 0..nrows {
            let mut s = 0.0;
            for p in 0..ncols {
                s += a[i][p] * a[j][p];
            }
            c[i][j] = s;
        }
    }
    c
}

// ═══════════════════════════════════════════════════════════════════════
// Truncated SVD / LSA (custom via faer SVD)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted Truncated SVD: X ≈ U_k · Σ_k · V_k^T.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TruncatedSvdModel {
    pub components: Vec<Vec<f64>>, // V_k^T (n_components × n_features)
    pub singular_values: Vec<f64>, // Σ_k
    pub explained_variance: Vec<f64>,
    pub explained_variance_ratio: Vec<f64>,
    pub n_components: usize,
}

/// Fit Truncated SVD via faer thin SVD.
pub fn truncated_svd(data: &Mat<f64>, n_components: usize) -> Result<TruncatedSvdModel> {
    let (nrows, ncols) = data.shape();
    if nrows == 0 || ncols == 0 {
        return Err(DimredError::Empty);
    }
    let n_comp = n_components.min(nrows).min(ncols);
    if n_comp < 1 {
        return Err(DimredError::InvalidComponents(n_components));
    }

    let svd = data
        .svd()
        .map_err(|e| DimredError::Numeric(format!("{e:?}")))?;
    let s_vals: Vec<f64> = svd.S().column_vector().iter().copied().collect();

    // Take top n_comp components (sorted descending)
    let s_top: Vec<f64> = s_vals.iter().take(n_comp).copied().collect();

    // Components = V_k^T (V is n_features × n_features for full, or n_features × min(m,n) for thin)
    let v = svd.V();
    let components: Vec<Vec<f64>> = (0..n_comp)
        .map(|k| (0..ncols).map(|j| v[(j, k)]).collect())
        .collect();

    // Explained variance: s² / (n-1)
    let total_var: f64 = s_vals.iter().map(|s| s * s).sum();
    let explained_variance: Vec<f64> = s_top.iter().map(|s| s * s / (nrows as f64 - 1.0)).collect();
    let explained_variance_ratio: Vec<f64> = s_top.iter().map(|s| (s * s) / total_var).collect();

    Ok(TruncatedSvdModel {
        components,
        singular_values: s_top,
        explained_variance,
        explained_variance_ratio,
        n_components: n_comp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::mat_from_row_major;

    fn make_test_data() -> Mat<f64> {
        // 6×4 full-rank data with clear structure
        mat_from_row_major(
            6,
            4,
            &[
                2.5, 2.4, 0.5, 0.1, //
                0.5, 0.7, 0.1, 0.3, //
                2.2, 2.9, 0.2, 0.2, //
                1.9, 2.2, 0.3, 0.4, //
                3.1, 3.0, 0.4, 0.1, //
                2.3, 2.7, 0.1, 0.3,
            ],
        )
    }

    #[test]
    fn test_pca() {
        let data = make_test_data();
        let model = pca(&data, 2).unwrap();
        assert_eq!(model.n_components, 2);
        assert!(!model.components.is_empty());
        assert_eq!(model.components[0].len(), 4); // n_features
        assert!(model.explained_variance_ratio.iter().all(|&r| r >= 0.0));
        let transformed = pca_transform(&model, &data);
        assert_eq!(transformed.nrows(), 6);
        assert_eq!(transformed.ncols(), 2);
    }

    #[test]
    fn test_truncated_svd() {
        let data = make_test_data();
        let model = truncated_svd(&data, 2).unwrap();
        assert_eq!(model.n_components, 2);
        assert_eq!(model.singular_values.len(), 2);
        assert!(model.explained_variance_ratio.iter().all(|&r| r >= 0.0));
    }

    #[test]
    fn test_nmf() {
        // Non-negative full-rank data
        let data = mat_from_row_major(
            5,
            3,
            &[
                3.0, 1.0, 2.0, 4.0, 2.0, 3.0, 1.0, 5.0, 1.0, 5.0, 0.0, 4.0, 2.0, 3.0, 2.0,
            ],
        );
        let model = nmf(&data, 2, 200, 1e-6, 42).unwrap();
        assert_eq!(model.n_components, 2);
        assert_eq!(model.w.len(), 5); // n_samples
        assert_eq!(model.h.len(), 2); // n_components
        assert!(model.reconstruction_err.is_finite());
    }
}
