//! Nelder-Mead 无导数优化器。
//!
//! 在无约束空间最小化 cost_of_vec。原版 univariate fit1 用它做局部精修。

use crate::{cost::univariate_cost_sufficient, data::UnivariateSufficient};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

/// 无约束向量 → 真实参数 → cost。
///
/// `x` 是优化器搜索的坐标，处在无约束空间 (−∞, +∞)，三个分量为：
///   - `x[0]` = `log(sig2_zero)`  → 截断方差 σ²_zero (>0)
///   - `x[1]` = `log(sig2_beta)`  → 效应方差 σ²_β (>0)
///   - `x[2]` = `logit(pi)`       → causal 比例 π ∈ (0,1)
///
/// 这个函数是优化器与 cost 之间的"翻译官"：优化器递来一个无约束 `x`，
/// 经 `from_unconstrained` 还原成有物理约束的 `UnivariateParams`，
/// 再喂给 cost function。这就是 parametrize 模块存在的全部意义。
pub(crate) fn cost_of_vec(data: &UnivariateSufficient, x: [f64; 3]) -> f64 {
    let params = crate::parametrize::from_unconstrained(x);
    univariate_cost_sufficient(data, &params)
}

const ALPHA: f64 = 1.0; // 反射
const GAMMA: f64 = 2.0; // 扩张
const RHO: f64 = 0.5; // 收缩
const SIGMA: f64 = 0.5; // 缩边

/// nelder_mead 无导数优化器
///
/// 对于univariate MiXeR，需要调节的参数有3个，需要定点数为4个
pub fn nelder_mead(
    data: &UnivariateSufficient,
    x0: [f64; 3],
    step: f64,
    tol: f64,
    max_iter: usize,
) -> [f64; 3] {
    // 1. 构造初始单纯形：x0 加上每个坐标方向偏移 step 的点
    let mut simplex: Vec<([f64; 3], f64)> = (0..=3)
        .map(|i| {
            let mut x = x0;
            if i > 0 {
                x[i - 1] += step;
            }
            (x, cost_of_vec(data, x))
        })
        .collect();

    for _ in 0..max_iter {
        // 2. 按 cost 升序排（最好在前）
        simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        // 3. 收敛判定：最好与最差 cost 差距足够小
        if simplex[3].1 - simplex[0].1 < tol {
            break;
        }

        // 4. 重心：除了最差点外的平均
        let centroid = {
            let mut c = [0.0; 3];
            for vertex in simplex.iter().take(3) {
                for (d, sum) in c.iter_mut().enumerate() {
                    *sum += vertex.0[d];
                }
            }
            [c[0] / 3.0, c[1] / 3.0, c[2] / 3.0]
        };

        let worst = simplex[3].0;
        let best_cost = simplex[0].1;
        let second_worst_cost = simplex[2].1;

        // 5. 反射: x_r = centroid + α·(centroid − worst)
        let xr = [
            centroid[0] + ALPHA * (centroid[0] - worst[0]),
            centroid[1] + ALPHA * (centroid[1] - worst[1]),
            centroid[2] + ALPHA * (centroid[2] - worst[2]),
        ];
        let fr = cost_of_vec(data, xr);

        let new_point = if fr < best_cost {
            // 比最好还好 → 扩张
            let xe = [
                centroid[0] + GAMMA * (xr[0] - centroid[0]),
                centroid[1] + GAMMA * (xr[1] - centroid[1]),
                centroid[2] + GAMMA * (xr[2] - centroid[2]),
            ];
            let fe = cost_of_vec(data, xe);
            if fe < fr { (xe, fe) } else { (xr, fr) }
        } else if fr < second_worst_cost {
            // 比次差好 → 接受反射
            (xr, fr)
        } else {
            // 比次差还差 → 收缩
            let dir = if fr < simplex[3].1 { xr } else { worst };
            let xc = [
                centroid[0] + RHO * (dir[0] - centroid[0]),
                centroid[1] + RHO * (dir[1] - centroid[1]),
                centroid[2] + RHO * (dir[2] - centroid[2]),
            ];
            let fc = cost_of_vec(data, xc);
            if fc < simplex[3].1 {
                (xc, fc)
            } else {
                // 收缩也无效 → 缩边：所有点向最好点靠拢
                let best = simplex[0].0;
                for point in simplex.iter_mut().take(4).skip(1) {
                    let xs = [
                        best[0] + SIGMA * (point.0[0] - best[0]),
                        best[1] + SIGMA * (point.0[1] - best[1]),
                        best[2] + SIGMA * (point.0[2] - best[2]),
                    ];
                    *point = (xs, cost_of_vec(data, xs));
                }
                continue; // 跳过下面的"替换最差点"
            }
        };

        simplex[3] = new_point; // 替换最差点
    }
    simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    simplex[0].0
}

/// 从种群中选 3 个互不相同且 != exclude 的索引（Fisher-Yates 抽前 3）。
fn pick_three(rng: &mut SmallRng, exclude: usize, popsize: usize) -> (usize, usize, usize) {
    let mut idxs: Vec<usize> = (0..popsize).filter(|&x| x != exclude).collect();
    for k in 0..3 {
        let j = rng.gen_range(k..idxs.len());
        idxs.swap(k, j);
    }
    (idxs[0], idxs[1], idxs[2])
}

/// 差分进化（DE/rand/1/bin），对齐原版 scipy `differential_evolution` 默认行为。
///
/// 原版 `diffevo-fast` 参数：`mutation=(0.5,1)`（F 在 [0.5,1] 每代抖动）、
/// `recombination=0.7`（交叉率 CR）、`tol=0.01`、`polish=False`。
/// 在无约束空间搜索，`bounds` 为每维 (low, high)。
pub fn differential_evolution(
    data: &UnivariateSufficient,
    bounds: &[(f64, f64)],
    popsize_mult: usize,
    max_gen: usize,
    tol: f64,
    seed: u64,
) -> [f64; 3] {
    let mut rng = SmallRng::seed_from_u64(seed);
    let n = bounds.len();
    let popsize = popsize_mult * n;
    const CR: f64 = 0.7; // 交叉率

    // 1. 初始化种群：每维在边界内随机
    let mut pop: Vec<[f64; 3]> = (0..popsize)
        .map(|_| {
            let mut ind = [0.0f64; 3];
            for d in 0..n {
                ind[d] = rng.gen_range(bounds[d].0..bounds[d].1);
            }
            ind
        })
        .collect();

    let mut costs: Vec<f64> = pop.iter().map(|x| cost_of_vec(data, *x)).collect();

    for _ in 0..max_gen {
        // 每代抖动一个突变因子 F ∈ [0.5, 1.0]
        let f = rng.gen_range(0.5..1.0);

        for i in 0..popsize {
            let (a, b, c) = pick_three(&mut rng, i, popsize);

            // 变异：mutant = a + F·(b − c)，裁剪到边界
            let mut mutant = [0.0f64; 3];
            for d in 0..n {
                mutant[d] =
                    (pop[a][d] + f * (pop[b][d] - pop[c][d])).clamp(bounds[d].0, bounds[d].1);
            }

            // 交叉（二项式）：保证至少一维来自 mutant
            let j_rand = rng.gen_range(0..n);
            let mut trial = [0.0f64; 3];
            for d in 0..n {
                trial[d] = if d == j_rand || rng.gen_range(0.0..1.0) < CR {
                    mutant[d]
                } else {
                    pop[i][d]
                };
            }

            // 选择：trial 更好则替换
            let cost_trial = cost_of_vec(data, trial);
            if cost_trial <= costs[i] {
                pop[i] = trial;
                costs[i] = cost_trial;
            }
        }

        // 收敛判定：种群 cost 极差 < tol
        let cmin = costs.iter().cloned().fold(f64::INFINITY, f64::min);
        let cmax = costs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if cmax - cmin < tol {
            break;
        }
    }

    let (best_idx, _) = costs
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
        .unwrap();
    pop[best_idx]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::UnivariateParams;

    #[test]
    fn nelder_mead_finds_minimum() {
        // 简单验证:优化后 cost 不比起始点差
        use crate::data::{ChromData, UnivariateSufficient};
        let triples = vec![(0, 1, 0.5), (1, 0, 0.3)];
        let data = ChromData::new(vec![1.0, 0.8], vec![100.0, 100.0], vec![0.5, 0.4], &triples);
        let suff = UnivariateSufficient::from_chrom_data(&data);

        let x0 = [0.0, -7.0, -5.0]; // 某个起始点
        let cost_before = cost_of_vec(&suff, x0);
        let x_best = nelder_mead(&suff, x0, 0.5, 1e-7, 2000);
        let cost_after = cost_of_vec(&suff, x_best);

        assert!(cost_after <= cost_before, "优化后 cost 应不大于起始");
        let params = crate::parametrize::from_unconstrained(x_best);
        println!(
            "拟合参数: pi={}, sig2_beta={}, sig2_zero={}",
            params.pi, params.sig2_beta, params.sig2_zero
        );
    }
}
