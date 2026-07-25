# mixer — Univariate MiXeR（Rust 移植）

MiXeR 单变量混合模型的纯 Rust 实现，从论文数学（Holland et al. 2020, *PLoS Genetics*）
clean-room 重新实现，不复制原版 GPL C++/Python 源码。详见 [`MIGRATION_PLAN.md`](MIGRATION_PLAN.md)。

## 模型

spike-and-slab 先验下的 GWAS z-score 负对数似然：

```
β_s ~ (1−π)·δ_0 + π·N(0, σ²_β)              # 先验
z_j = Σ_s √(N_j·h_s·r²_{js})·β_s + ε        # LD 传播 + 噪声
```

拟合 3 个参数：`π`（polygenicity）、`σ²_β`（discoverability）、`σ²_zero`（截断方差）。
cost 用解析 Gaussian 近似（2 分量矩匹配），优化用差分进化（DE×N）+ Nelder-Mead 精修，
完全无导数，对齐原版 `fit_sequence=['diffevo-fast','neldermead']`。

## 模块

| 模块 | 职责 |
|---|---|
| `params` | `UnivariateParams { pi, sig2_beta, sig2_zero }` |
| `parametrize` | 有约束参数 ↔ 无约束空间（log/logit）+ DE 边界 |
| `ld_matrix` | 稀疏 LD，COO → CSR |
| `data` | `ChromData`：z/n/h/weights/ld（tag 与 snp 共用 index 空间） |
| `cost` | `univariate_cost_gaussian`（核心热循环） |
| `optimizer` | `differential_evolution` + `nelder_mead` |
| `fit` | `fit1` 入口（DE×repeats → NM 精修） |
| `result` | `FitResult`：派生 h²/nc/nc@p9/AIC/BIC |
| `simulate` | 前向模拟器（cost 的逆运算，用于参数回收验证） |

## 测试数据基础设施（`tests/fixtures/`）

交叉验证用原版 MiXeR 的官方测试数据（1000G EUR HM3 chr21+22），与 `trait1.fit1.json`
金标准对齐。**LD/AF 是原版同源**——由 `libbgmg.so` 从其自定义 TurboPFor 压缩的 `.ld` 二进制
解压导出（Rust 自己解码不现实），完全忠实。

| 文件 | 内容 | 来源 |
|---|---|---|
| `trait1.sumstats.gz` | GWAS sumstats（34958 SNP，列 SNP/Z/N） | 原版 `gsa-mixer/precimed/mixer-test/data` |
| `hm3_af.tsv.gz` | allele frequency（`chrom, rsid, alt_freq`） | libbgmg `bgmg_retrieve_mafvec` dump |
| `hm3_ld.tsv.gz` | LD 对（`chrom, tag_rsid, snp_rsid, r, r2`，194 万对） | libbgmg `bgmg_retrieve_ld_r_chr` dump |
| `trait1.fit1.json` | 原版 fit1 参考输出（金标准） | 原版 `mixer.py fit1` |

### 重新生成 fixtures（数据更新时）

构建 `libbgmg.so` 并运行 dump 脚本（详见 `MIGRATION_PLAN.md` / 记忆 `mixer-ld-dump-workflow`）：

```bash
cd /mnt/disk3/gsa-mixer/src && mkdir -p build && cd build
cmake -DCMAKE_POLICY_VERSION_MINIMUM=3.5 .. && make bgmg -j$(nproc)
cd /mnt/disk3/gsa-mixer && python dump_ld.py
# 产物：precimed/mixer-test/data/hm3_{ld,af}.tsv.gz → 拷回 tests/fixtures/
```

构建需修复 3 处 `CMakeLists.txt`（Boost 1.91 适配 + C++17 + GCC14 `-Wno-*`），已落盘。

## 测试

```bash
# 单元测试（合成数据参数回收）
cargo test -p mixer

# 交叉验证（真实 HM3 数据 vs 原版金标准；~30s）
cargo test -p mixer --test cross_validation -- --ignored --nocapture
```

### 交叉验证结果（已对齐 extract + randprune 权重）

测试用原版**精确的预处理输入**（extract 子集 11200 tags + randprune 权重，
均由 libbgmg dump，`sum_weights=2839.72` 与原版完全一致），喂给我们的 Rust `fit1`，
对比 `trait1.fit1.json` 金标准：

| 量 | Rust 拟合 | 原版金标准 | 相对误差 |
|---|---|---|---|
| `pi` | 0.001305 | 0.001307 | 0.15% |
| `sig2_beta` | 0.040294 | 0.040229 | 0.16% |
| `sig2_zero` | 0.998163 | 0.998126 | 0.004% |
| `h2` | 0.588128 | 0.588019 | 0.02% |
| `cost` | 4107.1994 | 4107.1992 | 5e-6 |

**全部参数 < 0.2%，cost 匹配到 5 位有效数字**——证明 Rust 移植的 cost function +
LD 传播 + Gaussian 近似 + DE/Nelder-Mead 优化器在真实参考数据上数值正确。
残余 ~0.15% 来自 DE 实现差异（我们的 DE vs scipy 的）+ `sig2_zeroL=0` 简化。

> 注：精确对齐用的是 libbgmg dump 的权重/子集（验证 cost/optimizer 正确性）。
> 生产用的 Rust 原生 randprune（`weights.rs`）尚未实现，是后续工作。
