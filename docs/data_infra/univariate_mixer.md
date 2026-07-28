# Univariate MiXeR 节点：算法与数据基础设施

> **节点类型**：`univariate_mixer`  
> **输入端口**：上游 GWAS sumstats（`Z: Float64, N: Float64, rsid: Utf8`）
> **输出端口**：单行拟合结果 DataFrame（pi, sig2_beta, sig2_zero, h2, nc, nc_p9, aic, bic, loglike）
> **依赖表**：`iceberg.af.eur_af`（等位基因频率）、`iceberg.mixer.eur_tagsuff`（预计算充分统计量）

## 1. 数学模型

### 1.1 先验：spike-and-slab

每个 SNP 的效应量 β_s 服从两分量混合先验：

```
β_s ~ (1-π)·δ_0 + π·N(0, σ²_β)
```

三个自由参数：
| 参数 | 含义 | 约束 |
|------|------|------|
| π | 多基因比例（polygenicity）| (0, 1) |
| σ²_β | causal 效应量方差（discoverability）| >0 |
| σ²_zero | null 分量方差的膨胀系数（截距）| >0 |

### 1.2 观测模型：LD 传播

每个 tag SNP j 的 z-score 是所有因果 SNP 经 LD 传播的叠加：

```
z_j = Σ_s √(N_j · h_s · r²_{js}) · β_s + ε,   ε ~ N(0, σ²_zero)
```

其中 h_s = 2·maf_s·(1-maf_s) 是 SNP s 的杂合度，r²_{js} 是 tag j 与邻居 s 之间的 LD。

### 1.3 矩匹配 Gaussian 近似

z_j 的精确边际分布是复杂的（因果变异数目不确定）。MiXeR 用一个 2 分量 Gaussian 混合来近似：

```
f(z_j) = tag_pi₀ · φ(z_j; 0, s₁²) + tag_pi₁ · φ(z_j; 0, s₂²)
```

两个分量（null 与 signal）的权重和方差由 tag j 的遗传效应 2 阶矩 A_j 和 4 阶累积量 B_j 唯一确定：

```
A_j = E[δ_j²]   = π·σ²_β · Σ_s N_j · h_s · r²_{js}
B_j = κ₄(δ_j)   = 3π(1-π)(σ²_β)² · Σ_s (N_j · h_s · r²_{js})²
```

闭式解：
```
tag_pi₀ = B / (B + 3A²)           ← null 权重
tag_pi₁ = 1 − tag_pi₀             ← signal 权重
σ²_tag  = (B + 3A²) / (3A)        ← signal 额外方差
s₂      = √(σ²_zero + σ²_tag)     ← signal 分量标准差
```

## 2. 充分统计量压缩

### 2.1 核心洞察

A_j 和 B_j 可以因式分解为**参数部分 × 数据部分**：

```
A_j = ebeta2 × m1_j    where ebeta2 = π·σ²_β,           m1_j = Σ_s N_j · h_s · r²_{js}
B_j = ebeta4 × m2_j    where ebeta4 = 3π(1-π)(σ²_β)²,  m2_j = Σ_s (N_j · h_s · r²_{js})²
```

m1_j 和 m2_j 是纯数据的标量，不依赖参数。把它们**预计算一次**后，每次 cost 求值只需 O(1)/tag 的浮点乘加，不再访问 LD 矩阵。

### 2.2 为什么 key

优化器（DE×20 轮 → Nelder-Mead 精修）对 cost 函数调用数万次。原版实现每次评估都用 `tag_moments` 遍历 CSR 邻居表（O(nnz)/次），总开销巨大。

压缩后：
- 扫 LD 从 数万次 → 1 次（预算）
- 内存从 O(nnz) CSR 塌缩为 O(n_snp) 的 m1/m2/weights 向量
- CSR 在拟合开始前即可释放

### 2.3 两阶段管线

```
┌──────────────────────────────────────────────────────────────────┐
│  阶段 1: precompute_tags（离线，逐染色体）                        │
│                                                                  │
│  af.eur_af ─→ h_vec (maf → 2·maf·(1-maf))                      │
│  ld_matrix.eur_chr{N} ─→ ld_pairs (r² ≥ 0.05)                  │
│                                                                  │
│  select_tags: MAF≥0.05 → LD prune (r²>0.8) → random subset      │
│                                                                  │
│  对每个 LD pair (a,b,r²):                                        │
│    if a ∈ tags: s1[a] += h_b·r²,  s2[a] += (h_b·r²)²           │
│    if b ∈ tags: s1[b] += h_a·r²,  s2[b] += (h_a·r²)²           │
│                                                                  │
│  weight = 1/(1 + Σr²)   ← 逆 LD-score（去冗余）                  │
│                                                                  │
│  输出: iceberg.mixer.eur_tagsuff                                 │
│        列: id_tag | s1 | s2 | sr | weight                       │
├──────────────────────────────────────────────────────────────────┤
│  阶段 2: univariate_mixer 节点（运行时）                          │
│                                                                  │
│  1. 读上游 sumstats → z_vec, n_vec, rsid→idx                    │
│  2. 查 af.eur_af → totalhet = Σ 2·maf·(1-maf), n_snp_ref       │
│  3. 读 eur_tagsuff → rsid 匹配后:                                │
│       m1[idx] = N × s1     （注：s1 是 N-free，运行时刻乘 N）     │
│       m2[idx] = N² × s2                                          │
│       weights[idx] = 1/(1+sr)                                    │
│       tags = [matched tag indices]                               │
│  4. fit1(suff): DE×repeats → NM → FitResult.derive()            │
│  5. 输出单行 RecordBatch                                         │
└──────────────────────────────────────────────────────────────────┘
```

### 2.4 N-free 设计

`s1`/`s2` 预计算时不带 N（N 来自 GWAS sumstats，per-SNP 不同）。运行时刻再乘 `N_tag` 和 `N_tag²` 注入：

```
m1[j] = n_vec[j] × s1[j]         ← s1[j] 从 tagsuff 表读
m2[j] = n_vec[j]² × s2[j]        ← s2[j] 从 tagsuff 表读
```

好处：同一份 tagsuff 可复用给不同样本量的 GWAS（N 不同时不需重算 LD）。

## 3. Cost Function

### 3.1 数学形式

```
cost(π, σ²_β, σ²_zero) = -log L = Σ_{j∈tags} w_j · [-log f(z_j)]
```

其中 w_j 是去冗余权重（逆 LD-score），f(z_j) 是矩匹配 Gaussian 混合密度（见 §1.3）。

### 3.2 实现位置

| 函数 | 文件 | 用途 |
|------|------|------|
| `univariate_cost_gaussian` | `bio_crates/mixer/src/cost.rs` | CSR 版参考实现（逐邻居遍历） |
| `univariate_cost_sufficient` | `bio_crates/mixer/src/cost.rs` | 压缩版（读 m1/m2），rayon 多核并行 |
| `tag_moments` | `bio_crates/mixer/src/cost.rs` | 计算 tag j 的 (A_j, B_j)，CSR 使用 |

验证：`sufficient_cost_matches_gaussian` 测试确认两版误差 < 1e-9（ULP 级重结合）。

### 3.3 数值保护

- `K_MIN_PDF = 1e-300`：防止 pdf 下溢导致 `log(0) = -∞`
- A=0 时跳过（无 LD 信号的 tag），避免 `tag_pi1 = 1 - B/(B+0)` 和 `sig2_tag` 除零

## 4. 优化策略

### 4.1 参数变换

优化器在无约束空间搜索（宽松边界），通过 bijection 映射到参数空间：

```
x₀ = ln(σ²_zero)     → σ²_zero > 0
x₁ = ln(σ²_β)        → σ²_β > 0
x₂ = logit(π)        → π ∈ (0,1),  logit(p) = ln(p/(1-p))
```

搜索范围（参数空间）：
```
σ²_zero: [0.9, 2.5]
σ²_β:    [5e-6, 5e-2]
π:       [5e-5, 5e-1]
```

### 4.2 两阶段优化

```
fit1(data, cfg):
  1. 差分进化 (DE/rand/1/bin)
     - 种群 45 (15×3 维)
     - 每代 F∈[0.5,1.0] 随机, CR=0.7
     - 收敛: max_cost-min_cost < 0.01
     - 重复 cfg.diffevo_repeats 次（默认 20），每次种子不同
     - 取全局最优

  2. Nelder-Mead 精修
     - 初始单纯形边长 0.5
     - 收敛: Δcost < 1e-7
     - 最大 1200 次迭代
     - 从 DE 最优解出发
```

完全无导数——不需要梯度或 Hessian。

## 5. 派生量

拟合完成后从最优参数 + 数据 derived 得到：

| 量 | 公式 | 含义 |
|----|------|------|
| h² | π·σ²_β·totalhet | SNP 遗传力 |
| nc | π·n_snp | causal 变异总数 |
| nc_p9 | nc·0.319 | 解释 90% h² 的 causal 变异数 |
| AIC | 2·3 + 2·cost | Akaike 信息准则 |
| BIC | ln(Σw)·3 + 2·cost | Bayesian 信息准则 |

其中 `totalhet = Σ_s 2·maf_s·(1-maf_s)`（全部参考面板 SNP 的杂合度和），
`n_snp = |af.eur_af ∩ 指定染色体|`。

**nc_p9 的 0.319**：原版 C++ 中经验标定的常数。其含义是：效应最大的前 31.9% 因果变异
即可解释 90% 的遗传力（受负选择下效应 size-MAF 负相关驱动）。当前简化版尚未实现
MAF 依赖效应模型（`sig2 ∝ maf^s`），直接复用原版常数。

## 6. LD-score 权重

### 6.1 动机

邻近 tag 通过 LD 共享大量邻居，z-score 高度相关。若每个 tag 独立计入 likelihood，
等于**重复计数**同一份 LD 信息。

### 6.2 公式

```
w_j = 1 / (1 + Σ_s r²_{js})
```

| Σr²（该 tag 的 LD 总分） | w_j | 含义 |
|---|---|---|
| ≈ 0 | ≈ 1.0 | 接近独立 |
| 10 | ≈ 0.09 | 高度冗余 |
| 100 | ≈ 0.01 | 几乎完全被邻居代表 |

LD-score 是"软"去冗余——比 randprune 的"硬"边界（入选/落选）更平滑，
且只需一次扫 LD 即可完成，不需要随机采样。统计方向上两者等价（差异 1-3%）。

## 7. Tagsuff 表结构

`iceberg.mixer.eur_tagsuff`（全染色体合并，EUR 人群）：

| 列名 | 类型 | 含义 |
|------|------|------|
| `id_tag` | Utf8 | tag SNP 的 rsid |
| `s1` | Float64 | N-free 一阶充分统计量 Σ h_neighbor·r² |
| `s2` | Float64 | N-free 二阶充分统计量 Σ (h_neighbor·r²)² |
| `sr` | Float64 | Σ r²（用于 weight 派生，冗余但保留用于诊断） |
| `weight` | Float64 | LdScore 权重 1/(1+sr) |

每行一个 tag SNP（通过 MAF≥0.05 → LD prune r²>0.8 → random subset 筛选）。
非 tag SNP 不出现在表中。

## 8. 关键实现决策

### 8.1 单表统一（当前方案）

- `eur_tagsuff` 是单张全染色体的 Iceberg 表（`iceberg.mixer` 命名空间下）。
- 由 `precompute_tags`（`crates/data-engine/src/bin/precompute_tags.rs`）**逐染色体产出
  parquet 文件后合并上传**。
- 节点按 rsid 字符串匹配（不依赖染色体编号或整数 index），因此 tagsuff 与 sumstats
  的 index 空间可以不同。

### 8.2 N-free 设计

`m1/m2` 的运行时刻化：tagsuff 中预计算的是不含 N 的 `s1/s2`，运行时刻由节点按
per-SNP 的 N 注入。这样同一份 tagsuff 可复用给不同 sample size 的 GWAS 研究。

### 8.3 全染色体范围

`eur_tagsuff` 包含所有 22 条常染色体的 tag。节点按 sumstats rsid 交集动态过滤，
因此即使 tagsuff 有全基因组 tag，只与 sumstats 重叠的部分参与拟合。

`totalhet` 和 `n_snp_ref` 来自 `af.eur_af` 对 spec 指定染色体的聚合查询——
必须与 sumstats 的染色体范围一致，否则 h² 和 nc 的尺度不对。

### 8.4 简化：sig2_zeroL = 0

原版 MiXeR 有一个 sig2_zeroL 参数捕获低 r²（<阈值）LD 尾部的残余贡献。
当前实现固定 sig2_zeroL=0。这是与金标准 ~0.15% 残余差异的主要来源。

### 8.5 合并 vs 块对角

多染色体时，LD 矩阵是块对角的（不跨染色体）。原版 MiXeR 会 `merge_blocks` 构建全局 CSR。
当前实现中：
- LdScore 模式**不需要 CSR**——逐批 LD 流式折叠进 m1/m2/sr，峰值内存 = 单条染色体一个 batch。
- tagsuff 预计算替代了运行时刻 CSR 构建，峰值内存进一步降到 O(n_snp)。

## 9. 已知限制

1. **Only EUR population**：`eur_tagsuff` 名暗示当前仅 EUR 人群可用的 LD 参考面板。
   对跨人群 GWAS 不能直接复用。

2. **nc_p9 常数未精确推导**：0.319 来自原版 C++，基于负选择下效应 size-MAF 模型标定。
   简化版尚未实现该模型，但 nc_p9 仅影响展示，不反馈到拟合。

3. **extract 参数与 tagsuff 的一致性问题**：节点 spec 的 `extract_*` 字段仅用于日志/文档，
   `eur_tagsuff` 表的内容由 `precompute_tags` 脚本的硬编码常数（MAF=0.05, R2_PRUNE=0.8,
   SUBSET=2M, SEED=123）决定。若修改 spec 中的 extract 参数而不重跑 precompute_tags，
   tagsuff 内容不会改变，导致 spec 与数据不一致。

4. **LdScore only**：当前标签权重固定为 `1/(1+Σr²)`。randprune 加权模式需另行构建
   不同的 tagsuff 表（randprune 权重需 CSR + 多次随机采样，无法折叠进标量权重）。

5. **无中游进度回调**：fit1 的 DE×NM 是同步 CPU 密集型优化，无 `await` 点。
   节点在执行前后发 info 事件标注时间段，但优化过程中无法 emit 进度。

## 10. 代码索引

| 组件 | 路径 |
|------|------|
| 节点定义 + execute | `crates/data-engine/src/nodes/univariate_mixer.rs` |
| tagsuff 预算脚本 | `crates/data-engine/src/bin/precompute_tags.rs` |
| 模型参数 | `bio_crates/mixer/src/params.rs` |
| 数据容器（ChromData, UnivariateSufficient） | `bio_crates/mixer/src/data.rs` |
| Cost function | `bio_crates/mixer/src/cost.rs` |
| 优化器（DE + NM） | `bio_crates/mixer/src/optimizer.rs` |
| fit1 入口 | `bio_crates/mixer/src/fit.rs` |
| 结果派生（h², nc, AIC…） | `bio_crates/mixer/src/result.rs` |
| 参数映射（log, logit） | `bio_crates/mixer/src/parametrize.rs` |
| LD 稀疏矩阵（CSR + BlockDiagonal） | `bio_crates/mixer/src/ld_matrix.rs` |
| tag 选择（clumping） | `bio_crates/mixer/src/extract.rs` |
| 模拟器（合成 z-score） | `bio_crates/mixer/src/simulate.rs` |
| Cross-validation vs 原版金标准 | `bio_crates/mixer/tests/cross_validation.rs` |
| E2E test（Iceberg GWAS） | `bio_crates/mixer/tests/test_univariate_mixer_e2e.rs` |
| 数学推导背景 | `bio_crates/mixer/README.md` |
