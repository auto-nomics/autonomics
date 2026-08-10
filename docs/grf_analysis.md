# grf (Generalized Random Forests) 移植分析

> 参考代码: `reference/grf/` (R + C++, grf 2.6.1, GPL-3)
> 用途: 为 autonomics DAG 引擎设计 grf 的节点封装方案
> 关键问题: **能否直接调用 C++ core, DAG 节点仅做调度?**

---

## 1. grf 包功能总览

### 1.1 算法/模型族 (R `*.forest()` + C++ `*ForestTrainer`)

| Forest 类型 | 估计量 | 输入 | C++ Trainer | 状态 |
|---|---|---|---|---|
| `regression_forest` | μ(x) = E[Y\|X=x] | X, Y | `regression_trainer` | baseline |
| `causal_forest` | τ(x) = E[Y(1)-Y(0)\|X=x] (R-learner) | X, Y, W (+Y.hat/W.hat) | `regression_trainer` + 残差化 | 最常用 |
| `instrumental_forest` | τ(x) = Cov[Y,Z\|X]/Cov[W,Z\|X] | X, Y, W, Z (+Y.hat/W.hat/Z.hat) | `instrumental_trainer` | IV/2SLS |
| `causal_survival_forest` | RMST 或生存概率差, 右删失 | X, Y(time), W, D(event) | `causal_survival_trainer` | 时变事件 |
| `multi_arm_causal_forest` | K 个 arm 的对比 τ_k(x) | X, Y, W∈{1..K} | `multi_causal_trainer(K, M)` | 多处理/多结果 |
| `multi_regression_forest` | μ_j(x) = E[Y_j\|X=x], j=1..M | X, Y (M 列) | `multi_regression_trainer(M)` | 多任务回归 |
| `quantile_forest` | F_{Y\|X}(q) 的 q 分位数 | X, Y, quantiles= | `quantile_trainer(q)` | 分位数 |
| `probability_forest` | P[Y=k\|X=x] | X, Y (factor) | `probability_trainer(K)` | 分类 |
| `survival_forest` | S(t,x) = P[T>t\|X=x] | X, Y(time), D | `survival_trainer` | 生存函数 |
| `lm_forest` | 条件线性模型 h_k(x) | X, Y, W | `regression_trainer` + 残差化 | CLM |
| `ll_regression_forest` | 局部线性回归 μ(x) | X, Y | `ll_regression_trainer` | LLF |
| `boosted_regression_forest` | 多步 boosting 的 μ(x) | X, Y | 多次 `regression_trainer` | 偏差降低 |

每个 forest 对应一组 C++ 类:
- `ForestTrainer` (Trainers.h 工厂) → `forest::Forest`
- `ForestPredictor` (Predictors.h 工厂) → `vector<Prediction>`

### 1.2 分析/汇总函数 (纯 R, 围绕训练好的 forest)

| 函数 | 作用 | 是否依赖 C++ |
|---|---|---|
| `predict.forest_type` | 推断 (含 OOB / 新数据 / variance) | ✅ `*_predict` bindings |
| `average_treatment_effect` | ATE/AIPW/TMLE, target.sample ∈ {all, treated, control, overlap} | ✅ `get_scores` |
| `best_linear_projection` | τ(x) 对特征 A 的最优线性投影 | ✅ `get_scores` |
| `test_calibration` | 校准 omnibus 检验 (`sandwich::vcovCL`) | ❌ 纯 R + lm |
| `rank_average_treatment_effect` | RATE / TOC 曲线 + bootstrap SE | ✅ `get_scores` + `boot_grf` |
| `rank_average_treatment_effect.fit` | 同上, 用户自备 DR scores | ❌ 纯 R + `boot_grf` |
| `get_scores` | 各种 forest 的 DR scores | ✅ 调用 C++ predict + 必要时 auxiliary regression forest |
| `merge_forests` | 合并 N 个 forest (大森林分批训练) | ❌ `Forest::merge` 已通过 deserialization 完成 |
| `get_tree` | 抽取单棵树 (R 列表) | ❌ 纯 R 序列化 |
| `get_leaf_node` | 推断某棵树的叶节点 id | ❌ 纯 R (重写 C++ find_leaf_node) |
| `get_forest_weights` | 邻接权重 α(x) (稀疏矩阵) | ✅ `compute_weights` |
| `split_frequencies` | 每个变量在不同深度的分裂频次 | ✅ `compute_split_frequencies` |
| `variable_importance` | 加权 split 频次 (衰减权重) | ❌ 纯 R |
| `grf_options` | 全局选项 (legacy_seed, verbose) | ❌ |
| `generate_causal_data` | 模拟数据 (DGP) | ❌ 纯 R |
| `generate_causal_survival_data` | 模拟数据 | ❌ 纯 R |

### 1.3 训练通用参数 (几乎所有 forest 都接受)

`num_trees`, `sample_weights`, `clusters`, `equalize_cluster_weights`,
`sample_fraction`, `mtry`, `min_node_size`, `honesty`, `honesty_fraction`,
`honesty_prune_leaves`, `alpha`, `imbalance_penalty`, `ci_group_size`,
`tune_parameters` (+ `tune_num_trees/reps/draws`), `compute_oob_predictions`,
`num_threads`, `seed`

---

## 2. C++ core 架构 (grf 2.6.1)

### 2.1 目录布局
```
core/src/
├── commons/        Data, globals, ProgressBar, utility
├── forest/         Forest, ForestOptions, ForestTrainer{,s}, ForestPredictor{,s}
├── tree/           Tree, TreeTrainer, TreeOptions
├── prediction/     11 种 PredictionStrategy (含 collector/)
├── splitting/      8 种 SplittingRule + factory/
├── relabeling/     9 种 RelabelingStrategy (R-learner 残差化核心)
├── sampling/       RandomSampler (mt19937_64), SamplingOptions
├── analysis/       SplitFrequencyComputer
├── RuntimeContext  forest_name + verbose_stream + interrupt_handler
└── third_party/    Eigen 3.4.0 (vendored), random/, tqdm/
```

### 2.2 C++ 抽象边界 (完全可以脱离 Rcpp 调用)

```cpp
// 训练
grf::ForestTrainer trainer = grf::regression_trainer();
grf::Data data(double_ptr, num_rows, num_cols);  // 仅借指针, 不拷贝
data.set_outcome_index(idx);
grf::ForestOptions options(num_trees, ci_group_size, sample_fraction, mtry,
                           min_node_size, honesty, honesty_fraction,
                           honesty_prune_leaves, alpha, imbalance_penalty,
                           num_threads, seed, legacy_seed,
                           std::vector<size_t>(clusters), samples_per_cluster);
grf::Forest forest = trainer.train(data, options);

// 预测
grf::ForestPredictor predictor = grf::regression_predictor(num_threads);
std::vector<grf::Prediction> preds = predictor.predict(forest, train, test, estimate_variance);

// 序列化 (类似 RcppUtilities::serialize_forest, 但写到字节流而非 Rcpp::List)
```

**关键事实**:
- `RcppUtilities.{h,cpp}` 是**唯一**把 Rcpp 类型 ↔ grf core 类型相互转换的胶水, 24 个 C++ 函数, 不到 250 行.
- `RuntimeContext` 已经为多语言绑定设计: `verbose_stream`(任意 `ostream*`)、`interrupt_handler`(`std::function<void()>`), **可以零侵入地接入 Rust**:
  ```cpp
  runtime_context.verbose_stream = &std::cerr;
  runtime_context.interrupt_handler = [](){ check_rust_abort_flag(); };
  ```

### 2.3 唯一非标准依赖

- **Eigen 3.4.0** (vendored) — 用于向量化的 prediction strategy (multi_causal, causal_survival 等)
- **C++17**, pthread
- **Rcpp** 只在 `r-package/grf/src/` 出现, **core/ 不依赖 Rcpp**, 可以独立编译

---

## 3. 能否直接调用 C++ core? DAG 节点仅做调度?

### ✅ 结论: 可以, 且强烈推荐这样做

| 评估维度 | 判断 | 说明 |
|---|---|---|
| 算法保真度 | ✅ 完美 | 调用 grf 官方 C++ 实现, 与 R 包 bit-identical |
| 性能 | ✅ 与 R 包持平或更好 | Eigen 已向量化, 自身用 mt19937_64+pthread; 我们省掉 R 序列化 |
| 开发成本 | ✅ 极低 | 写 12 个 `extern "C"` 封装 + Rust FFI, 不重写算法 |
| 维护成本 | ✅ 极低 | grf 升级时只需重新 build `.so` |
| DAG 化 | ✅ 自然 | 每个 forest 类型 → 一个 DAG 节点, 参数→输入端口, forest 自身→可序列化 binary blob |
| 跨语言调用 | ⚠️ 中等 | 需要 FFI, 但 core 已经是无 Rcpp 依赖, 比 LDSC/BGMG 干净 |
| **许可证** | ⚠️ **需要确认** | grf 是 **GPL-3**, DAG 节点 binary 链接 grf 后是 GPL-3 (传染), 但 DAG 节点本身可以独立发布; autonomics 项目使用闭源 DAG 时需要法务评估 |

### 3.1 推荐架构: grf-sys + grf DAG nodes

```
crates/
├── grf-sys/                     # build.rs + bindgen, 产出 libgrf.a + Rust 绑定
│   ├── build.rs                 # CMake build grf/core → staticlib
│   ├── src/lib.rs               # ForestTrainer / ForestPredictor / Data 包装
│   └── wrapper/                 # 1 个 grf_shim.cpp: extern "C" ABI
│
└── grf/                         # DAG 节点实现 (Apache-2.0 / 项目许可)
    ├── src/lib.rs               # 注册 grf_xxx 节点
    └── src/nodes/
        ├── regression_forest.rs
        ├── causal_forest.rs     # 内部 R-learner: 自动训练 Y.hat/W.hat
        ├── instrumental_forest.rs
        ├── causal_survival_forest.rs
        ├── multi_arm_causal_forest.rs
        ├── multi_regression_forest.rs
        ├── quantile_forest.rs
        ├── probability_forest.rs
        ├── survival_forest.rs
        ├── lm_forest.rs
        ├── ll_regression_forest.rs
        ├── boosted_regression_forest.rs
        ├── merge_forests.rs
        ├── predict_forest.rs    # 通用 (泛型 predict 节点, 接任意 forest)
        ├── average_treatment_effect.rs
        ├── best_linear_projection.rs
        ├── test_calibration.rs  # 纯 R-equivalent: 走 statkit OLS
        ├── rank_average_treatment_effect.rs
        ├── get_scores.rs
        ├── get_forest_weights.rs
        ├── split_frequencies.rs
        ├── variable_importance.rs
        └── get_tree.rs          # 序列化为 JSON / DAG 节点元数据
```

### 3.2 C ABI 设计 (grf-sys wrapper 思路)

把 `RcppUtilities` 的 Rcpp 胶水**重写**为不带 Rcpp 的版本 (参考 grf 已有的 `R/` 路径):

```cpp
// grf_shim.cpp — extern "C" + C ABI, 避免 C++ name mangling + 简化 bindgen
extern "C" {

// Forest 训练 (返回 opaque handle + forest 二进制)
grf_forest_t* grf_train_regression(
    const double* data, size_t n_rows, size_t n_cols,
    size_t outcome_index, const double* weights, bool use_weights,
    uint32_t mtry, uint32_t num_trees, uint32_t min_node_size,
    double sample_fraction, bool honesty, double honesty_fraction,
    bool honesty_prune_leaves, uint32_t ci_group_size,
    double alpha, double imbalance_penalty,
    const size_t* clusters, size_t n_clusters, uint32_t samples_per_cluster,
    uint32_t num_threads, uint32_t seed, bool legacy_seed,
    bool compute_oob, bool verbose);

grf_predictions_t* grf_predict(
    const grf_forest_t* forest, const double* test_data,
    size_t n_test_rows, size_t n_test_cols,
    bool estimate_variance, uint32_t num_threads, bool verbose);

void grf_forest_free(grf_forest_t*);
void grf_predictions_free(grf_predictions_t*);

// 二进制序列化 (写到字节流, 用于 DAG 节点间传递 forest)
uint8_t* grf_serialize_forest(const grf_forest_t*, size_t* out_len);
grf_forest_t* grf_deserialize_forest(const uint8_t* bytes, size_t len);

// 其他算子
grf_split_freq_t* grf_compute_split_frequencies(
    const grf_forest_t*, size_t max_depth);
double* grf_compute_weights(const grf_forest_t*, ...);  // 邻接权重 α

} // extern "C"
```

**Forest 序列化格式** (从 `RcppUtilities::serialize_forest` 移植到二进制):
```
header:   { ci_group_size, num_variables, num_trees, pv_num_types }
per tree: { root_node, child_nodes[2*N], leaf_samples[], split_vars[N],
            split_values[N], drawn_samples[], send_missing_left[N], pv_values[] }
```
约 250 行 C++, 与 grf 现有实现对齐, 字节序/对齐由我们自己定 (建议 little-endian, 显式长度前缀).

### 3.3 DAG 节点接口 (Rust 端)

每个 forest 训练节点长这样 (举 `causal_forest` 为例):

```rust
pub struct CausalForestArgs {
    pub x: DataFramePort,                // X (n×p)
    pub y: ArrayPort,                    // Y (n)
    pub w: ArrayPort,                    // W (n, 可选连续)
    pub y_hat: Option<ArrayPort>,        // 用户给定或 None→自动训练
    pub w_hat: Option<ArrayPort>,
    pub num_trees: u32,
    pub mtry: Option<u32>,               // None → ceil(sqrt(p) + 20)
    pub min_node_size: u32,
    pub honesty: bool,
    pub honesty_fraction: f64,
    pub alpha: f64,
    pub imbalance_penalty: f64,
    pub ci_group_size: u32,              // 1 = 关掉 variance estimate
    pub compute_oob: bool,
    pub num_threads: Option<u32>,
    pub seed: u64,
    pub sample_weights: Option<ArrayPort>,
    pub clusters: Option<ArrayPort>,
    pub stabilize_splits: bool,
}

pub struct CausalForestOutput {
    pub forest: ForestBlob,              // 二进制 grf forest
    pub y_hat: ArrayPort,                // 训练时估计的 E[Y|X]
    pub w_hat: ArrayPort,                // 训练时估计的 E[W|X]
    pub oob_predictions: Option<ArrayPort>,
    pub tuning_output: Option<TuningOutput>,
}

impl DagNode for CausalForestNode {
    fn run(&self, ctx: &mut Context) -> Result<CausalForestOutput> {
        let x = ctx.table(&self.args.x)?;
        let (y_centered, w_centered, y_hat, w_hat) = if self.args.y_hat.is_none() {
            // R-learner 第一步: 自动跑 regression_forest 估计 Y.hat/W.hat
            let y_hat = self.sub_train_regression(&x, &y)?;
            let w_hat = self.sub_train_regression(&x, &w)?;
            (y - &y_hat, w - &w_hat, y_hat, w_hat)
        } else {
            // 用户已给
            (y - y_hat.as_ref().unwrap(),
             w - w_hat.as_ref().unwrap(),
             self.args.y_hat.clone().unwrap(),
             self.args.w_hat.clone().unwrap())
        };
        // 拼成 grf 列优先矩阵, 调 grf_train
        let forest = grf::train_causal(
            &x, &y_centered, &w_centered, &self.args)?;
        Ok(CausalForestOutput { forest, y_hat, w_hat, ... })
    }
}
```

### 3.4 与本仓库已有 pattern 的对照

| 已有 crate | grf 适用性 | 借鉴要点 |
|---|---|---|
| `ldsc-sys` (`build.rs` + Eigen, 静态库 + bindgen) | ✅ grf 也是 Eigen+CMake | 复用同一套 build 脚手架 |
| `mtag` 节点 (Iceberg 表 + DAG 端口 + serialized model blob) | ✅ forest 本身就是个 blob | DAG 节点间以 `ForestBlob` 传递训练结果 |
| `magma` (`test_data` 在 aliyun) | ✅ grf 也需要 fixture | `aliyun://autonomics-data/grf/test-data/` 归档 |
| `coloc` (R 交叉验证 golden tests) | ✅ grf 全部 12 个 forest 都需要 | `grf_xxx` 节点对每种 forest 跑 R grf 同输入 → bit-equal |

### 3.5 与 R 端交叉验证策略

1. **golden tests** 对每个 forest 类型, 用 R `grf` 包产出 reference output (预测值、OOB error、variable_importance 等), 在 `fixtures/` 存为 CSV/parquet.
2. Rust 节点用同一 seed 跑, 比较到 1e-12 (Eigen 行为与 R 一致, 没有浮点漂移).
3. **differential tests** 对 `causal_forest`, 验证 R-learner 第一步的子 forest (Y.hat, W.hat) 与单独 `regression_forest(X, Y)` 输出一致.

---

## 4. DAG 节点详表

> 端口命名遵循已有 crate 约定 (DataFrame/Array/Option/Spec).

### 4.1 Forest 训练节点 (12 个)

| 节点 | 输入端口 (DataFrame / Array / Spec) | 输出端口 | 备注 |
|---|---|---|---|
| `grf_regression_forest` | x, y, [weights], [clusters], forest_params | forest, [oob_pred], [tuning] | baseline |
| `grf_causal_forest` | x, y, w, [y_hat], [w_hat], forest_params, [stabilize_splits] | forest, [y_hat], [w_hat], [oob_pred] | 内部自动 R-learner 第一步 |
| `grf_instrumental_forest` | x, y, w, z, [y_hat], [w_hat], [z_hat], forest_params, reduced_form_weight, [stabilize_splits] | forest, [y_hat], [w_hat], [z_hat] | |
| `grf_causal_survival_forest` | x, time, w, d, [w_hat], target (RMST/SP), horizon, [failure_times], forest_params | forest, [w_hat] | |
| `grf_multi_arm_causal_forest` | x, y (M 列), w (factor), [gradient_weights], forest_params, [stabilize_splits] | forest | num_treatments 自动从 W.factor levels 推断 |
| `grf_multi_regression_forest` | x, y (M 列), forest_params | forest | |
| `grf_quantile_forest` | x, y, quantiles (Vec<f64>), [regression_splitting] | forest | quantiles 默认 (0.1, 0.5, 0.9) |
| `grf_probability_forest` | x, y (factor), forest_params | forest | num_classes 自动 |
| `grf_survival_forest` | x, time, d, [failure_times], forest_params | forest | |
| `grf_lm_forest` | x, y, w (K 列), [y_hat], [w_hat], [gradient_weights], forest_params | forest | |
| `grf_ll_regression_forest` | x, y, [enable_ll_split], ll_split_*, forest_params | forest | |
| `grf_boosted_regression_forest` | x, y, [boost_steps] / 自动选择, forest_params | forest | 内部循环 train regression + OOB residual |

通用 forest_params (Spec):
```yaml
num_trees: 2000          # int
sample_fraction: 0.5     # float
mtry: null               # int? → 自动 sqrt(p)+20
min_node_size: 5         # int
honesty: true            # bool
honesty_fraction: 0.5    # float
honesty_prune_leaves: true # bool
alpha: 0.05              # float
imbalance_penalty: 0.0   # float
ci_group_size: 2         # int (1=无 variance)
compute_oob: true        # bool
tune_parameters: "none"  # enum[none|all|subset]
tune_num_trees: 50       # int
tune_num_reps: 100       # int
tune_num_draws: 1000     # int
num_threads: null        # int?
seed: null               # int?
equalize_cluster_weights: false # bool
```

### 4.2 通用 predict 节点 (1 个泛型)

```
grf_predict_forest
  in:  forest (ForestBlob), new_x (DataFrame), [linear_correction_vars], 
       [ll_lambda], [ll_weight_penalty], [estimate_variance]
  out: predictions (Array), [variance_estimates] (Array), 
       [debiased_error] (Array), [excess_error] (Array)
```
OOB 预测模式: `new_x = None` → 走 C++ `predict_oob`.
forest 类型在 `ForestBlob` header 里编码 (`u8` magic), 节点根据 magic 分派到对应 predictor.

### 4.3 Forest 操作 / 分析节点

| 节点 | in | out | 备注 |
|---|---|---|---|
| `grf_merge_forests` | forest_list (Vec<ForestBlob>) | forest | 大森林分批 |
| `grf_average_treatment_effect` | forest, target_sample, method (AIPW/TMLE), [subset], [debiasing_weights], [compliance_score] | estimate (f64), std_err (f64), (or contrast × outcome DataFrame) | 走 C++ get_scores + statkit OLS/sandwich |
| `grf_best_linear_projection` | forest, [A] (协变量), [subset], target_sample | coef 估计 + HC SE | statkit::ols + sandwich |
| `grf_test_calibration` | forest, vcov_type | HC 检验 (mean.pred & diff.pred 系数 + p) | statkit |
| `grf_rank_average_treatment_effect` | forest, priorities (Array or 2-col), target (AUTOC/QINI), q (Vec<f64>), R (bootstrap reps), [subset], [debiasing_weights] | estimate, std_err, target, TOC (DataFrame q × priority) | 内部: get_scores + cluster bootstrap |
| `grf_get_scores` | forest, [subset], [debiasing_weights], [compliance_score], [num_trees_for_weights] | DR scores (Array, 或 multi_arm: 3D Array) | |
| `grf_get_forest_weights` | forest, [new_x] | 稀疏邻接权重矩阵 (sparse::CsMat) | |
| `grf_split_frequencies` | forest, max_depth | depth × p DataFrame | |
| `grf_variable_importance` | forest, decay_exponent, max_depth | Array<f64> (length p) | |
| `grf_get_tree` | forest, index (usize) | TreeBlob (DAG 端口, 可序列化) | JSON/RON 输出 |

### 4.4 模拟数据节点 (2 个)

| 节点 | in | out | 备注 |
|---|---|---|---|
| `grf_generate_causal_data` | n, p, 参数 | DataFrame X/Y/W, 真实 τ(x) | 纯 R-等价 (Rust 重写) |
| `grf_generate_causal_survival_data` | n, p, 参数, horizon | DataFrame X/Y(time)/W/D | 纯 R-等价 |

### 4.5 全局 / 工具节点

| 节点 | in | out | 备注 |
|---|---|---|---|
| `grf_options` | legacy_seed, verbose | (无输出, 写全局) | 单例节点, 写进程级 grf::runtime_context |

---

## 5. 与已有 DAG 节点的复用

- **`mtag` 的 forest blob 模式**: forest 本身作为不可变 binary blob 在 DAG 节点间传递, 训练节点和 predict 节点解耦. grf 完全沿用.
- **`statkit` 的 OLS / sandwich SE**: `best_linear_projection`, `test_calibration`, `average_treatment_effect` 的 TMLE / overlap 路径都依赖 R 端的 `lm()` + `sandwich::vcovCL`. 走 statkit 已经验证过的实现.
- **`ldsc-sys` 的 build 脚手架**: Eigen + CMake + bindgen 的脚手架直接复用, grf-sys 只是 CMake target 不同.
- **`hierint` 的多输出 DAG node**: grf 的 forest 输出同时含 `oob_pred` / `tuning_output`, 沿用多输出节点 pattern.

---

## 6. 不直接复用的部分 (需要 Rust 重写)

仅以下 5 个分析/工具函数**值不值得**走 FFI, 直接 Rust 实现更轻:

1. `grf_options` (单例 setter)
2. `variable_importance` (10 行 R = 纯矩阵乘法)
3. `get_tree` 的 R-列表 → RON 转换 (ForestBlob→TreeBlob, 复用 `RcppUtilities::serialize_forest` 的二进制输出即可, 无需重写)
4. `generate_causal_data` / `generate_causal_survival_data` (DGP, 无 C++)
5. `test_calibration` / `best_linear_projection` 的 R 端 lm/sandwich 部分 (走 statkit)

`get_leaf_node` (纯 R 重写 C++ `Tree::find_leaf_node`) 也可以 Rust 重写, 但不暴露为 DAG 节点 — 它只用于 `get_tree` 之后的人类可读树形 dump, 调试用.

---

## 7. 工作分解建议 (基于以上分析)

| 阶段 | 内容 | 产出 | 估时 |
|---|---|---|---|
| P0 | `grf-sys`: CMake build grf/core → `libgrf.a`; `wrapper/grf_shim.cpp` 实现 12 个 forest + predict + serialize; Rust FFI 绑定 (bindgen) | `grf-sys` crate, 单元测试 (调通 1 个 forest) | 3 d |
| P1 | `grf::regression_forest` + `grf::predict_forest` 节点 + golden test (R `grf::regression_forest` 同输入) | 1 个 node + 1 个 golden | 1 d |
| P2 | `grf::quantile_forest`, `probability_forest`, `survival_forest`, `multi_regression_forest` | 4 nodes + 4 golden | 2 d |
| P3 | `grf::causal_forest` (含 R-learner 内部两阶段) + `grf::average_treatment_effect` + `grf::best_linear_projection` + `grf::test_calibration` | 4 nodes + golden | 3 d |
| P4 | `grf::instrumental_forest`, `grf::lm_forest`, `grf::ll_regression_forest`, `grf::boosted_regression_forest` | 4 nodes + golden | 3 d |
| P5 | `grf::causal_survival_forest`, `grf::multi_arm_causal_forest`, `grf::rank_average_treatment_effect`, `grf::get_scores`, `grf::get_forest_weights`, `grf::split_frequencies`, `grf::variable_importance`, `grf::get_tree`, `grf::merge_forests` | 8 nodes + golden | 4 d |
| P6 | `grf::generate_causal_data`, `grf::generate_causal_survival_data` (纯 Rust DGP) | 2 nodes | 1 d |
| P7 | 文档 + Agent tools (grf_forest_train / grf_predict / grf_ate / grf_calibration ...) | doc + tools | 1 d |

总计 ~18 工作日 (一个人).

---

## 8. 风险与决策点

1. **许可证 (GPL-3)**: 静态链接 grf core 后, `grf-sys` 的输出 .a 是 GPL-3. 如果 `autonomics` 整体是 Apache-2.0/MIT, 解决方案: 把 `grf-sys` 作为独立 GPL-3 crate 在用户机器上 build, 或者用 LGPL 兼容的策略 (load-time dynamic linking + 命令行 `grf_xxx`). **需要法务 early-on 决策**.
2. **Eigen**: 已经在 `ldsc-sys` 等用过, 无新风险.
3. **Forest 二进制兼容性**: grf 升级后序列化格式可能变, 锁定 2.6.1 版本, 在 `grf-sys/build.rs` 中 pin version.
4. **大森林 OOM**: grf 1M × 30 float forest ≈ 25 GB. DAG 节点应支持 `forest` 端口流式写出到 Iceberg / parquet, 而不是塞进 DAG 内存.
5. **节点间 seed 一致性**: DAG 节点并行时, 每个节点内部 seed 来自 spec, 不要依赖全局 RNG. (grf 2.6.1 已经用 `runtime_context.random_seed` 显式 seed, OK.)

---

## 9. 回答原问题

> 能否直接调用 C++ 源码, DAG 节点仅做调度?

**可以, 而且这是最合理的方案**:

- grf 的 C++ core (`core/src/`) **完全独立于 Rcpp**, 已经是为多语言绑定设计的:
  - `RuntimeContext` 提供 `verbose_stream` / `interrupt_handler` 注入点
  - `Data` 只借用外部指针 (`const double*`), 不拷贝
  - `Forest`/`Prediction` 都有完整序列化路径
- 唯一需要做的胶水: 写一个 `grf_shim.cpp` (~250 行 C ABI, 不带 Rcpp) + Rust `bindgen` 绑定
- 12 个 forest 训练节点 + 1 个通用 predict 节点 + ~15 个分析节点 = 整个 grf R API 的完整 DAG 化
- 时间投入 ~18 工作日 vs 全 Rust 重写估计 ~3-4 个月, 且能保证与 R 包 bit-identical

唯一前置条件: **确认 GPL-3 兼容性**.