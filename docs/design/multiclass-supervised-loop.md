# 多分类监督闭环（multiclass supervised loop）设计

状态：**设计已评审，按 §7 顺序实现中**。首期服务于脊索瘤 CRM 三型研究主线
（蛋白组分型迁移 + MRI/H&E 三分类），但对任意"冻结分类器 → 外部应用 +
patient-level 评价"场景通用。

## 0. 背景

引擎现状（2026-09 调查）：

- `ml` 的监督分类全部二元（`stat_crates/ml/src/classify.rs` 的 logistic 输出
  P(class=1)），无 multinomial elastic-net、无 PAM/nearest shrunken centroid；
- 经典 ML 无"冻结模型 → 新样本"通路：`ml_model_save` 写空模型字节
  （`nodes-ml/src/model_nodes/model_save.rs`），`ml_model_load` 只回 metadata；
- multiclass 概率输出、macro-OvR AUROC、multiclass Brier/log loss、
  class-wise 校准均无实现；
- `ml::split::group_kfold` 库函数已存在但无节点包装。

DAG 侧先例：DL 的 `artifact_bytes` Binary 列端口闭环
（`nodes-dl/src/model_nodes.rs`）；`nested_oof` 的黑盒节点内嵌套 CV
（`nodes-survival/src/nested_oof.rs`）。

## 1. 总体架构

```
                     ┌─ 节点层 nodes-ml ─────────────────────────────────┐
 新增:  ml_group_kfold  ml_pam_fit  ml_multinomial_enet_fit              │
       ml_frozen_predict  ml_multiclass_metrics                          │
 修复:  ml_model_save  ml_model_load                                     │
                     └──────────────┬────────────────────────────────────┘
                                    │ 包装
                     ┌─ 库层 stat_crates/ml ────────────────────────────┐
 新增:  src/centroid.rs（PAM）  src/multinomial.rs（softmax elastic-net）│
 扩展:  src/metrics.rs（OvR AUC / multiclass Brier / log loss /        │
        calibration / cluster bootstrap）                               │
 不动:  src/artifact.rs  src/split.rs（group_kfold 已存在）             │
                     └──────────────────────────────────────────────────┘
```

### artifact 数据流（照抄 DL 模式）

```
ml_pam_fit ──port0: artifact_bytes──┬──► ml_frozen_predict ◄── port1: 新样本表
  port1: cv_curve                    └──► ml_model_save(uri=crm_pam_v1.bin)
  port2: signature_panel                       │ (跨 DAG / 跨周冻结)
  port3: oof_predictions(可选)                 ▼
                                       ml_model_load ──► ml_frozen_predict(外部 DAG)
```

端口契约：**artifact 行 = `artifact_bytes`(Binary) + `kind`(Utf8) 单行**；
fit 节点可附元数据列，`ml_frozen_predict` 只依赖 `artifact_bytes`。
save/load 校验并透传字节（`ModelArtifact::from_bytes`），另附
`feature_names`/`training_meta` 人读列。

"冻结"的操作语义：`ml_model_save` 写出的文件 + 其指纹进入快照 RunReport；
外部测试 DAG 只连 `ml_model_load`，不重连 fit。

## 2. 库层设计

### 2.1 `ml/src/centroid.rs` — nearest shrunken centroid（PAM）

pamr（Tibshirani et al. 2002 PNAS）公式（2026-09-25 对 pamr 源码逐项实证，
后验对齐至 2.2e-16；三处与 pamr 实现差异见标注）：

- 类内合并 SD `s_j`（除数 n−K，**再加 offset = median(s)**，即 pamr
  `offset.percent=50` 默认——设计初稿漏了此项）；
- `m_k = √(1/n_k − 1/n)`（pamr 实现为**减号**；PNAS 原文印的是加号，
  以 pamr 为准）；
- 中心 `d_kj = (x̄_kj − x̄_j)/(s_j·m_k)`，收缩
  `d'_kj = sign(d)·max(|d| − Δ, 0)`，z 空间收缩中心 `c_kj = d'_kj·m_k`；
- 判别分数 `disc_k(i) = Σ_j z_ij·c_kj − ½Σ_j c_kj² + log π_k`（**+log π_k 取
  argmax**——初稿的 `+2·log π_k` 符号反了），`z_ij = (x_ij−x̄_j)/s_j`；
  后验 = `exp(clamp(disc, ±500))` 归一化，并列取首个最大；
- Δ 网格默认 30 点线性（0..max|d|），**全网格保留**（pamr 的 `$nonzero`
  只是诊断，不做尾部截断）；K 折 CV error 曲线；
  选择默认 min-error（并列取大 Δ），1SE 备选（spec 可切换）；
- signature panel = `d'_kj ≠ 0` 的 (class, feature) 对；
- 输入 z-score 参数随模型序列化（apply 端同变换 = 锁定预处理）。

```rust
pub struct PamModel { /* class_labels, feature_names, grand_mean, pooled_sd,
                         m_k, priors, shrunk_centroids, delta_selected,
                         delta_1se, x_mean, x_sd */ }   // kind = "pam:v1"
pub fn pam_fit(x: &Mat<f64>, y: &[usize], labels: &[String],
               delta_grid: Option<Vec<f64>>, cv: Option<CvPlan>)
    -> Result<(PamModel, PamCvCurve, PamSignature)>;
pub fn pam_predict(model: &PamModel, x: &Mat<f64>) -> Result<ClassProbs>;
```

### 2.2 `ml/src/multinomial.rs` — softmax elastic-net

- 对称参数化 `W ∈ R^{K×P}` + 无惩罚截距；损失
  `L = −(1/N)ΣΣ y_ik·log p_ik + λ·[α‖W‖₁ + (1−α)/2·‖W‖²_F]`；
- 求解 **FISTA + soft-threshold + backtracking line search**（非 glmnet
  坐标下降；系数对齐 glmnet 到相对 1e-3，见 §6）；
- λ 路径：`λ_max` 向下 100 点 log 网格（min_ratio 1e-3），逐 λ 热启动；
- 内置 k 折 CV（可 group-aware）选 λ.min / λ.1se，默认 min；
- `class_weight = "balanced" | "none"`；特征标准化参数随模型序列化。

```rust
pub struct MultinomialEnetModel { /* W, intercept, class_labels, feature_names,
                                     x_mean, x_sd, lambda_selected, alpha */ }
pub fn mnet_fit(x: &Mat<f64>, y: &[usize], labels: &[String],
                cfg: MnetConfig, cv: Option<CvPlan>)
    -> Result<(MultinomialEnetModel, CvPath, CoefTable)>;
pub fn mnet_predict(model: &MultinomialEnetModel, x: &Mat<f64>) -> Result<ClassProbs>;
```

### 2.3 `ml/src/metrics.rs` 扩展

```rust
pub fn ovr_auc(y_true: &[usize], probs: &Mat<f64>, k: usize) -> Result<Vec<f64>>;
pub fn macro_ovr_auc(/* 同上 */) -> Result<f64>;       // Mann-Whitney 秩，处理并列
pub fn multiclass_brier(y_true: &[usize], probs: &Mat<f64>) -> Result<f64>;
pub fn multiclass_log_loss(/* 同上 */) -> Result<f64>; // clip 1e-15
pub fn class_calibration(y_bin: &[bool], p: &[f64], n_bins: usize)
    -> Result<CalibrationCurve>;   // 分组率 + logistic 校准斜率/截距
pub fn balanced_accuracy(cm: &[usize], k: usize) -> Result<f64>;
pub fn cluster_bootstrap<T: Clone>(stat: impl Fn(&[usize]) -> T, clusters: &[u64],
    n_boot: usize, seed: u64) -> Result<Vec<T>>;       // 组级重抽样
```

`confusion_matrix`/`macro_precision_recall_f1` 已存在，直接接节点。
OvR AUC 在 ml 内自实现（不依赖 `epi::roc`，避免 crate 依赖方向改动）。

## 3. 节点层设计（nodes-ml）

| 节点 | 输入 | spec 关键字段 | 输出 |
|---|---|---|---|
| `ml_group_kfold` | 表 | `group_column`、`k` | 原表 + `fold`(UInt32)；组完整同折 |
| `ml_pam_fit` | 特征表 | `features`、`label_column`、`delta_select`(min/1se)、`cv_k`、`group_column`、`fold_column`、`seed` | p0 artifact；p1 cv_curve；p2 signature_panel；p3 OOF（可选） |
| `ml_multinomial_enet_fit` | 特征表 | `alpha`、`lambda_select`、`class_weight`、`cv_k`、`group_column`、`fold_column`、`max_iter`、`tol`、`seed` | p0 artifact；p1 cv_path；p2 coef_table；p3 OOF（可选） |
| `ml_frozen_predict` | p0 artifact 行；p1 新样本 | `min_top_prob`、`uncertain_margin` | 原列 + `p_<class>`×K + `prediction` + `top_prob` + `margin` + `is_uncertain` |
| `ml_multiclass_metrics` | 概率表 | `label_column`、`prob_columns`、`cluster_column`、`n_boot`(2000)、`seed`、`ci_level`(0.95)、`calibration_bins`(10) | p0 汇总单行（各指标+CI）；p1 per-class；p2 confusion；p3 calibration |
| `ml_model_save`（修复） | artifact 行 | `uri` | 校验 → 写 bincode → 透传 |
| `ml_model_load`（修复） | 无 | `uri` | `artifact_bytes` + `kind` + 人读列 |

`fold_column` 语义（照抄 `nested_oof`）：给出时，对每个外层折——其余折上
重新执行内层超参 CV → 训练 → 预测该折，产出 OOF 概率表（防泄漏，fusion
层合规输入）；同时全数据在选定超参上重训出最终 artifact。无环 DAG 不变。

`ml_frozen_predict` 按 artifact `kind` dispatch（`pam:v1` /
`multinomial_enet:v1`），未知 kind 报 NodeError；不重训、不接触训练数据。

## 4. 端到端接入（脊索瘤主线）

```
阶段1 分型迁移:
 原102例蛋白矩阵 ─► ml_pam_fit ─p0─► ml_model_save(crm_pam_v1.bin)  ← 冻结物
                     └p2 signature_panel
 新病例 ─► ml_model_load(crm_pam_v1.bin) ─► ml_frozen_predict(min_top_prob=0.6)

阶段2 M1:
 radiomics 宽表+clinical ─► ml_group_kfold(patient_id,5) ─► fold 列
   ─► ml_multinomial_enet_fit(fold_column, group_column=patient_id)
        p3 OOF 概率 ─► fusion 输入；p0 artifact ─► save(m1_v1.bin)
 外部封存中心 ─► load ─► frozen_predict ─► multiclass_metrics(cluster_column=patient_id)
```

## 5. 明确不做（首期出界）

XGBoost/GBM、ComBat/批次桥接、SMOTE、Platt/isotonic 校准（后续可加
`ml_calibrate`）、校准曲线渲染（表输出，图走 visualization 容器）、
图级循环原语。

## 6. 数值验证

R 参考脚本放 `stat_crates/ml/tests/golden/`（statkit/tests/xval 先例；
`reference/` 目录名被未锚定的 gitignore 规则吞掉，改为 golden）：

| 组件 | R 参考 | 对齐目标 |
|---|---|---|
| PAM | `pamr` | 同 Δ 后验 1e-4；CV 曲线形状；signature 集合 |
| multinomial EN | `glmnet(family="multinomial")` | λ_max/网格公式 1e-10；支撑集精确；目标函数值单侧 ≤1e-5；概率 ≤2e-3；CV deviance 差 ≤1e-2（系数见下方注记） |

> **平坦谷注记（2026-09-25 实测，glmnet 5.1）**：多项逻辑回归目标函数存在
> 平坦方向（给所有类别得分加公共 v(x) 似然不变），等效参数化下两次
> glmnet 拟合系数可差 2e-2 而目标函数仅差 1e-7——原定"固定 λ 系数
> ≤1e-3"数学上不可达（glmnet 自身也做不到）。golden 改为比较可识别量：
> λ 网格（公式精确）、支撑集（精确）、z 尺度目标函数值（单侧，更紧求解器
> 应更低）、概率（~3e-4）、CV deviance 曲线（1e-2 容差）。
| OvR AUC/Brier/log loss | 解析公式 + `pROC` | 1e-6 / 1e-10 / 1e-10 |
| group_kfold | 纯 Rust | 组完整性、并集完备 |

技巧：Rust 与 R **共享 fold 索引 CSV**（Rust 侧 dump），不对齐 RNG。

## 7. 实现顺序与验收

| # | 内容 | 估时 | 验收 | 状态 |
|---|---|---|---|---|
| 1 | save/load 修复 | 0.5 d | fit→save→load→predict 概率一致（roundtrip 单测） | ✅ 完成 |
| 2 | `ml_group_kfold` 节点 | 0.5 d | 组完整性测试 + spec 单测 | ✅ 完成 |
| 3 | metrics 库+节点 | 1.5 d | R 对齐 3 组 golden | ✅ 完成 |
| 4 | PAM 库+节点 | 2 d | pamr 对齐；panel 非空 | ✅ 完成 |
| 5 | multinomial EN 库+节点 | 3 d | glmnet 对齐；λ 热启动收敛 | ✅ 完成 |
| 6 | `ml_frozen_predict` + 端到端 fixture | 1 d | 合成三分类数据全链路 + OOF/全拟合分离断言 | ✅ 完成 |

每步独立 PR。#1、#2 无依赖可先行；#4、#5 可并行。
