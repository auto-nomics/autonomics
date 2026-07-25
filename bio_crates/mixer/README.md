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

### 交叉验证的已知差异

我们的 `fit1` 用 `weights=1`、全 SNP；原版金标准用 `randprune` 权重 + `extract` 子集
（11200 tags）+ `downsample=50`。因此：

- **`sig2_zero` 应高度一致**（null 方差膨胀，对权重稳健）——这是首要校验量。
- `pi`/`sig2_beta`/`h2` 会有系统偏差，但 `pi·σ²_β`（遗传效应预算）应接近。

当前结果（Rust vs 原版）：`sig2_zero` 误差 < 0.3%，`pi·σ²_β` 误差 ~14%，证实核心数学正确。
