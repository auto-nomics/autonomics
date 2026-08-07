//! Clustering algorithms — K-means, DBSCAN, Gaussian mixture, hierarchical.
//!
//! K-means and GMM are implemented natively in faer for full control.
//! DBSCAN and hierarchical will wrap linfa once API is stabilised; for now
//! they return `NotImplemented`.

use faer::Mat;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClusterError {
    #[error("empty input")]
    Empty,
    #[error("k must be ≥ 1, got {0}")]
    InvalidK(usize),
    #[error("not enough samples ({n}) for {k} clusters")]
    TooFewSamples { n: usize, k: usize },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ClusterError>;

const LN_2PI: f64 = 1.8378770664093453; // (2*π).ln()

// ═══════════════════════════════════════════════════════════════════════
// Matrix helper (faer 0.24 lacks from_row_major on owned Mat)
// ═══════════════════════════════════════════════════════════════════════

/// Create a `Mat<f64>` from row-major data slice.
pub fn mat_from_row_major(nrows: usize, ncols: usize, data: &[f64]) -> Mat<f64> {
    Mat::from_fn(nrows, ncols, |i, j| data[i * ncols + j])
}

// ═══════════════════════════════════════════════════════════════════════
// K-Means (native faer + rand implementation)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted K-Means model.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KMeansModel {
    pub centroids: Vec<Vec<f64>>,
    pub k: usize,
    pub n_features: usize,
    pub inertia: f64,
    pub n_iter: usize,
}

/// K-Means clustering options.
#[derive(Debug, Clone)]
pub struct KMeansOptions {
    pub k: usize,
    pub max_n_iterations: u32,
    pub n_init: u32,
    pub tolerance: f64,
    pub seed: u64,
}

impl Default for KMeansOptions {
    fn default() -> Self {
        Self {
            k: 3,
            max_n_iterations: 300,
            n_init: 10,
            tolerance: 1e-4,
            seed: 42,
        }
    }
}

/// Fit K-means with k-means++ initialisation, multiple restarts.
pub fn kmeans(data: &Mat<f64>, opts: &KMeansOptions) -> Result<KMeansModel> {
    if opts.k < 1 {
        return Err(ClusterError::InvalidK(opts.k));
    }
    let (nrows, ncols) = data.shape();
    if nrows == 0 {
        return Err(ClusterError::Empty);
    }
    if nrows < opts.k {
        return Err(ClusterError::TooFewSamples {
            n: nrows,
            k: opts.k,
        });
    }

    let mut best_model: Option<KMeansModel> = None;
    let mut best_inertia = f64::INFINITY;

    for run in 0..opts.n_init {
        let seed = opts.seed.wrapping_add(run as u64);
        let mut rng = ChaCha8Rng::seed_from_u64(seed);

        // k-means++ initialisation
        let mut centroids = kmeans_pp_init(data, opts.k, &mut rng);

        let mut prev_inertia = f64::INFINITY;
        let mut n_iter = 0usize;

        for iter in 0..opts.max_n_iterations {
            n_iter = (iter + 1) as usize;
            // Assignment step
            let (labels, inertia) = assign_clusters(data, &centroids);

            // Update step
            let mut new_centroids = vec![vec![0.0f64; ncols]; opts.k];
            let mut counts = vec![0usize; opts.k];
            for (i, &label) in labels.iter().enumerate() {
                counts[label] += 1;
                for j in 0..ncols {
                    new_centroids[label][j] += data[(i, j)];
                }
            }
            for c in 0..opts.k {
                if counts[c] > 0 {
                    for j in 0..ncols {
                        new_centroids[c][j] /= counts[c] as f64;
                    }
                } else {
                    // Empty cluster: keep old centroid
                    new_centroids[c] = centroids[c].clone();
                }
            }
            centroids = new_centroids;

            // Convergence check
            if (prev_inertia - inertia).abs() < opts.tolerance {
                break;
            }
            prev_inertia = inertia;
        }

        let (_, final_inertia) = assign_clusters(data, &centroids);
        if final_inertia < best_inertia {
            best_inertia = final_inertia;
            best_model = Some(KMeansModel {
                centroids,
                k: opts.k,
                n_features: ncols,
                inertia: final_inertia,
                n_iter,
            });
        }
    }

    best_model.ok_or(ClusterError::Other("K-means failed".into()))
}

/// Assign each sample to nearest centroid, return (labels, inertia).
fn assign_clusters(data: &Mat<f64>, centroids: &[Vec<f64>]) -> (Vec<usize>, f64) {
    let (nrows, ncols) = data.shape();
    let k = centroids.len();
    let mut labels = vec![0usize; nrows];
    let mut inertia = 0.0;
    for i in 0..nrows {
        let mut best_c = 0;
        let mut best_dist = f64::INFINITY;
        for c in 0..k {
            let dist: f64 = (0..ncols)
                .map(|j| (data[(i, j)] - centroids[c][j]).powi(2))
                .sum();
            if dist < best_dist {
                best_dist = dist;
                best_c = c;
            }
        }
        labels[i] = best_c;
        inertia += best_dist;
    }
    (labels, inertia)
}

/// K-means++ initialisation.
fn kmeans_pp_init(data: &Mat<f64>, k: usize, rng: &mut ChaCha8Rng) -> Vec<Vec<f64>> {
    let (n, d) = data.shape();
    let mut centroids: Vec<Vec<f64>> = Vec::with_capacity(k);
    // Choose first center randomly
    let first = rng.random_range(0..n);
    centroids.push((0..d).map(|j| data[(first, j)]).collect());

    for _ in 1..k {
        let dists: Vec<f64> = (0..n)
            .map(|i| {
                centroids
                    .iter()
                    .map(|c| {
                        (0..d)
                            .map(|j| (data[(i, j)] - c[j]).powi(2))
                            .sum::<f64>()
                    })
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
        let total: f64 = dists.iter().sum();
        if total == 0.0 {
            let idx = rng.random_range(0..n);
            centroids.push((0..d).map(|j| data[(idx, j)]).collect());
            continue;
        }
        let r = rng.random::<f64>() * total;
        let mut cumsum = 0.0;
        let mut chosen = n - 1;
        for (i, &di) in dists.iter().enumerate() {
            cumsum += di;
            if cumsum >= r {
                chosen = i;
                break;
            }
        }
        centroids.push((0..d).map(|j| data[(chosen, j)]).collect());
    }
    centroids
}

/// Predict nearest cluster for each sample.
pub fn kmeans_predict(model: &KMeansModel, data: &Mat<f64>) -> Vec<usize> {
    let (nrows, ncols) = data.shape();
    (0..nrows)
        .map(|i| {
            let mut best_k = 0;
            let mut best_dist = f64::INFINITY;
            for (k, c) in model.centroids.iter().enumerate() {
                let dist: f64 = (0..ncols)
                    .map(|j| (data[(i, j)] - c[j]).powi(2))
                    .sum();
                if dist < best_dist {
                    best_dist = dist;
                    best_k = k;
                }
            }
            best_k
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// Gaussian Mixture Model (EM)
// ═══════════════════════════════════════════════════════════════════════

/// Fitted GMM parameters.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GmmModel {
    pub weights: Vec<f64>,
    pub means: Vec<Vec<f64>>,
    pub covariances: Vec<Vec<Vec<f64>>>,
    pub k: usize,
    pub n_features: usize,
    pub log_likelihood: f64,
    pub bic: f64,
    pub aic: f64,
    pub n_iter: usize,
}

#[derive(Debug, Clone)]
pub struct GmmOptions {
    pub k: usize,
    pub max_iter: usize,
    pub tol: f64,
    pub seed: u64,
    pub n_init: usize,
}

impl Default for GmmOptions {
    fn default() -> Self {
        Self {
            k: 3,
            max_iter: 100,
            tol: 1e-6,
            seed: 42,
            n_init: 1,
        }
    }
}

pub fn gaussian_mixture(data: &Mat<f64>, opts: &GmmOptions) -> Result<GmmModel> {
    if opts.k < 1 {
        return Err(ClusterError::InvalidK(opts.k));
    }
    let (n, d) = data.shape();
    if n < opts.k {
        return Err(ClusterError::TooFewSamples { n, k: opts.k });
    }
    if n == 0 {
        return Err(ClusterError::Empty);
    }

    let mut best_model: Option<GmmModel> = None;

    for run in 0..opts.n_init {
        let seed = opts.seed.wrapping_add(run as u64);
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let result = fit_gmm_once(data, opts.k, opts.max_iter, opts.tol, &mut rng)?;
        match &best_model {
            Some(b) if b.bic <= result.bic => {}
            _ => best_model = Some(result),
        }
    }
    best_model.ok_or(ClusterError::Other("GMM failed".into()))
}

fn fit_gmm_once(
    data: &Mat<f64>,
    k: usize,
    max_iter: usize,
    tol: f64,
    rng: &mut ChaCha8Rng,
) -> Result<GmmModel> {
    let (n, d) = data.shape();

    // Initialise via k-means++
    let init_means = kmeans_pp_init(data, k, rng);
    let mut means = init_means.clone();
    let mut weights = vec![1.0 / k as f64; k];
    let mut covariances: Vec<Mat<f64>> = (0..k).map(|_| Mat::identity(d, d)).collect();

    let mut prev_ll = f64::NEG_INFINITY;
    let mut n_iter = 0;

    for iter in 0..max_iter {
        n_iter = iter + 1;

        // E-step
        let mut resp = vec![vec![0.0f64; k]; n];
        let mut ll = 0.0;
        for i in 0..n {
            let point: Vec<f64> = (0..d).map(|j| data[(i, j)]).collect();
            let densities: Vec<f64> = (0..k)
                .map(|c| weights[c] * gaussian_density(&point, &means[c], &covariances[c]))
                .collect();
            let total: f64 = densities.iter().sum();
            if total > 0.0 {
                ll += total.ln();
            }
            for c in 0..k {
                resp[i][c] = if total > 0.0 {
                    densities[c] / total
                } else {
                    1.0 / k as f64
                };
            }
        }

        if (ll - prev_ll).abs() < tol {
            break;
        }
        prev_ll = ll;

        // M-step
        for c in 0..k {
            let nk: f64 = resp.iter().map(|r| r[c]).sum();
            weights[c] = nk / n as f64;
            if nk < 1e-10 {
                continue;
            }
            for j in 0..d {
                means[c][j] = (0..n).map(|i| resp[i][c] * data[(i, j)]).sum::<f64>() / nk;
            }
            let mut cov = Mat::zeros(d, d);
            for i in 0..n {
                let diff: Vec<f64> = (0..d).map(|j| data[(i, j)] - means[c][j]).collect();
                for r in 0..d {
                    for col in 0..d {
                        cov[(r, col)] += resp[i][c] * diff[r] * diff[col];
                    }
                }
            }
            for r in 0..d {
                for col in 0..d {
                    cov[(r, col)] = cov[(r, col)] / nk + if r == col { 1e-6 } else { 0.0 };
                }
            }
            covariances[c] = cov;
        }
    }

    // Final log-likelihood
    let mut final_ll = 0.0;
    for i in 0..n {
        let point: Vec<f64> = (0..d).map(|j| data[(i, j)]).collect();
        let total: f64 = (0..k)
            .map(|c| weights[c] * gaussian_density(&point, &means[c], &covariances[c]))
            .sum();
        if total > 0.0 {
            final_ll += total.ln();
        }
    }

    let n_params = k * (d + d * (d + 1) / 2) + k - 1;
    let bic = -2.0 * final_ll + n_params as f64 * (n as f64).ln();
    let aic = -2.0 * final_ll + 2.0 * n_params as f64;

    let covs_serialized: Vec<Vec<Vec<f64>>> = covariances
        .iter()
        .map(|c| {
            let (r, _) = c.shape();
            (0..r)
                .map(|i| (0..r).map(|j| c[(i, j)]).collect())
                .collect()
        })
        .collect();

    Ok(GmmModel {
        weights,
        means,
        covariances: covs_serialized,
        k,
        n_features: d,
        log_likelihood: final_ll,
        bic,
        aic,
        n_iter,
    })
}

pub fn gaussian_density_pub(point: &[f64], mean: &[f64], cov: &Mat<f64>) -> f64 {
    gaussian_density(point, mean, cov)
}

fn gaussian_density(point: &[f64], mean: &[f64], cov: &Mat<f64>) -> f64 {
    use faer::linalg::solvers::Solve;

    let d = point.len();
    let cov_reg = {
        let mut m = cov.clone();
        for i in 0..d {
            m[(i, i)] += 1e-10;
        }
        m
    };

    let diff: Vec<f64> = point.iter().zip(mean).map(|(a, b)| a - b).collect();
    let diff_col = mat_from_row_major(d, 1, &diff);

    let lu = cov_reg.partial_piv_lu();
    let u = lu.U();
    let log_det: f64 = (0..d).map(|i| u[(i, i)].abs().ln()).sum();
    let solved = lu.solve(&diff_col);
    let mahalanobis_sq: f64 = diff
        .iter()
        .enumerate()
        .map(|(i, &di)| di * solved[(i, 0)])
        .sum();

    let log_density = -0.5 * (d as f64 * LN_2PI + log_det + mahalanobis_sq);
    log_density.exp()
}

// ═══════════════════════════════════════════════════════════════════════
// DBSCAN (naive O(n²) implementation — fine for moderate n)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct DbscanResult {
    pub labels: Vec<Option<usize>>,
    pub n_clusters: usize,
    pub n_noise: usize,
}

#[derive(Debug, Clone)]
pub struct DbscanOptions {
    pub eps: f64,
    pub min_points: usize,
}

pub fn dbscan(data: &Mat<f64>, opts: &DbscanOptions) -> Result<DbscanResult> {
    let (n, d) = data.shape();
    if n == 0 {
        return Err(ClusterError::Empty);
    }

    let eps_sq = opts.eps * opts.eps;

    // Pre-compute neighbourhoods
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        for j in (i + 1)..n {
            let dist_sq: f64 = (0..d)
                .map(|k| (data[(i, k)] - data[(j, k)]).powi(2))
                .sum();
            if dist_sq <= eps_sq {
                neighbours[i].push(j);
                neighbours[j].push(i);
            }
        }
    }

    // Classify core points (≥ min_points in neighbourhood, including self)
    let is_core: Vec<bool> = (0..n)
        .map(|i| neighbours[i].len() + 1 >= opts.min_points)
        .collect();

    let mut labels = vec![None; n];
    let mut current_cluster = 0usize;

    for i in 0..n {
        if labels[i].is_some() || !is_core[i] {
            continue;
        }
        // BFS expand cluster
        labels[i] = Some(current_cluster);
        let mut queue = vec![i];
        while let Some(node) = queue.pop() {
            for &nbr in &neighbours[node] {
                if labels[nbr].is_none() {
                    labels[nbr] = Some(current_cluster);
                    if is_core[nbr] {
                        queue.push(nbr);
                    }
                }
            }
        }
        current_cluster += 1;
    }

    let n_noise = labels.iter().filter(|l| l.is_none()).count();
    Ok(DbscanResult {
        labels,
        n_clusters: current_cluster,
        n_noise,
    })
}

// ═══════════════════════════════════════════════════════════════════════
// Agglomerative / Hierarchical clustering (naive O(n²) single/complete/average linkage)
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub enum Linkage {
    Ward,
    Complete,
    Average,
    Single,
}

#[derive(Debug, Clone)]
pub struct HierarchicalResult {
    pub labels: Vec<usize>,
    pub n_clusters: usize,
}

pub fn hierarchical(data: &Mat<f64>, k: usize, linkage: Linkage) -> Result<HierarchicalResult> {
    let (n, d) = data.shape();
    if n == 0 {
        return Err(ClusterError::Empty);
    }
    if k < 1 {
        return Err(ClusterError::InvalidK(k));
    }

    // Start with each point as its own cluster
    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();

    // Pre-compute pairwise distances
    let mut dist = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let dd: f64 = (0..d)
                .map(|k2| (data[(i, k2)] - data[(j, k2)]).powi(2))
                .sum::<f64>()
                .sqrt();
            dist[i][j] = dd;
            dist[j][i] = dd;
        }
    }

    // Merge until k clusters remain
    while clusters.len() > k {
        // Find closest pair
        let mut best_i = 0;
        let mut best_j = 1;
        let mut best_dist = f64::INFINITY;
        for ci in 0..clusters.len() {
            for cj in (ci + 1)..clusters.len() {
                let merge_dist = cluster_distance(&clusters[ci], &clusters[cj], &dist, linkage);
                if merge_dist < best_dist {
                    best_dist = merge_dist;
                    best_i = ci;
                    best_j = cj;
                }
            }
        }

        // Merge j into i
        let merged_j = clusters.remove(best_j);
        clusters[best_i].extend(merged_j);
    }

    // Assign labels
    let mut labels = vec![0usize; n];
    for (cluster_id, members) in clusters.iter().enumerate() {
        for &member in members {
            labels[member] = cluster_id;
        }
    }

    Ok(HierarchicalResult {
        labels,
        n_clusters: clusters.len(),
    })
}

fn cluster_distance(
    c1: &[usize],
    c2: &[usize],
    dist: &[Vec<f64>],
    linkage: Linkage,
) -> f64 {
    match linkage {
        Linkage::Single => c1
            .iter()
            .flat_map(|&i| c2.iter().map(move |&j| dist[i][j]))
            .fold(f64::INFINITY, f64::min),
        Linkage::Complete => c1
            .iter()
            .flat_map(|&i| c2.iter().map(move |&j| dist[i][j]))
            .fold(0.0f64, f64::max),
        Linkage::Average => {
            let sum: f64 = c1
                .iter()
                .flat_map(|&i| c2.iter().map(move |&j| dist[i][j]))
                .sum();
            sum / (c1.len() * c2.len()) as f64
        }
        Linkage::Ward => {
            // Approximation: use average linkage for Ward
            // (true Ward's method requires computing centroids + SSE)
            let sum: f64 = c1
                .iter()
                .flat_map(|&i| c2.iter().map(move |&j| dist[i][j]))
                .sum();
            sum / (c1.len() * c2.len()) as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_data() -> Mat<f64> {
        mat_from_row_major(
            6,
            2,
            &[
                0.0, 0.0, 1.0, 0.5, 0.5, 1.0, //
                10.0, 10.0, 11.0, 10.5, 10.5, 11.0,
            ],
        )
    }

    #[test]
    fn test_kmeans() {
        let data = make_test_data();
        let opts = KMeansOptions {
            k: 2,
            seed: 42,
            ..Default::default()
        };
        let model = kmeans(&data, &opts).unwrap();
        assert_eq!(model.k, 2);
        let labels = kmeans_predict(&model, &data);
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[1], labels[2]);
        assert_eq!(labels[3], labels[4]);
        assert_ne!(labels[0], labels[3]);
    }

    #[test]
    fn test_gmm() {
        let data = make_test_data();
        let opts = GmmOptions {
            k: 2,
            max_iter: 50,
            ..Default::default()
        };
        let model = gaussian_mixture(&data, &opts).unwrap();
        assert_eq!(model.k, 2);
        assert!(model.bic.is_finite());
    }

    #[test]
    fn test_dbscan() {
        let data = make_test_data();
        let result = dbscan(&data, &DbscanOptions { eps: 2.0, min_points: 2 }).unwrap();
        assert!(result.n_clusters >= 1);
    }

    #[test]
    fn test_hierarchical() {
        let data = make_test_data();
        let result = hierarchical(&data, 2, Linkage::Complete).unwrap();
        assert_eq!(result.n_clusters, 2);
        assert_eq!(result.labels[0], result.labels[1]);
        assert_ne!(result.labels[0], result.labels[3]);
    }
}
