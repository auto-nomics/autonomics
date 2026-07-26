//! 闭式泛型优化器（接收 `Fn(&[f64]) -> f64` cost 闭包）。
//!
//! 复刻原版 fit2 的四步无导数优化：
//!   - `differential_evolution`（DE/rand/1/bin，scipy `differential_evolution` 默认）
//!   - `nelder_mead`（adaptive，scipy `method='Nelder-Mead', adaptive=True`）
//!   - `brute1`（1D 网格，scipy `optimize.brute` Ns=20）
//!   - `brent1`（1D Brent，scipy `optimize.brent`，parabolic + golden-section）
//!
//! 与 `crate::optimizer`（univariate 专用、写死 3D + univariate cost）解耦，
//! 不影响已验证的 univariate 路径。

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

// ───────────────────────── 差分进化 ─────────────────────────

/// DE/rand/1/bin。`bounds` 每维 (low, high)；种群 = `popsize_mult · dim`。
///
/// 对齐 scipy：mutation F∈[0.5,1] 每代抖动、recombination CR=0.7、收敛 tol。
pub fn differential_evolution<F>(
    cost: F,
    bounds: &[(f64, f64)],
    popsize_mult: usize,
    max_gen: usize,
    tol: f64,
    seed: u64,
) -> Vec<f64>
where
    F: Fn(&[f64]) -> f64,
{
    let mut rng = SmallRng::seed_from_u64(seed);
    let dim = bounds.len();
    let popsize = (popsize_mult * dim).max(5);
    const CR: f64 = 0.7;

    let mut pop: Vec<Vec<f64>> = (0..popsize)
        .map(|_| {
            (0..dim)
                .map(|d| rng.gen_range(bounds[d].0..bounds[d].1))
                .collect()
        })
        .collect();
    let mut costs: Vec<f64> = pop.iter().map(|x| cost(x)).collect();

    for _ in 0..max_gen {
        let f = rng.gen_range(0.5..1.0);
        for i in 0..popsize {
            // 选 3 个互不相同且 != i 的索引
            let (a, b, c) = pick_three(&mut rng, i, popsize);
            let j_rand = rng.gen_range(0..dim);
            let mut trial = vec![0.0; dim];
            for d in 0..dim {
                let mutant = (pop[a][d] + f * (pop[b][d] - pop[c][d])).clamp(bounds[d].0, bounds[d].1);
                trial[d] = if d == j_rand || rng.gen_range(0.0..1.0) < CR {
                    mutant
                } else {
                    pop[i][d]
                };
            }
            let ct = cost(&trial);
            if ct <= costs[i] {
                pop[i] = trial;
                costs[i] = ct;
            }
        }
        let cmin = costs.iter().cloned().fold(f64::INFINITY, f64::min);
        let cmax = costs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if cmax - cmin < tol {
            break;
        }
    }
    let (bi, _) = costs.iter().enumerate().min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap()).unwrap();
    pop[bi].clone()
}

fn pick_three(rng: &mut SmallRng, exclude: usize, popsize: usize) -> (usize, usize, usize) {
    let mut idxs: Vec<usize> = (0..popsize).filter(|&x| x != exclude).collect();
    for k in 0..3 {
        let j = rng.gen_range(k..idxs.len());
        idxs.swap(k, j);
    }
    (idxs[0], idxs[1], idxs[2])
}

// ───────────────────────── Nelder-Mead（adaptive） ─────────────────────────

/// Adaptive Nelder-Mead（Gao & Han 2012），对齐 scipy `adaptive=True`。
///
/// 收敛：x 边长 < xatol 且 f 极差 < fatol（scipy 风格）。返回最优点。
pub fn nelder_mead<F>(cost: F, x0: &[f64], step: f64, xatol: f64, fatol: f64, max_iter: usize) -> Vec<f64>
where
    F: Fn(&[f64]) -> f64,
{
    let dim = x0.len();
    // adaptive 系数
    let alpha = 1.0;
    let gamma = 1.0 + 2.0 / dim as f64;
    let rho = 0.75 - 1.0 / (2.0 * dim as f64);
    let sigma = 1.0 - 1.0 / dim as f64;

    // 初始单纯形：x0 + 每个方向偏移 step
    let mut simplex: Vec<(Vec<f64>, f64)> = (0..=dim)
        .map(|i| {
            let mut x = x0.to_vec();
            if i > 0 {
                x[i - 1] += step;
            }
            let c = cost(&x);
            (x, c)
        })
        .collect();

    for _ in 0..max_iter {
        simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let best = &simplex[0];
        let worst = &simplex[dim];

        // 收敛：所有顶点到 best 的距离 < xatol，且 f 极差 < fatol
        let max_dx = simplex.iter().skip(1).map(|(x, _)| dist(x, &best.0)).fold(0.0_f64, f64::max);
        let max_df = simplex.iter().map(|(_, f)| (f - best.1).abs()).fold(0.0_f64, f64::max);
        if max_dx < xatol && max_df < fatol {
            break;
        }

        // 重心（除最差）
        let centroid = centroid_of(&simplex[..dim]);

        let xr = reflect(&centroid, &worst.0, alpha);
        let fr = cost(&xr);
        let second_worst_f = simplex[dim - 1].1;

        let new_point = if fr < best.1 {
            // 扩张：xe = centroid + gamma·(xr − centroid)
            let xe: Vec<f64> = centroid.iter().zip(xr.iter()).map(|(c, r)| c + gamma * (r - c)).collect();
            let fe = cost(&xe);
            if fe < fr { (xe, fe) } else { (xr, fr) }
        } else if fr < second_worst_f {
            (xr, fr)
        } else {
            // 收缩
            let dir = if fr < worst.1 { xr } else { worst.0.clone() };
            let xc: Vec<f64> = centroid.iter().zip(dir.iter()).map(|(c, d)| c + rho * (d - c)).collect();
            let fc = cost(&xc);
            if fc < worst.1 {
                (xc, fc)
            } else {
                // 缩边
                let bv = best.0.clone();
                for v in simplex.iter_mut().take(dim + 1).skip(1) {
                    let xs: Vec<f64> = bv.iter().zip(v.0.iter()).map(|(b, x)| b + sigma * (x - b)).collect();
                    *v = (xs.clone(), cost(&xs));
                }
                continue;
            }
        };
        simplex[dim] = new_point;
    }
    simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    simplex[0].0.clone()
}

fn dist(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum::<f64>().sqrt()
}

fn centroid_of(verts: &[(Vec<f64>, f64)]) -> Vec<f64> {
    let dim = verts[0].0.len();
    let n = verts.len() as f64;
    let mut c = vec![0.0; dim];
    for (x, _) in verts {
        for d in 0..dim {
            c[d] += x[d];
        }
    }
    for d in 0..dim {
        c[d] /= n;
    }
    c
}

fn reflect(centroid: &[f64], worst: &[f64], alpha: f64) -> Vec<f64> {
    centroid.iter().zip(worst.iter()).map(|(c, w)| c + alpha * (c - w)).collect()
}

// ───────────────────────── brute1（1D 网格） ─────────────────────────

/// 1D 暴力网格：在 `[lo, hi]` 上取 `ns` 个等距点，返回 cost 最小者。
///
/// 对齐 scipy `optimize.brute(ranges=[(lo,hi)], Ns=ns)`。endpoint=False（不含 hi）。
pub fn brute1<F>(cost: F, lo: f64, hi: f64, ns: usize) -> (f64, f64)
where
    F: Fn(f64) -> f64,
{
    let mut best_x = lo;
    let mut best_f = f64::INFINITY;
    for i in 0..ns {
        let x = lo + (hi - lo) * (i as f64) / (ns as f64);
        let f = cost(x);
        if f < best_f {
            best_f = f;
            best_x = x;
        }
    }
    (best_x, best_f)
}

// ───────────────────────── brent1（1D Brent） ─────────────────────────

/// 1D Brent 极小化（parabolic 插值 + golden-section 兜底）。
///
/// 复刻 Numerical Recipes 的 `brent`（scipy `optimize.brent` 同源算法）。
/// 输入括号 `(ax, bx, cx)`，要求 bx 在 ax、cx 之间且 f(bx) 同时 < f(ax)、f(cx)。
/// 若不满足（scipy 会抛 ValueError），返回 `None`，由调用方回退到 bx。
pub fn brent1<F>(cost: F, ax: f64, bx: f64, cx: f64, xtol: f64, max_iter: usize) -> Option<(f64, f64)>
where
    F: Fn(f64) -> f64,
{
    const CGOLD: f64 = 0.3819660112501051; // (√5−1)/2
    const ZEPS: f64 = 1e-10;

    let fa = cost(ax);
    let fb = cost(bx);
    let fc = cost(cx);
    // 括号有效性：bx 是三者中的（严格）极小
    if !(fb < fa && fb < fc) {
        return None;
    }

    let mut a = ax;
    let mut c = cx;
    if a > c {
        std::mem::swap(&mut a, &mut c);
    }
    let mut x = bx;
    let mut w = bx;
    let mut v = bx;
    let mut fx = fb;
    let mut fw = fb;
    let mut fv = fb;
    let mut e: f64 = 0.0; // 上一步（跨两步前）的位移
    let mut d: f64 = 0.0;

    for _ in 0..max_iter {
        let xm = 0.5 * (a + c);
        let tol1 = xtol * x.abs() + ZEPS;
        let tol2 = 2.0 * tol1;
        if (x - xm).abs() <= tol2 - 0.5 * (c - a) {
            return Some((x, fx));
        }
        let tried_parabolic = if e.abs() > tol1 {
            // parabolic 拟合
            let r = (x - w) * (fx - fv);
            let q = (x - v) * (fx - fw);
            let mut p = (x - v) * q - (x - w) * r;
            let mut q = 2.0 * (q - r);
            if q > 0.0 {
                p = -p;
            } else {
                q = -q;
            }
            let etemp = e;
            e = d;
            if p.abs() < (0.5 * q * etemp).abs()
                && p > q * (a - x)
                && p < q * (c - x)
            {
                d = p / q;
                let u = x + d;
                if (u - a) < tol2 || (c - u) < tol2 {
                    d = if xm >= x { tol1 } else { -tol1 };
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        if !tried_parabolic {
            // golden-section
            e = if xm >= x { a - x } else { c - x };
            d = CGOLD * e;
        }
        let u = if d.abs() >= tol1 {
            x + d
        } else {
            x + if d >= 0.0 { tol1 } else { -tol1 }
        };
        let fu = cost(u);
        if fu <= fx {
            if u >= x {
                a = x;
            } else {
                c = x;
            }
            v = w;
            fv = fw;
            w = x;
            fw = fx;
            x = u;
            fx = fu;
        } else {
            if u < x {
                a = u;
            } else {
                c = u;
            }
            if fu <= fw || w == x {
                v = w;
                fv = fw;
                w = u;
                fw = fu;
            } else if fu <= fv || v == x || v == w {
                v = u;
                fv = fu;
            }
        }
    }
    Some((x, fx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn de_finds_quadratic_min() {
        // (x-3)^2 + (y+1)^2, min at (3,-1)
        let cost = |x: &[f64]| (x[0] - 3.0).powi(2) + (x[1] + 1.0).powi(2);
        let bounds = vec![(-10.0, 10.0), (-10.0, 10.0)];
        let x = differential_evolution(cost, &bounds, 15, 200, 1e-6, 42);
        assert!((x[0] - 3.0).abs() < 0.5, "x={:?}", x);
        assert!((x[1] + 1.0).abs() < 0.5, "x={:?}", x);
    }

    #[test]
    fn nm_finds_quadratic_min() {
        let cost = |x: &[f64]| (x[0] - 2.0).powi(2) + (x[1] - 5.0).powi(2);
        let x0 = vec![0.0, 0.0];
        let x = nelder_mead(cost, &x0, 0.5, 1e-8, 1e-8, 2000);
        assert!((x[0] - 2.0).abs() < 1e-4, "x={:?}", x);
        assert!((x[1] - 5.0).abs() < 1e-4, "x={:?}", x);
    }

    #[test]
    fn brute1_finds_min() {
        let cost = |x: f64| (x - 2.3).powi(2);
        let (x, f) = brute1(cost, 0.0, 5.0, 50);
        assert!((x - 2.3).abs() < 0.2, "x={x}");
        assert!(f < 0.05);
    }

    #[test]
    fn brent1_finds_min_precisely() {
        // (x-2.3456)^2 + 0.1·sin(x)；0.1·sin 项把极小点从 2.3456 偏移到 ~2.381。
        // 真实极小：2(x-2.3456) + 0.1·cos(x) = 0 → x ≈ 2.3818。
        let cost = |x: f64| (x - 2.3456).powi(2) + 0.1 * (x).sin();
        let res = brent1(cost, 0.0, 2.3, 5.0, 1e-8, 100);
        assert!(res.is_some());
        let (x, _) = res.unwrap();
        assert!((x - 2.3818).abs() < 1e-4, "x={x}");
    }

    #[test]
    fn brent1_invalid_bracket_returns_none() {
        let cost = |x: f64| (x - 4.0).powi(2); // 单调在 [0,1,2] 上递减 → bx 不是极小
        assert!(brent1(cost, 0.0, 1.0, 2.0, 1e-8, 100).is_none());
    }
}
