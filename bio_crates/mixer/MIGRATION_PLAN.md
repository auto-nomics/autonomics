# Univariate MiXeR (`fit1` / `test1`) — 纯算法 crate 迁移方案

> 范围：只做**纯算法 crate** `bio_crates/mixer`。从数据湖读取 LD 与 SNP 注释，
> 实现 univariate 混合模型的 cost function 与拟合，与原版 MiXeR 输出数值交叉验证。
> **不含** DAG node 集成（留待算法验证通过后再做）。

## 1. 算法事实核对（从 `gsa-mixer` 源码确认，纠正旧计划）

旧计划 `composed-forging-truffle.md` 假设 univariate 用 **Adam + 梯度**。这是错的——
那是 GSA-MiXeR（注释模型）的做法。Univariate `fit1` 完全不同：

| 项 | 事实（来自 `precimed/bivar_mixer/cli.py:83,162` 与 `src/bgmg_calculator_unified.cc`） |
|---|---|
| 默认 `--fit-sequence` | `['diffevo-fast', 'neldermead']` |
| `diffevo-fast` | scipy `differential_evolution`，全局优化，`--diffevo-fast-repeats` 默认 **20** 次取最优 |
| `neldermead` | scipy Nelder-Mead，局部精修 |
| cost calculator | `_cost_calculator_gaussian`（解析高斯近似，"fast"） |
| **梯度** | **不需要**。DE 与 Nelder-Mead 都是 derivative-free，只需 cost 值 |

**结论**：Phase 1 的热循环是 `cost(params) -> f64`（负对数似然），**无需梯度**。
这把工作量砍掉一大块——没有 Jacobian、没有反向传播。

### 1.1 三个自由参数（natural axis → 无约束空间）

`UnivariateParametrization_natural_axis`（`bivar_mixer/utils.py:304`）把参数映射到无约束空间：

```
x1 = log(sig2_zero)        # sig2_zero > 0
x2 = log(sig2_beta)        # sig2_beta > 0
x3 = logit(pi)             # pi ∈ (0,1)
```

优化在 `[x1,x2,x3]` 空间进行，求值时 `vec_to_params` 还原。DE 的边界（`cli.py:178`）：

```
pi:        5e-5 .. 5e-1
sig2_beta: 5e-6 .. 5e-2
sig2_zero: 0.9  .. 2.5
```

### 1.2 cost function 核（解析 Gaussian 近似）

精确抄自 `BgmgCalculator::calc_unified_univariate_cost_gaussian`（`bgmg_calculator_unified.cc:204`）。
Univariate 下 `num_components=1`，`pi_vec[0]=π`、`sig2_vec[0]=σ²_β` 对所有 SNP 一致。

对每个 tag SNP `j`：

```
# Step 1: 每个 causal SNP s 的矩（univariate 下处处相同）
Ebeta2[s] = π · σ²_β
Ebeta4[s] = 3·π·(σ²_β)² − 3·(π·σ²_β)²          # 注：源码里 Ebeta4 实存 E[β⁴]−3E[β²]²

# Step 2: LD 加权聚合到 tag（CSR 行扫描 j 的所有 r²≥r2min 邻居 s）
a2ij      = sig2_zeroC · n_j · h_s · r²_{j,s}
Edelta2[j]= Σ_s a2ij        · Ebeta2[s]
Edelta4[j]= Σ_s a2ij²       · Ebeta4[s]

# Step 3: tag 上的 2-分量高斯混合
A = Edelta2[j];  B = Edelta4[j]
tag_pi0   = B / (B + 3A²)
tag_pi1   = 3A² / (B + 3A²)
sig2_tag  = (B + 3A²) / (3A)
sig2_zero = sig2_zeroA + ld_tag_sum_r2_below_r2min[j] · n_j · sig2_zeroL
s1 = sqrt(sig2_zero);  s2 = sqrt(sig2_zero + sig2_tag)

# Step 4: 负对数似然（censoring：|z|>zmax 用 censored_cdf 代替 pdf）
pdf_j = tag_pi0 · φ(z_j; 0, s1) + tag_pi1 · φ(z_j; 0, s2)
cost += −log(pdf_j) · weight_j          # weight 来自随机剪枝/逆 LD-score
```

**Univariate 特例化**：因为 `Ebeta2`/`Ebeta4` 处处常数，Step 1 退化成标量，
Step 2 的内循环只依赖 LD CSR 行 + `h_s`（heterozygosity = `2·maf·(1−maf)`）。
`sig2_zeroC` 在纯 univariate 下可固定为 1（它是 GSA 注释路径的缩放）。
首版可把 `sig2_zeroL` 固定为 0（忽略 sub-r2min 尾部）或与 `sig2_zeroA` 合并，
等对齐验证后再决定是否放开。

### 1.3 必需输入量

| 量 | 来源 | 说明 |
|---|---|---|
| `z_j` | GWAS sumstats（trait1） | `Z` 列，per-tag |
| `n_j` | GWAS sumstats 或 SNP 注释表 | per-tag 样本量 |
| `r²_{j,s}` | **`ld_matrix.eur_chr{N}`** COO 对表 | `id_a`(tag rsid) + `id_b`(snp rsid) + `unphased_r2` |
| `h_s = 2·maf·(1−maf)` | **`af.eur_af`** 表 | `alt_freq` → `2·f·(1−f)` |
| `weight_j` | 随机剪枝 `--randprune` 或逆 LD-score | per-tag |
| `ld_tag_sum_r2_below_r2min[j]` | LD 对表（含低于阈值部分？需确认） | sig2_zeroL 项用 |

---

## 2. 数据湖读取层（用户已确认格式）

- **LD**：稀疏对表（COO 长表），按染色体。推断 schema：`(chr, tag_rsid/tag_idx, snp_rsid/snp_idx, r2)`。
- **MAF / N**：单独的 SNP 注释表。

### 已确认的 schema（来自 `infra/sink_ld_matrix` 和 `infra/sink_af` 源码）

#### `iceberg.ld_matrix.eur_chr{N}` （每条染色体一张表，N=1..22）

| 列名 | 类型 | 说明 |
|---|---|---|
| `chrom_a` | Int64 | tag SNP 染色体 |
| `pos_a` | Int64 | tag SNP 位置 |
| `id_a` | Utf8 | tag SNP rsid |
| `chrom_b` | Int64 | neighbor SNP 染色体 |
| `pos_b` | Int64 | neighbor SNP 位置 |
| `id_b` | Utf8 | neighbor SNP rsid |
| `unphased_r2` | Float64 | r² 值 |

- 来源：`/mnt/disk2/dataset/1000g_plink/eur/ld/1000G.EUR.chr{N}.ld.vcor.zst`（PLINK `.ld`，zstd 压缩 TSV）
- COO 稀疏对，每行是一个 (tag, snp, r²) 三元组
- **SNP 用 rsid 字符串**（`id_a`/`id_b`），不是数值 index → reader 需建 rsid→idx 映射
- tag 与 snp 来自同一参考面板（1000G EUR），但 tag 是 `.ld` 命令的第一列 SNP

#### `iceberg.af.eur_af` （单表，全部染色体，按 `chrom` 分区）

| 列名 | 类型 | 说明 |
|---|---|---|
| `chrom` | Int64 | 染色体 |
| `id` | Utf8 | rsid |
| `ref` | Utf8 | ref allele |
| `alt` | Utf8 | alt allele |
| `provisional_ref` | Utf8 | |
| `alt_freq` | Float64 | **allele frequency** → `h = 2·alt_freq·(1−alt_freq)` |
| `obs_ct` | Int64 | 观测计数 |

- 来源：`1000G.EUR.chr{N}.qc.afreq`
- **无样本量 N** — N 来自 GWAS sumstats 输入

### 关键推论

1. **rsid → idx 映射**：LD 对表用 rsid 字符串，cost 核用数值 index。Reader 需先收集全量 rsid 集合，建 HashMap，再把 COO 的 rsid 对转成 `(tag_idx, snp_idx, r2)`。
2. **MAF 来源**：`af.eur_af.alt_freq` → `hvec = 2·f·(1−f)`。
3. **样本量 N**：不在湖中，来自 GWAS sumstats 的 `N` 列。
4. **`sig2_zeroL` 项**：湖中无 sub-r2min r² 和。PLINK `.ld` 默认存所有对（`--ld-window-r2 0`），需确认导入时是否过滤。**首版固定 sig2_zeroL=0**，验证通过后再处理。
5. **tag/snp 可能是同一集合**：PLINK `.ld` 的 tag 列（第一列 SNP）和 snp 列来自同一 bim，tag 的邻居是窗口内所有其他 SNP。

### 2.1 reader 设计（与格式解耦）

```rust
// ld_matrix.rs —— 把湖里 COO 对表聚合成 per-chromosome CSR 行视图
pub struct LdBlock {
    pub chrom: u32,
    pub n_tag: usize,
    pub n_snp: usize,
    // CSR: 对每个 tag, 它的 (snp_idx, r2) 邻居列表（连续内存）
    pub row_ptr: Vec<usize>,   // CSR index, len = n_tag+1
    pub col_idx: Vec<u32>,     // snp idx
    pub r2:      Vec<f32>,     // r² 值
    pub tag_sum_r2_below_r2min: Vec<f32>, // 可选, 全 0 若湖里无
}
```

### 2.2 reader 流程（确认 schema 后）

```text
1. 查 af.eur_af（WHERE chrom = N）→ rsid→hvec HashMap
2. 查 ld_matrix.eur_chrN → (id_a, id_b, unphased_r2) COO 对
3. 收集所有 tag snp（id_a DISTINCT）+ 所有 snp（id_a ∪ id_b）的 rsid 集合
4. 建 rsid→idx 映射（tag_idx 和 snp_idx 可能不同集合）
5. COO → CSR: 按 tag_idx 排序, row_ptr/col_idx/r2 构建 LdBlock
```

reader 接口接受 `DataFrame`（从 Iceberg SQL 查回），与具体表名解耦，
方便单测时用内存 DataFrame 喂入。

---

## 3. crate 目录结构

```
bio_crates/mixer/
├── Cargo.toml          # 加 faer, ndarray(可选), rand, argmin, serde, serde_json, tracing
├── MIGRATION_PLAN.md   # 本文件
└── src/
    ├── lib.rs
    ├── error.rs        # Error 枚举（已有空壳）
    ├── params.rs       # UnivariateParams { pi, sig2_beta, sig2_zero }
    ├── parametrize.rs  # natural_axis ↔ 无约束空间 (log/logit)，DE 边界
    ├── cost.rs         # ★ 核心热循环：calc_univariate_cost_gaussian
    ├── ld_matrix.rs    # COO→CSR LdBlock，从 DataFrame 构建
    ├── data.rs         # DataBundle { z, n, h(maf), weights, ld_blocks }
    ├── weights.rs      # 随机剪枝 / 逆 LD-score 权重
    ├── optimize.rs     # differential_evolution + Nelder-Mead（argmin 或手写）
    ├── fit.rs          # fit1: diffevo-fast(×20) → neldermead，返回 FitResult
    ├── test.rs         # test1: 在拟合参数上评估全 SNP 后验/QQ（Phase 2）
    └── result.rs       # FitResult: pi, sig2_beta, sig2_zero, h², nc@p9, AIC/BIC
```

### 3.1 核心签名

```rust
// cost.rs
pub fn univariate_cost_gaussian(
    data: &DataBundle,
    ld: &[LdBlock],          // per-chromosome
    p: &UnivariateParams,
) -> f64;                    // 负对数似然（标量，无梯度）

// fit.rs
pub fn fit1(cfg: &FitConfig, data: &DataBundle, ld: &[LdBlock]) -> Result<FitResult>;

// result.rs —— 对齐原版 SCZ_and_INT.fit.csv 列
pub struct FitResult {
    pub pi: f64,
    pub sig2_beta: f64,
    pub sig2_zero: f64,
    pub h2: f64,            // = π · σ²_β · Σh / (π·σ²_β·Σh + ...)，按论文公式
    pub nc_p9: usize,       // 捕获 90% h² 的 causal 变异数
    pub aic: f64, pub bic: f64,   // MiXeR(3参) vs LDSR(2参)
    pub loglike: f64,
}
```

### 3.2 依赖

```toml
faer = { workspace = true }     # 已有，但 cost 核其实是标量循环，faer 用得少
rand = { workspace = true, features = ["small_rng"] }
argmin = "0.10"                 # 提供 NelderMead；DE 可能要手写或用 argmin-macros
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
# 不需要 datafusion/arrow —— 读湖由调用方(binary/后续 node)做,crate 只吃 DataFrame/原生类型
```

> **刻意选择**：crate 不依赖 `datafusion`/`arrow`/`datalake`。
> 读湖逻辑放在 binary 里，crate 只接受已组装好的 `DataBundle` + `LdBlock`。
> 这样算法核心可单测、可离线跑 fixture，不被湖耦合。（与 `ldsc` crate 把 `estimate_h2`
> 设计成吃 `DataFrame` 的取舍一致——但本 crate 连 DataFrame 都不吃，更纯。）

---

## 4. 拟合流程（对齐原版 `apply_univariate_fit_sequence`）

```
fit1:
  1. diffevo-fast ×20:
       for rep in 0..20:
         DE 最小化 calc_cost(x), seed = base_seed + rep
         保留 cost 最低的 (x*, cost*)
  2. neldermead:
       从 x* 起 Nelder-Mead 精修 (maxiter 1200, xatol 1e-4, fatol 1e-7, adaptive)
  3. 由 (pi, sig2_beta, sig2_zero) 派生 h², nc@p9, AIC/BIC, loglike
```

首版可先做 **单起点 Nelder-Mead**（最快走通端到端 + 交叉验证），
再补 DE 多起点（DE 是为了逃离局部极小，验证数值正确性不依赖它）。

---

## 5. 验证策略（最重要的部分）

### 5.1 单元测试（合成数据，已知参数）
- 构造已知 `(π, σ²_β, σ²_zero)` 的合成 z-vector（按模型采样），跑 fit，验证参数回收。
- cost function 单测：固定参数，手算一个小 LD 块的 cost，比对 `univariate_cost_gaussian`。

### 5.2 与原版交叉验证（真实数据）
- 原版示例：`mixer/usecases/mixer_real/SCZ_and_INT.fit.csv`（单变量列含 pi, sig2_beta, sig2_zero, h², nc@p9, AIC/BIC）。
- toy data：`gsa-mixer/precimed/mixer-test/` 的 chr21/22 bfile + LD + `trait1.sumstats.gz`。
- **基准**：跑 Docker/容器原版 fit1 在同一 toy sumstats + 同一 LD 上，
  Rust 版 `pi / sig2_beta / sig2_zero` 与原版误差 < 1%（浮点 + 种子差异）。
- LD 来源一致是关键：Rust 版必须用**产生原版 LD 的同一参考面板**，否则不可比。

### 5.3 回归
- AIC 符号要与原版一致（正 → 数据支持 MiXeR 优于 LDSR）。

---

## 6. 实施步骤（按顺序）

1. **~~查湖 schema~~** ✓ 已完成。LD: `ld_matrix.eur_chr{N}`（`id_a`, `id_b`, `unphased_r2`），AF: `af.eur_af`（`id`, `alt_freq`）。见 §2。
2. **`params.rs` + `parametrize.rs`**：参数结构 + log/logit 映射 + DE 边界。单测映射可逆。
3. **`ld_matrix.rs`**：COO DataFrame → `LdBlock` CSR。单测用内存 COO 喂入。
4. **`data.rs` + `weights.rs`**：组装 `DataBundle`；首版 weights 先全 1 或逆 LD-score。
5. **`cost.rs`**：抄 §1.2 公式实现 `univariate_cost_gaussian`。单测手算对拍。
6. **`optimize.rs`**：Nelder-Mead（argmin）先走通；再补 DE。
7. **`fit.rs` + `result.rs`**：串成 `fit1`，派生 h²/AIC 等。
8. **binary `examples/fit1_chr.rs`**：读湖 → fit → 打印/写 JSON，与原版交叉验证。
9. 数值对齐后 → 再谈 `test1`（Phase 2）与 DAG node（后续）。

## 7. 风险与注意

- **GPL 边界**：autonomics 是 MIT/Apache。§1.2 的公式从论文 + 公开算法行为推导，
  **不复制 C++/Python 源码文本**。本文档记录的是算法原理，实现须 clean-room。
- **LD 可比性**：交叉验证要求 Rust 与原版用同一 LD（同一参考面板、同一 r2min、同一剪枝）。
  否则参数差异无法归因于实现正确性。
- **`sig2_zeroC / sig2_zeroL`**：univariate 下含义与 GSA 不同，首版固定（C=1, L=0），
  验证后按需放开——放开前要确认湖里有 sub-r2min 的 r² 和。
- **float vs double**：原版 cost 用 `float`（`FLOAT_TYPE`）+ `valarray` + OpenMP。
  Rust 默认 `f64` 更稳但略慢；首版用 `f64` 保证正确，性能优化（Phase 6）再考虑 `f32`。
