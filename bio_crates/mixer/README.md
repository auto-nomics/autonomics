# mixer — Univariate + Bivariate MiXeR（Rust 移植）

MiXeR 混合模型的纯 Rust 实现（单变量 + 双变量），从论文数学（Holland et al. 2020,
_PLoS Genetics_）clean-room 重新实现，不复制原版 GPL C++/Python 源码。
详见 [`MIGRATION_PLAN.md`](MIGRATION_PLAN.md)。

## 模型

### Univariate（`fit1`）

spike-and-slab 先验下的 GWAS z-score 负对数似然：

```
β_s ~ (1−π)·δ_0 + π·N(0, σ²_β)              # 先验
z_j = Σ_s √(N_j·h_s·r²_{js})·β_s + ε        # LD 传播 + 噪声
```

拟合 3 个参数：`π`（polygenicity）、`σ²_β`（discoverability）、`σ²_zero`（截断方差）。
cost 用解析 Gaussian 近似（2 分量矩匹配），优化用差分进化（DE×N）+ Nelder-Mead 精修，
完全无导数，对齐原版 `fit_sequence=['diffevo-fast','neldermead']`。

### Bivariate（`fit2`）

联合两个 trait 估计共享/特异 causal 变异与遗传相关。每个 tag 拟合**单个 2D 高斯**，
协差矩阵由 β 的三个 2 阶矩经 LD 传播得到（区别于 univariate 的 2 分量矩匹配）：

```
Eb20=(π1+π12)·σ²β,1   Eb02=(π2+π12)·σ²β,2   Eb11=π12·ρβ·√(σ²β,1·σ²β,2)
Σ_j = [[Ed20+σ²z,1,  Ed11+ρz·√(σ²z,1·σ²z,2)], [·, Ed02+σ²z,2]]   # 每 tag 2×2 协差
```

固定两个 univariate fit1 约束，优化 3 个自由参数 `ρβ`（因子相关）、`ρz`（残差相关）、
`π12`（共有 causal 比例）。fit_sequence 对齐原版
`diffevo-fast → neldermead-fast → brute1-fast → brent1-fast`（全程 gaussian cost）。

## 模块

| 模块          | 职责                                                        |
| ------------- | ----------------------------------------------------------- |
| `params`      | `UnivariateParams { pi, sig2_beta, sig2_zero }`             |
| `parametrize` | 有约束参数 ↔ 无约束空间（log/logit）+ DE 边界               |
| `ld_matrix`   | 稀疏 LD，COO → CSR                                          |
| `data`        | `ChromData`：z/n/h/weights/ld（tag 与 snp 共用 index 空间） |
| `cost`        | `univariate_cost_gaussian`（核心热循环）                    |
| `optimizer`   | `differential_evolution` + `nelder_mead`                    |
| `fit`         | `fit1` 入口（DE×repeats → NM 精修）                         |
| `result`      | `FitResult`：派生 h²/nc/nc@p9/AIC/BIC                       |
| `simulate`    | 前向模拟器（cost 的逆运算，用于参数回收验证）               |
| `bivariate/`  | 双变量子模块（见下）                                        |

### `bivariate/` 子模块

| 子模块        | 职责                                                                                 |
| ------------- | ------------------------------------------------------------------------------------ |
| `params`      | `BivariateParams { pi[3], sig2_beta[2], sig2_zero[2], rho_beta, rho_zero }` + `rg()` |
| `transforms`  | 边界变换（log/exp、logit/logistic、arctanh/tanh，带 epsval 裁剪）                    |
| `data`        | `BivariateData`：z1/z2/n1/n2/h/weights/ld/tags                                       |
| `cost`        | `bivariate_cost_gaussian`（3 分量矩 + LD 传播 + 2D 高斯密度）                        |
| `parametrize` | `NaturalAxis`（3D）+ `ConstRgRhoZero`（1D，brute/brent）                             |
| `optimizers`  | 闭式泛型 DE / adaptive NM / brute1 / brent1                                          |
| `fit`         | `fit2` 入口（DE×N → NM → brute1 → brent1）                                           |
| `result`      | `BivariateFitResult`：派生 rg/dice/h²_T1/T2/nc12/AIC/BIC                             |

## 测试数据基础设施（`tests/fixtures/`）

交叉验证用原版 MiXeR 的官方测试数据（1000G EUR HM3 chr21+22），与 `trait1.fit1.json`
金标准对齐。**LD/AF 是原版同源**——由 `libbgmg.so` 从其自定义 TurboPFor 压缩的 `.ld` 二进制
解压导出（Rust 自己解码不现实），完全忠实。

| 文件                       | 内容                                                    | 来源                                      |
| -------------------------- | ------------------------------------------------------- | ----------------------------------------- |
| `trait1.sumstats.gz`       | GWAS sumstats（34958 SNP，列 SNP/Z/N）                  | 原版 `gsa-mixer/precimed/mixer-test/data` |
| `hm3_af.tsv.gz`            | allele frequency（`chrom, rsid, alt_freq`）             | libbgmg `bgmg_retrieve_mafvec` dump       |
| `hm3_ld.tsv.gz`            | LD 对（`chrom, tag_rsid, snp_rsid, r, r2`，194 万对）   | libbgmg `bgmg_retrieve_ld_r_chr` dump     |
| `trait1.fit1.json`         | 原版 fit1 参考输出（金标准）                            | 原版 `mixer.py fit1`                      |
| `trait2.sumstats.gz`       | 第二 trait 的 GWAS sumstats（bivariate 用）             | 原版同上                                  |
| `trait2.fit1.json`         | trait2 的 fit1 输出（fit2 约束）                        | 原版 `mixer.py fit1`                      |
| `hm3_bivar_weights.tsv.gz` | bivariate tag 子集 + randprune 权重 + 两 trait 的 n1/n2 | libbgmg `dump_bivar.py`                   |
| `fit2.json`                | 原版 fit2 参考输出（金标准）                            | 原版 `mixer.py fit2`                      |

### 重新生成 fixtures（数据更新时）

构建 `libbgmg.so` 并运行 dump 脚本（详见 `MIGRATION_PLAN.md` / 记忆 `mixer-ld-dump-workflow`）：

```bash
cd /mnt/disk3/gsa-mixer/src && mkdir -p build && cd build
cmake -DCMAKE_POLICY_VERSION_MINIMUM=3.5 .. && make bgmg -j$(nproc)
cd /mnt/disk3/gsa-mixer && python dump_ld.py        # → hm3_{ld,af}.tsv.gz
python dump_weights.py                               # → hm3_weights.tsv.gz (univariate)
python dump_bivar.py                                 # → hm3_bivar_weights.tsv.gz (bivariate)
# fit1/fit2 金标准：用 mixer.py 跑（命令见 cross_validation_*bivar.rs 头注释）
```

构建需修复 3 处 `CMakeLists.txt`（Boost 1.91 适配 + C++17 + GCC14 `-Wno-*`），已落盘。

## 测试

```bash
# 单元测试（合成数据参数回收 + 变换/优化器 roundtrip）
cargo test -p mixer --lib

# Univariate 交叉验证（真实 HM3 数据 vs 原版金标准；~30s）
cargo test -p mixer --test cross_validation -- --ignored --nocapture

# Bivariate 交叉验证（~40s）
cargo test -p mixer --test cross_validation_bivar -- --ignored --nocapture
```

### 交叉验证结果（已对齐 extract + randprune 权重）

测试用原版**精确的预处理输入**（extract 子集 11200 tags + randprune 权重，
均由 libbgmg dump，`sum_weights=2839.72` 与原版完全一致），喂给我们的 Rust `fit1`，
对比 `trait1.fit1.json` 金标准：

| 量          | Rust 拟合 | 原版金标准 | 相对误差 |
| ----------- | --------- | ---------- | -------- |
| `pi`        | 0.001305  | 0.001307   | 0.15%    |
| `sig2_beta` | 0.040294  | 0.040229   | 0.16%    |
| `sig2_zero` | 0.998163  | 0.998126   | 0.004%   |
| `h2`        | 0.588128  | 0.588019   | 0.02%    |
| `cost`      | 4107.1994 | 4107.1992  | 5e-6     |

**全部参数 < 0.2%，cost 匹配到 5 位有效数字**——证明 Rust 移植的 cost function +
LD 传播 + Gaussian 近似 + DE/Nelder-Mead 优化器在真实参考数据上数值正确。
残余 ~0.15% 来自 DE 实现差异（我们的 DE vs scipy 的）+ `sig2_zeroL=0` 简化。

> 注：精确对齐用的是 libbgmg dump 的权重/子集（验证 cost/optimizer 正确性）。
> Rust 原生 randprune（`weights.rs`）已实现并 **bit-exact** 对齐 libbgmg
> （`sum_weights=2839.7188`，11200 tags 逐 tag 精确匹配，见下"随机剪枝权重"）。

### Bivariate 交叉验证结果

测试用原版精确的预处理（extract + randprune 权重，与 univariate 同源；bivariate 权重经
`dump_bivar.py` 确认与 univariate 完全一致：11200 tags，`sum_weights=2839.72`），univariate
约束取自原版 fit1 输出，喂给 Rust `fit2`，对比 `fit2.json` 金标准（同样 `--fit-sequence
...-fast`，全程 gaussian cost）：

| 量         | Rust fit2 | 原版金标准 | 相对误差 |
| ---------- | --------- | ---------- | -------- |
| `cost`     | 8329.7159 | 8329.7158  | 1e-5     |
| `rg`       | −0.4733   | −0.4731    | 0.04%    |
| `rho_zero` | −0.0858   | −0.0858    | 0.01%    |
| `h2_T1`    | 0.58801   | 0.58801    | <1e-6    |
| `h2_T2`    | 0.73402   | 0.73402    | <1e-6    |
| `pi12`     | 0.000482  | 0.000786   | ⚠ 见下   |
| `rho_beta` | −0.994    | −0.609     | ⚠ 见下   |
| `dice`     | 0.462     | 0.753      | ⚠ 见下   |

**可识别量（cost、rg、rho_zero、h²）全部对齐到 4–5 位有效数字**，证明 Rust 的双变量
cost function（3 分量矩 + LD 传播 + 2D 高斯密度）+ 3D 优化器（DE/NM）+ 1D 精修（brute/brent）
在真实参考数据上数值正确。

⚠ **pi12 / rho_beta / dice 不匹配是模型固有的可识别性退化，非 bug**：bivariate 高斯 cost
对 `(pi12, rho_beta)` 的分解存在精确退化——沿 `rg=const` 曲线（`rho_beta = rg·√(pi1u·pi2u)/pi12`）
cost 恒定（`tests/scan_pi12.rs` 实测全区間皆为 8329.71589）。故高斯 cost 只识别 `rg`、`rho_zero`、
`cost`，无法区分 pi12/rho_beta 的具体分解。Rust 与原版落在同一平坦山脊上的不同等价点
（cost、rg 完全相同）。原版 production 的 `brute1`/`brent1`（**sampling** cost，非 `-fast`）
才打破此退化识别 pi12；本测试对齐 `-fast`（gaussian）序列，故只对可识别量断言。

> 后续工作：若需生产级识别 pi12，可移植 sampling cost（Monte Carlo 因果配置采样）。

### Bivariate sampling cost（打破退化）

`sampling` cost（`bivariate::sampling`）用 Monte Carlo 显式采样因果配置：每个 tag 按
(pi1,pi2,pi12) 对 LD 邻居做多项采样，生成 `k_max` 个配置，各配置算一组 (δ1²,δ2²,δ1·δ2)，
对应一个 2D 高斯 pdf，`pdf_tag = (1/k_max)·Σ_k pdf_k`。该平均保留了高阶矩信息，**打破
高斯 cost 的 pi12/rho_beta 退化**。纯 Rust 实现（`SmallRng` + `rand_distr::Binomial` +
Fisher-Yates 部分洗牌，rayon 并行），与 C++ **统计一致**（非逐 bit：RNG/binomial 算法不同，
但 k_max=20000、~11000 tag 下 MC 方差极小，两独立估计高度收敛）。

`Fit2Config { sampling: true, k_max: 20000 }` 让 brute1/brent1 用 sampling cost
（diffevo/neldermead 仍 gaussian）。

#### Sampling 交叉验证结果（`tests/cross_validation_bivar_sampling.rs`）

对比原版 `fit2_sampling.json`（`--fit-sequence diffevo-fast neldermead-fast brute1 brent1 --kmax 20000`）：

| 量           | Rust sampling | 原版金标准 | 相对误差 |
| ------------ | ------------- | ---------- | -------- |
| `cost(samp)` | 8294.82       | 8294.91    | **1e-5** |
| `rg`         | −0.4734       | −0.4731    | 0.04%    |
| `rho_zero`   | −0.0858       | −0.0858    | 0.01%    |
| `pi12`       | 0.000497      | 0.000483   | 2.75%    |
| `rho_beta`   | −0.964        | −0.990     | 2.63%    |
| `dice`       | 0.476         | 0.463      | 2.75%    |

**sampling 打破退化后 pi12/rho_beta/dice 也对齐**（~3%，残余为 MC 噪声 + 边界处 rho_beta≈−1
的平坦性）。sampling cost 本身匹配到 5 位有效数字。模型把解钉在 `rho_beta→−1` 边界
（pi12→min_pi12），两实现一致收敛到该区域。

## DAG 节点

`univariate_mixer`（fit1）与 `bivariate_mixer`（fit2）已注册为 data-engine DAG 节点
（`crates/data-engine/.../nodes/`）。`bivariate_mixer` 4 输入：trait1/trait2 sumstats +
trait1/trait2 fit1 结果；从 VFS 数据湖读 LD/AF，调 `mixer::bivariate::fit2`，默认
`sampling=true`，并计算 **bit-exact 随机剪枝权重**（`weights.rs`，与原版 `set_weights_randprune` 逐位一致）。

## 随机剪枝权重（`weights.rs`）

clean-room 复刻原版 `set_weights_randprune(n, r2, maf, use_w_ld)`。64 轮随机贪心选取 LD 独立集，
`weight[tag] = 选中次数 / n`。**纯 Rust，bit-exact**：

- `Mt19937_64`：参考实现，与 `std::mt19937_64` 位兼容（实测前 3 个输出逐位一致）。
- `uniform_int`：libstdc++ GCC 的 Lemire 无偏法（`_S_nd` + `u128`）。
- 贪心剪枝算法逐字镜像（碰撞重建 candidate、`num_changes==0` 取消、per-tag `max(0,·)` 钳制等）。

### 权重交叉验证（`tests/cross_validation_weights.rs`）

| 量                | Rust            | 原版 libbgmg |
| ----------------- | --------------- | ------------ |
| `sum_weights`     | 2839.718750     | 2839.718750  |
| 逐 tag 精确匹配   | **11200/11200** | —            |
| max_abs / max_rel | 0 / 0           | —            |

**逐 tag bit-exact 一致**——给定相同 LD 顺序与 deftag 集合，Rust 权重与原版完全相同。
两节点（`univariate_mixer`、`bivariate_mixer`）已接入，默认 `n=64, r2=0.1, seed=123`。
