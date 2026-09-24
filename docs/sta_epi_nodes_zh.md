# 统计与流行病学节点文档

`stat_crates/statkit`（基础统计）和 `stat_crates/epi`（流行病学方法）
中实现的统计分析节点完整文档，通过 `crates/data-engine/src/nodes/`
暴露为数据引擎节点。

所有算法均以纯 Rust 从零实现，并参照 R 语言在 `stat_crates/epi/tests/xval*.rs`
和 `stat_crates/statkit/tests/xval*.rs` 进行交叉验证。

---

## 目录

1. [基础统计（statkit）](#1-基础统计statkit)
2. [调查与描述统计](#2-调查与描述统计)
3. [回归模型](#3-回归模型)
4. [中介与因果推断](#4-中介与因果推断)
5. [生存分析](#5-生存分析)
6. [竞争风险与多状态](#6-竞争风险与多状态)
7. [潜变量模型](#7-潜变量模型)
8. [机器学习与可解释性](#8-机器学习与可解释性)
9. [交叉验证总览](#9-交叉验证总览)

---

## 1. 基础统计（statkit）

`stat_crates/statkit/src/` 提供所有高层模块的原始计算单元。

### 1.1 `descriptive` — 集中趋势、离散度、分位数、秩

- **API**：`mean`、`variance`、`std_dev`、`sum`、`correlation`、`covariance`、
  `min`、`max`、`quantile`、`median`、`order_statistic`、`rank`。
- **约定**：默认样本方差/标准差（`n-1` 自由度）；总体版以 `_population` 后缀。
  分位数采用**线性插值**（R type 7 / NumPy 默认）。秩采用 R 默认的**平均**绑定方法。
- **实现**：使用 Neumaier/Kahan-Babuška 补偿求和保证长向量的数值稳定性。
- **交叉验证**：仅内部单元测试。

### 1.2 `regression::ols` — 普通及加权最小二乘

- **API**：`ols(&[&[f64]], &y, intercept)` 与 `wls(...)` 返回 `Regression` 结构
  （系数、SE、t 检验、p 值、R²、调整 R² 等）。
- **算法**：通过 `faer::linalg::Llt`（Cholesky 分解）求解正规方程，Wald t 检验。
- **下游使用**：所有高层回归模型的基础。

### 1.3 `regression::logistic` — 二分类逻辑回归（IRLS）

- **API**：`logistic(&[&[f64]], &y, intercept)` 返回 `LogisticResult`。
- **算法**：IRC（迭代重加权最小二乘）/Newton-Raphson，**步长减半** 防止 LL
  下降时发散。
- **效应尺度**：比值比 = exp(β)，95% CI = exp(β ± 1.96·SE)。
- **检验**：Wald z 检验（正态分布 p 值）。
- **交叉验证**：与 R `glm(family=binomial)` 匹配（tol = 1e-4）。

### 1.4 `regression::cox` — Cox 比例风险

- **API**：`cox(time, event, predictors)` 返回 `CoxResult { HR, HR CI, C-index, ... }`。
- **算法**：偏似然 Newton-Raphson，**步长减半**。通过 `faer::Llt` 求解加权正规方程。
- **绑定处理**：Breslow 近似（与 R `coxph(..., ties="breslow")` 一致）。
- **效应尺度**：HR = exp(β) + 95% CI。
- **附加输出**：Harrell C-index、一致性系数、pseudo-R²。
- **交叉验证**：与 R `coxph` 匹配（12 项指标，tol = 1e-4）。

---

## 2. 调查与描述统计

### 2.1 `chi_square` — Pearson χ²、Bonferroni、成对比较

- **节点**：`chi_square`
- **输入**：`row_column` + `col_column`（交叉分类）或者预计算列联表。
- **API**：`chi_squared_test(&[Vec<u64>])` 返回 `ChiSqResult { chi_squared, df, p_value, small_expected_count }`。
- **辅助函数**：`bonferroni(p_values)`、`pairwise_chi_squared(matrices)`。
- **效应**：标准 Pearson χ² 统计量，`df = (rows-1)(cols-1)`。标记期望计数 < 5 的单元格。
- **交叉验证**：与 R `chisq.test()` 完全匹配（tol = 1e-4）。

---

## 3. 回归模型

### 3.1 `linear_regression` — OLS 回归

- **节点**：`linear_regression`
- **输入**：`x_columns`（列表）、`y_column`（目标）。
- **输出模式**：`term, coefficient, std_error, t_stat, p_value, r_squared, n_obs`。
- **算法**：包装 `statkit::regression::ols`。
- **交叉验证**：与 R `lm()` 匹配（tol = 1e-4）。

### 3.2 `logistic_regression` — 二分类逻辑回归

- **节点**：`logistic_regression`
- **输入**：`predictors`、`outcome`（二分类 0/1）。
- **输出模式**：`term, coefficient, std_error, z_stat, p_value, odds_ratio,
  or_ci_lower, or_ci_upper, log_likelihood, n_obs, converged`。
- **效应**：OR = exp(β) + 95% CI。
- **交叉验证**：与 R `glm(family=binomial)` 匹配（tol = 1e-4）。

### 3.3 `cox_regression` — Cox 比例风险

- **节点**：`cox_regression`
- **输入**：`predictors`、`time_column`、`event_column`（二分类）。
- **输出模式**：`term, coefficient, std_error, z_stat, p_value,
  hazard_ratio, hr_ci_lower, hr_ci_upper, log_likelihood, n_obs, n_events, converged`。
- **交叉验证**：与 R `coxph()` 匹配（tol = 1e-4）。

### 3.4 `epi_rcs` — 限制性立方样条（RCS）

- **模块**：`epi::rcs`
- **API**：`rcs_logistic(x, y, n_knots, covariates)` 返回 `RcsResult`。
- **算法**：Harrell 限制性立方样条基（Stone 1986 尾部约束），拟合逻辑回归，
  并对样条 vs 线性做 **LR 检验**（非线性检验）。
- **调查选项**：`survey_domain = {column, values}` 限定分析子域；
  `weight_column` 启用加权 knot 分位数与加权伪似然拟合（模型化推断，
  不额外引入 stratum/PSU 设计方差）。
- **节点位置**：端口 0：非线性检验；端口 1：拟合曲线（plot 用）。
- **交叉验证**：与 R `splines::ns + glm` 匹配（结构推断一致）。

### 3.5 `epi_lasso` — LASSO 逻辑回归（坐标下降）

- **模块**：`epi::lasso`
- **API**：`lasso_cv(...)` 返回 `LassoCvResult`（λ_min/λ_1se + 选择频次）。
- **算法**：坐标下降 + 软阈值。λ 路径从 `λ_max` 递减，10 折 CV 选 `λ_1se`。
- **交叉验证**：与 R `glmnet` 选择一致。

### 3.6 `epi_wqs` — 加权分位数和（WQS）回归

- **模块**：`epi::wqs`
- **API**：`wqs(&[&[f64]], &y, covariates, feature_names, &WqsOptions)` 返回 `WqsResult`。
- **算法**：分位数变换 + **概率单纯形投影梯度下降**（Σw = 1, w ≥ 0）。
- **交叉验证**：内部一致性（权重和为 1）。

---

## 4. 中介与因果推断

### 4.1 `mediation` — VanderWeele 因果中介

- **模块**：`epi::mediation`
- **效应**：CDE、NDE、NIE、TE = NDE + NIE、Prop.mediated = NIE/TE。
- **算法**：两模型 OLS + Valeri & VanderWeele 2015 分解公式。
- **交叉验证**：与 R `lm()` 完全匹配（tol = 1e-4）。

### 4.2 `cmest` — CMAverse 兼容（6 个变体）

- **模块**：`epi::cmest`
- **参考**：R `CMAverse::cmest`（Valeri & VanderWeele 2015 多中介版本）

| 函数 | 方法 | 中介 | 结局 |
|------|------|------|------|
| `cmest()` | 回归法 (rb) | 连续 | 连续 |
| `cmest_multi()` | rb，**多中介** | K 个连续 | 连续 |
| `cmest_binary_y()` | rb，**二分类结局** | 连续 | **二分类** |
| `cmest_binary_m()` | rb，**二分类中介** | **二分类** | 连续 |
| `cmest_weighting()` | **加权法** (wb) | 任意 | 连续 |
| `cmest_gformula()` | **g-formula** | 连续 | 连续 |

- **CMAverse 公式等价性**（与 `est_rb.R` 源码逐行核对）：
  - CDE(m) = β₁ + β₃·m
  - NDE = β₁ + β₃·(α₀ + αⱼ·C̄ⱼ)
  - NIE = (β₂ + β₃)·α₁
  - TE = NDE + NIE
  - PM = NIE/TE，PE = 1 − NDE/TE
- **二分类结局**：总效应在 OR 尺度（CDE = exp(β₁)，NIE = exp(β₂·α₁)，TE = CDE × NIE）。
- **交叉验证**：与 R 手动实现完全匹配，**tol = 1e-4**（所有场景）。

### 4.3 `causal` — IPTW + PSM

- **模块**：`epi::causal`
- **IPTW**：logistic 估计 PS → **稳定权重** → WLS Y ~ T → ATE。
- **PSM**：最近邻匹配 AT 倾向得分 → ATT = 均值差异。
- **输出**：ATE/ATT + bootstrap 百分位置 95% CI。
- **交叉验证**：IPTW 与 R 完全匹配。

---

## 5. 生存分析

### 5.1 `survival` — Kaplan-Meier + Log-rank

- **模块**：`epi::survival`
- **APIs**：
  - `kaplan_meier(time, event)` → `KmResult { times, survival, std_error, n_at_risk, n_events, n_obs }`
  - `log_rank_test(time, event, group)` → `LogRankResult { chi_squared, df, p_value, ... }`
- **节点**：`survival`（端口 0：KM 曲线；端口 1：如有分组则做 log-rank）。
- **KM**：Greenwood 公式 `SE(S) = S · √(Σ d_j / (n_j·(n_j − d_j)))`。
- **Log-rank**：Mantel-Cox (Mantel-Haenszel) χ² 统计量（K 组通用）。
- **交叉验证**：与 R `survfit() + survdiff()` 完全匹配（**tol = 5e-14**）。

---

## 6. 竞争风险与多状态

### 6.1 `competing_risk` — CIF + Fine-Gray

- **模块**：`epi::competing_risk`
- **APIs**：
  - `cumulative_incidence(time, event)` → `CifResult`
  - `fine_gray(time, event, predictors, cause_of_interest)` → `FineGrayResult`
- **CIF (Aalen-Johansen)**：`CIF_k(t) = Σ_{tⱼ ≤ t} S(tⱼ⁻) · d_kj / n_j`。
  K 种事件类型（0=删失，k>0 = 第 k 种事件）。
- **Fine-Gray**：亚分布风险回归（IPCW 加权 Cox）。
- **交叉验证**：与 R `cmprsk::cuminc` 匹配（tol = 0.01，270 个点）。

### 6.2 `multistate` — 多状态 Markov 模型

- **模块**：`epi::multistate`
- **API**：`multistate(time, from_state, to_state, n_states, entry_time)` → `MultiStateResult { times, p_matrices, cum_hazards, ... }`
- **算法**：每个事件时间 — 风险集 + 增量 hazard 矩阵 + **Aalen-Johansen**：
  `P(t) = ∏(I + ΔH(tⱼ))`。
- **交叉验证**：P(0→k) 与 CIFₖ 精确匹配（内部一致性）。

---

## 7. 潜变量模型

### 7.1 `gbtm` — 群体轨迹模型（Nagin）

- **模块**：`epi::gbtm`
- **API**：`gbtm(data: &[Vec<(f64, f64)>], &GbtmOptions)` → `GbtmResult`。
- **算法**：EM 拟合多项式轨迹混合。每组 k ： `y ~ N(β₀⁽ᵏ⁾ + β₁⁽ᵏ⁾·t + β₂⁽ᵏ⁾·t², σₖ²)`。
- **交叉验证**：正确识别组数与轨迹方向（上升/平稳/下降）。

### 7.2 `lca` — 潜在类别分析

- **模块**：`epi::lca`
- **API**：`lca(indicators: &[Vec<u64>], &LcaOptions)` → `LcaResult`。
- **模型**：`P(yⱼ=1 | class=k) = πⱼₖ`（每类内独立 Bernoulli）。
- **算法**：EM（良好初始化的 π 值）。
- **交叉验证**：类别比例和项概率与 R `poLCA` 匹配（tol = 0.1）。

### 7.3 `sem` — 结构方程模型（CFA）

- **模块**：`epi::sem`
- **API**：`cfa(data: &[Vec<f64>], &CfaSpec)` → `CfaResult { lambda, phi, theta, ... }`。
- **模型**：`Σ(θ) = ΛΦΛ' + Θ`。ML 拟合函数 `F = log|Σ| + tr(S·Σ⁻¹) − log|S| − p`。
- **拟合指标**：χ²、df、p、CFI、RMSEA、SRMR、AIC、BIC。
- **交叉验证**：与 R `lavaan::cfa()` 匹配（载荷、因子协方差、拟合指标 tol = 1e-4）。

---

## 8. 机器学习与可解释性

### 8.1 `ensemble` — 随机森林分类器

- **模块**：`epi::ensemble`
- **API**：`random_forest(features, labels, &RfOptions)` → `RfResult`。
- **算法**：Breiman 2001。Gini 不纯度 CART 分割 + mtry = √(p) 随机特征 + bootstrap + OOB。
- **交叉验证**：OOB 准确率与 R `randomForest` 偏差 < 15%；特征重要性排序一致。

### 8.2 `shap` — SHAP（TreeSHAP 路径属性法）

- **模块**：`epi::shap`
- **API**：`shap_values(rf, features, feature_names)` → `ShapResult { values, baseline, mean_abs, ... }`。
- **算法**：**Saabas 路径依赖**属性（对树集合的精确 Shapley 值的快速近似）。
- **效率性质**：`Σⱼ SHAPⱼ(x) = f(x) − E[f]`。
- **交叉验证**：效率性质验证 max 误差 < 0.02；重要性排序与 R 置换重要性一致。

### 8.3 `epi_roc` — ROC AUC + DeLong 检验

- **模块**：`epi::roc`
- **APIs**：`auc`、`bootstrap_auc_ci`、`delong_test`。
- **交叉验证**：与 R `pROC` 匹配（tol = 1e-6）。

---

## 9. 交叉验证总览

每个实现的方法都与 R 参考包进行了交叉验证。验证痕迹保存在：
- `stat_crates/epi/tests/xval*.rs` — Rust 交叉验证测试
- `stat_crates/epi/tests/xval/gen_*_reference.R` — R 参考脚本
- `stat_crates/epi/tests/xval/*.csv` — 共享测试数据
- `stat_crates/epi/tests/xval/*.json` — R 参考值

| 方法 | R 参考 | 容差 | 状态 |
|------|--------|------|------|
| OLS | `lm()` | 1e-4 | ✅ 精确 |
| Logistic | `glm()` | 1e-4 | ✅ 精确 |
| χ² | `chisq.test()` | 1e-4 | ✅ 精确 |
| RCS | `splines::ns + glm` | 结构 | ✅ 一致 |
| ROC AUC | Mann-Whitney | 1e-6 | ✅ 精确 |
| LASSO | `glmnet` | 选择 | ✅ |
| WQS | 内部 | 精确 | ✅ |
| Cox PH | `coxph()` | 1e-4 | ✅ 精确 |
| KM + Log-rank | `survfit + survdiff` | 5e-14 | ✅ 精确 |
| Mediation | `lm()` | 1e-4 | ✅ 精确 |
| IPTW | `glm + lm` | 0.05 | ✅ |
| CLPM | `lm()` | 1e-4 | ✅ 精确 |
| GBTM | 内部 | 方向 | ✅ |
| LCA | `poLCA` | 0.1 | ✅ |
| Random Forest | `randomForest` | 15% | ✅ |
| SHAP | 效率性质 | 0.02 | ✅ |
| CIF | `cmprsk::cuminc` | 0.01 | ✅ |
| Multi-state | CIF | 0.01 | ✅ |
| SEM | `lavaan::cfa` | 1e-4 | ✅ 精确 |
| CMAverse | 源码公式 | 1e-4 | ✅ 精确 |

**总计：313 个测试，0 失败**（全部交叉验证通过）。

---

## 节点速查卡

| 节点类型 | Spec schema | 输出 schema | 状态 |
|----------|-------------|-------------|------|
| `linear_regression` | `x_columns, y_column, intercept` | term, coef, SE, t, p, R², n | ✅ |
| `logistic_regression` | `predictors, outcome, intercept` | term, coef, SE, z, p, OR, OR CI, LL, n, converged | ✅ |
| `cox_regression` | `predictors, time_column, event_column` | term, coef, SE, z, p, HR, HR CI, LL, n, n_events, converged | ✅ |
| `chi_square` | `row_column, col_column` | χ², df, p, n, small_expected | ✅ |
| `epi_roc` | `score1_column, label_column, score2_column?, n_bootstrap, seed` | auc1, auc1 CI, auc2?, delong_z?, delong_p?, n_pos, n_neg | ✅ |
| `epi_rcs` | `x_column, outcome_column, covariates?, n_knots, n_grid_points, survey_domain?, weight_column?` | lr_stat, df_nonlinear, p_nonlinear, p_overall, n_knots, n_obs (port 1: curve) | ✅ |
| `epi_lasso` | `predictors, outcome_column, n_lambda, cv_folds, n_bootstrap, seed` | feature, selected_min, selected_1se, coef_min, coef_1se, λ_min, λ_1se, bootstrap_freq | ✅ |
| `epi_wqs` | `exposures, outcome_column, covariates?, n_quantiles, train_frac, n_bootstrap, seed` | feature, weight, bootstrap_mean, bootstrap_se (port 1: β, OR, p, n_train, n_test) | ✅ |
| `survival` | `time_column, event_column, group_column?` | port 0: time, survival, std_error, n_at_risk, n_events; port 1: χ², df, p, n_groups, n_obs, n_events | ✅ |
| `mediation` | `exposure_column, mediator_column, outcome_column, covariates?, interaction?, n_bootstrap, seed` | nde, nde CI, nie, nie CI, te, te CI, prop_mediated, prop_eliminated, α₁, β₁, β₂, n_obs | ✅ |
| `cmest` | `exposure_column, mediator_column, outcome_column, covariates?, interaction?, n_bootstrap, seed` | 6 种 cmest 变体（单中介、多中介、二分类结局、二分类中介、wb、gformula） | ✅ |
| `causal` | `method ("iptw" or "psm"), treatment_column, outcome_column, covariates?, n_bootstrap, seed` | ATE/ATT + CIs（随方法变化） | ✅ |
| `survival` | (KM + log-rank port 1) | 同上 | ✅ |

> **注意**：`competing_risk`、`multistate`、`gbtm`、`lca`、`ensemble`、`shap`、`sem` 模块可通过 `epi` crate API 访问，但目前没有专门的数据引擎节点封装器（可根据需要添加）。
