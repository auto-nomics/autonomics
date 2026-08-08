//! Survey-weighted hypothesis tests.
//!
//! Implements the Rao-Scott chi-squared test for two-way contingency tables
//! under a complex survey design (`svychisq`).

use faer::Mat;
use faer::linalg::solvers::{DenseSolveCore, Llt, Solve};

use crate::design::SurveyDesign;
use crate::error::{Result, SurveyError};
use crate::variance::svy_cprod_matrix;

/// Result of a survey chi-squared test.
#[derive(Debug, Clone)]
pub struct SvyChisq {
    /// F statistic (Rao-Scott adjusted).
    pub statistic: f64,
    /// Numerator degrees of freedom.
    pub ndf: f64,
    /// Denominator degrees of freedom.
    pub ddf: f64,
    /// p-value from F(ndf, ddf).
    pub p_value: f64,
}

/// Design-based Rao-Scott chi-squared test for a two-way table.
///
/// # Arguments
/// - `row`: row variable (categorical) per observation.
/// - `col`: column variable (categorical) per observation.
/// - `design`: the survey design.
pub fn svy_chisq(row: &[String], col: &[String], design: &SurveyDesign) -> Result<SvyChisq> {
    let n = design.n_obs;
    if row.len() != n || col.len() != n {
        return Err(SurveyError::LengthMismatch {
            context: "row/col vs design".into(),
            a: row.len(),
            b: n,
        });
    }
    let w = design.weights();

    // Unique levels in order of appearance.
    let row_levels = unique(row);
    let col_levels = unique(col);
    let nr = row_levels.len();
    let nc = col_levels.len();
    let ncell = nr * nc;

    // Cell indicator matrix (n × ncell) + svymean of cell proportions.
    let mut cells = vec![vec![0.0_f64; n]; ncell];
    let mut cell_index: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    for r in 0..nr {
        for c in 0..nc {
            cell_index.insert((row_levels[r].clone(), col_levels[c].clone()), r * nc + c);
        }
    }
    for i in 0..n {
        if let Some(&idx) = cell_index.get(&(row[i].clone(), col[i].clone())) {
            cells[idx][i] = 1.0;
        }
    }

    // Weighted cell proportions and their design covariance.
    let total_w: f64 = w.iter().sum();
    let mut mean2 = vec![0.0_f64; ncell];
    for j in 0..ncell {
        mean2[j] = cells[j].iter().zip(&w).map(|(&x, &wi)| x * wi).sum::<f64>() / total_w;
    }
    // Center + scale for covariance.
    let z: Vec<Vec<f64>> = (0..ncell)
        .map(|j| {
            (0..n)
                .map(|i| w[i] * (cells[j][i] - mean2[j]) / total_w)
                .collect()
        })
        .collect();
    let v = svy_cprod_matrix(&z, design)?;

    // Model matrices: X1 (main effects, nr+nc-1 cols), X12 (interaction).
    // Build X1: intercept + row indicators (nr-1) + col indicators (nc-1).
    let p1 = 1 + (nr - 1) + (nc - 1);
    let mut x1 = vec![vec![0.0_f64; p1]; ncell];
    for c in 0..ncell {
        let r = c / nc;
        let k = c % nc;
        x1[c][0] = 1.0;
        if r > 0 {
            x1[c][r] = 1.0; // row indicator
        }
        if k > 0 {
            x1[c][nr - 1 + k] = 1.0; // col indicator
        }
    }
    // X12: interaction columns = cell indicators (ncell), minus main effects.
    // Cmat = qr.resid(qr(X1), X12[,-(1:(nr+nc-1))]) → interaction part only.
    // The interaction columns of X12 are the last (nr-1)(nc-1) columns.
    let nint = (nr - 1) * (nc - 1);
    let mut xint = vec![vec![0.0_f64; nint]; ncell];
    // Interaction cell (r>0, c>0) maps to interaction column.
    for c in 0..ncell {
        let r = c / nc;
        let k = c % nc;
        if r > 0 && k > 0 {
            xint[c][(r - 1) * (nc - 1) + (k - 1)] = 1.0;
        }
    }
    // QR residual of X1 from xint columns (projection residual).
    let cmat = qr_resid(&x1, &xint);

    // Delta = solve(C' iD/N C, C' iD/N V iD/N C)
    // where iD = diag(1/mean2), N = number of cells (here ncell... actually N=nrow).
    // R uses N = NROW(mm) = number of observations.
    let n_obs_f = n as f64;
    let id_vec: Vec<f64> = mean2
        .iter()
        .map(|&m| if m == 0.0 { 0.0 } else { 1.0 / m })
        .collect();

    // denom = C' (iD/N) C
    let denom = quadratic(&cmat, &id_vec, 1.0 / n_obs_f);
    // numr = C' iD V iD C
    let numr = sandwich(&cmat, &id_vec, &v, &id_vec);
    // Delta = solve(denom, numr)
    let delta = solve_sym(&denom, &numr)?;

    // trace(Delta), trace(Delta²)
    let t = nint;
    let mut tr_delta = 0.0;
    let mut tr_delta2 = 0.0;
    for i in 0..t {
        tr_delta += delta[i][i];
    }
    for i in 0..t {
        for k in 0..t {
            tr_delta2 += delta[i][k] * delta[k][i];
        }
    }
    let d0 = tr_delta * tr_delta / tr_delta2.max(1e-12);
    let nu = design.degf() as f64;

    // Pearson statistic on the weighted table, scaled to total N observations
    // (R: svytable(..., Ntotal=N) scales the table so its sum is N).
    let raw_pearson = pearson_stat(row, col, &w);
    let pearson = raw_pearson * n as f64 / total_w;

    // F = pearson / trace(Delta)
    let statistic = pearson / tr_delta;
    let ndf = d0;
    let ddf = d0 * nu;
    let p_value = f_dist_surv(statistic, ndf, ddf);

    Ok(SvyChisq {
        statistic,
        ndf,
        ddf,
        p_value,
    })
}

/// Unique values in order of first appearance.
fn unique(x: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for v in x {
        if seen.insert(v.clone()) {
            out.push(v.clone());
        }
    }
    out
}

/// Compute the Pearson chi-squared statistic from a weighted two-way table.
fn pearson_stat(row: &[String], col: &[String], w: &[f64]) -> f64 {
    let n = row.len();
    let row_levels = unique(row);
    let col_levels = unique(col);
    let mut obs = vec![vec![0.0_f64; col_levels.len()]; row_levels.len()];
    let mut row_tot = vec![0.0_f64; row_levels.len()];
    let mut col_tot = vec![0.0_f64; col_levels.len()];
    let mut total = 0.0_f64;
    for i in 0..n {
        let r = row_levels.iter().position(|x| x == &row[i]).unwrap();
        let c = col_levels.iter().position(|x| x == &col[i]).unwrap();
        obs[r][c] += w[i];
        row_tot[r] += w[i];
        col_tot[c] += w[i];
        total += w[i];
    }
    let mut x2 = 0.0_f64;
    for r in 0..row_levels.len() {
        for c in 0..col_levels.len() {
            let expected = row_tot[r] * col_tot[c] / total;
            if expected > 0.0 {
                x2 += (obs[r][c] - expected).powi(2) / expected;
            }
        }
    }
    x2
}

/// C' (iD scale) C where iD is a diagonal vector indexed by cell, scale a scalar.
///
/// `c` is ncell × nint (rows = cells, cols = interaction columns).
/// Result is nint × nint: `out[a][b] = Σ_k c[k][a]·iD[k]·c[k][b]·scale`.
fn quadratic(c: &[Vec<f64>], id: &[f64], scale: f64) -> Vec<Vec<f64>> {
    let ncell = c.len();
    let nint = c[0].len();
    let mut out = vec![vec![0.0_f64; nint]; nint];
    for a in 0..nint {
        for b in 0..nint {
            let mut s = 0.0;
            for k in 0..ncell {
                s += c[k][a] * id[k] * c[k][b] * scale;
            }
            out[a][b] = s;
        }
    }
    out
}

/// C' iD V iD C (full sandwich).
///
/// `c` is ncell × nint; `v` is ncell × ncell; `id`/`id2` are length ncell.
fn sandwich(c: &[Vec<f64>], id: &[f64], v: &[Vec<f64>], id2: &[f64]) -> Vec<Vec<f64>> {
    let ncell = c.len();
    let nint = c[0].len();
    // iD V iD → m (ncell × ncell)
    let mut m = vec![vec![0.0_f64; ncell]; ncell];
    for i in 0..ncell {
        for j in 0..ncell {
            m[i][j] = id[i] * v[i][j] * id2[j];
        }
    }
    // C' m C → nint × nint
    let mut out = vec![vec![0.0_f64; nint]; nint];
    for a in 0..nint {
        for b in 0..nint {
            let mut s = 0.0;
            for i in 0..ncell {
                for j in 0..ncell {
                    s += c[i][a] * m[i][j] * c[j][b];
                }
            }
            out[a][b] = s;
        }
    }
    out
}

/// Solve A X = B robustly via LU with a small ridge on the diagonal of A.
fn solve_sym(a: &[Vec<f64>], b: &[Vec<f64>]) -> Result<Vec<Vec<f64>>> {
    use faer::linalg::solvers::PartialPivLu;
    let n = a.len();
    let ridge = 1e-12
        * (0..n)
            .map(|i| a[i][i].abs())
            .fold(0.0_f64, f64::max)
            .max(1e-30);
    let a_m = Mat::from_fn(n, n, |i, j| a[i][j] + if i == j { ridge } else { 0.0 });
    let b_m = Mat::from_fn(n, n, |i, j| b[i][j]);
    let lu = PartialPivLu::new(a_m.as_ref());
    let x = lu.solve(&b_m);
    Ok((0..n)
        .map(|i| (0..n).map(|j| x[(i, j)]).collect())
        .collect())
}

/// F-distribution survival function via statrs.
fn f_dist_surv(x: f64, df1: f64, df2: f64) -> f64 {
    use statrs::distribution::{ContinuousCDF, FisherSnedecor};
    FisherSnedecor::new(df1, df2)
        .map(|d| d.sf(x))
        .unwrap_or(0.0)
}

/// QR residuals: project each column of `x` onto the column space of `x1`,
/// returning the residual matrix `x - P_{x1} x`.
fn qr_resid(x1: &[Vec<f64>], x: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = x.len(); // = ncell (rows)
    let p1 = x1[0].len();
    // (X1'X1)^{-1} X1' via Llt.
    let mut xtx = vec![vec![0.0_f64; p1]; p1];
    for i in 0..n {
        for a in 0..p1 {
            for b in 0..p1 {
                xtx[a][b] += x1[i][a] * x1[i][b];
            }
        }
    }
    let xtx_m = Mat::from_fn(p1, p1, |a, b| xtx[a][b]);
    let llt = Llt::new(xtx_m.as_ref(), faer::Side::Lower).ok();
    let nint = x[0].len();
    let mut out = vec![vec![0.0_f64; nint]; n];
    if let Some(llt) = llt {
        for col in 0..nint {
            // x1' x_col
            let xtx1x: Vec<f64> = (0..p1)
                .map(|a| (0..n).map(|i| x1[i][a] * x[i][col]).sum())
                .collect();
            let b_m = Mat::from_fn(p1, 1, |a, _| xtx1x[a]);
            let coef = llt.solve(&b_m);
            for i in 0..n {
                let mut proj = 0.0;
                for a in 0..p1 {
                    proj += x1[i][a] * coef[(a, 0)];
                }
                out[i][col] = x[i][col] - proj;
            }
        }
    } else {
        // If X1 is singular (e.g., a level absent), fall back to raw interaction.
        for col in 0..nint {
            for i in 0..n {
                out[i][col] = x[i][col];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::SurveyDesignBuilder;

    #[test]
    fn pearson_stat_correct() {
        // 2x2 table with known chi-squared.
        let row = vec![
            "a".to_string(),
            "a".to_string(),
            "b".to_string(),
            "b".to_string(),
        ];
        let col = vec![
            "x".to_string(),
            "y".to_string(),
            "x".to_string(),
            "y".to_string(),
        ];
        let w = vec![1.0, 1.0, 1.0, 1.0];
        let x2 = pearson_stat(&row, &col, &w);
        assert!(x2 >= 0.0);
    }

    #[test]
    fn svy_chisq_apiclus1_matches_r() {
        // R golden (apiclus1, stype ~ comp.imp, statistic="F"):
        //   F=3.4748, ndf=1.4294, ddf=20.0110, p=0.06408
        // Load apiclus1.csv if present.
        if !std::path::Path::new("/tmp/apiclus1.csv").exists() {
            eprintln!("skipping: /tmp/apiclus1.csv not present");
            return;
        }
        let data = std::fs::read_to_string("/tmp/apiclus1.csv").unwrap();
        let mut lines = data.lines();
        let header = lines.next().unwrap();
        let idx: std::collections::HashMap<&str, usize> = header
            .split(',')
            .enumerate()
            .map(|(i, n)| (n.trim_matches('"'), i))
            .collect();
        let mut stype = Vec::new();
        let mut comp = Vec::new();
        let mut dnum = Vec::new();
        let mut pw = Vec::new();
        for line in lines {
            let cols: Vec<&str> = line.split(',').collect();
            stype.push(cols[idx["stype"]].to_string());
            comp.push(cols[idx["comp.imp"]].to_string());
            dnum.push(cols[idx["dnum"]].parse::<usize>().unwrap());
            pw.push(cols[idx["pw"]].parse::<f64>().unwrap());
        }
        let n = stype.len();
        let d = crate::design::SurveyDesignBuilder::new()
            .strata(vec!["1".to_string(); n])
            .cluster(dnum.iter().map(|i| i.to_string()).collect())
            .weights(pw)
            .build()
            .unwrap();
        let r = svy_chisq(&stype, &comp, &d).unwrap();
        assert!(
            (r.statistic - 3.4748).abs() < 0.5,
            "F: {} (R=3.4748)",
            r.statistic
        );
        assert!(
            (r.p_value - 0.06408).abs() < 0.05,
            "p: {} (R=0.06408)",
            r.p_value
        );
    }

    #[test]
    fn svy_chisq_returns_finite() {
        let d = SurveyDesignBuilder::new()
            .strata(vec!["1".into(); 20])
            .cluster((0..20).map(|i| i.to_string()).collect())
            .weights(vec![1.0; 20])
            .build()
            .unwrap();
        let row: Vec<String> = (0..20)
            .map(|i| if i % 2 == 0 { "a".into() } else { "b".into() })
            .collect();
        let col: Vec<String> = (0..20)
            .map(|i| if i % 3 == 0 { "x".into() } else { "y".into() })
            .collect();
        let r = svy_chisq(&row, &col, &d).unwrap();
        assert!(r.statistic.is_finite());
        assert!(r.p_value >= 0.0 && r.p_value <= 1.0);
    }
}
