# 设计:统计节点空白填补 — factor_rotation / rrr / mediation_weighted_binary / svy_rcs / survey-LRT

状态:已实现(分支 fix/svydomain)。缺口清单 #6(零权重容忍)已由 a46a18f 单独修复,本文档覆盖 #1–#5。

## 分层

| 层 | 位置 | 内容 |
|---|---|---|
| 算法库 | `stat_crates/{ml,statkit,epi,survey}/src/…` | 纯 Rust,无 Arrow |
| 节点 | `crates/node-bundles/nodes-{ml,regression,causal,survey}/src/…` | Spec/Factory/Node + 内联测试 |
| 金标准 | 各 crate `tests/golden/`(ml/statkit/survey、nodes-survey)或 `tests/xval/`(epi) | gen_*.R + CSV + JSON + 对照测试 |

## 节点一览

| 节点 kind | bundle | 库函数 | R 等价 |
|---|---|---|---|
| `ml_factor_rotation` | nodes-ml(dimred) | `ml::rotation::{varimax, promax}` | `stats::varimax` / `stats::promax` |
| `rrr` | nodes-regression | `statkit::regression::reduced_rank_regression` | Izenman RRR(lm+svd 手写参照) |
| `mediation_weighted_binary` | nodes-causal | `epi::mediation_weighted::mediation_weighted_binary` | svyglm(quasibinomial) 路径模型 |
| `svy_rcs` | nodes-survey | `epi::rcs` 基 + `survey::svyglm` + `reg_term_test_with` | `svyglm(y ~ rcs(x,k) + C, design)` + regTermTest |
| `reg_term_test` 的 `method="LRT"` | nodes-survey | `survey::svy_lrt` + `survey::pfsum` | `regTermTest(method="LRT")` / `anova.svyglm(method="LRT")` |

端口约定:`ml_factor_rotation` 与 `rrr` 双输出(载荷/系数表 + rotmat/scree);`svy_rcs` 双输出(summary 单行 + curve 网格);其余单输出。

## 已核实的 R 语义(实现依据)

- **varimax**(`normalize=TRUE, eps=1e-5`):Kaiser 行归一;迭代 ≤1000 次,`z=x·TT`,
  `M = z³ − z·diag(colsum z²)/p`,`B = xᵀM`,`svd(B)`,`TT = u·vᵀ`,
  收敛 `d < d_past·(1+eps)`(d = 奇异值和);**循环结束后用最终 TT 重算 z**。
  关键下标:`M[i,j]` 全部由 z 的 **j 列**构成(B = Xᵀ·M 是普通矩阵乘)。
- **promax**(`m=4`):内部硬编码 `varimax(normalize=TRUE)`;`Q = v∘|v|^(m−1)`;
  `U = (vᵀv)⁻¹vᵀQ`;**列缩放 `sqrt(diag((UᵀU)⁻¹))`**(diag 要逐单位向量求解,
  不是逆矩阵乘全 1 向量);`rotmat = varimax.rotmat · U`。
- **survey LRT**:`chisq = deviance(reduced) − dev_full`(同权重行时 rescale=1;
  deviance 为 mean-1 重标权重的加权 deviance);`V = design_cov[idx]`,
  `V0 = naive_cov[idx]`(**未乘 dispersion**,R 用 cov.unscaled);
  `λ = eig(V0⁻¹V)`;`p = pFsum(chisq, rep(1,q), λ, ddf, method="saddlepoint")`。
- **ddf**:`df.residual = degf(design) + 1 − p`(探针在 gaussian/quasibinomial 上
  机器验证:degf=32, p=4 → 29)。`reg_term_test_with` 的 Wald 分支已同步修正
  (此前用 degf,偏差一并修复)。
- **pFsum/pchisqsum**:saddlepoint = Satterthwaite 基线 + Lugannani–Rice 精化
  (`a' = λ ⊕ (−x/ddf)×ddf`,在 0 处取尾);"sad" 前缀匹配 **saddlepoint**。
  R 的 saddle 用 `uniroot(tol=1e-8)`,近退化情形(小 ζ)只能复现到 ~1e-6;
  我们用机器精度二分(更准的一方),金标容差据此设定。
- **RRR**:`B_k = B_ols·V_kV_kᵀ` 作用于**中心化**拟合矩阵的 SVD,截距由加权均值
  重加(`ȳ − x̄ᵀB_k`)——中心化拟合矩阵秩 ≤ k 时 rank-k 无损。
- **svy_rcs**:基函数 = Harrell/Stone-Koo(rms/Hmisc 语义);打结 = 加权百分位;
  拟合/推断走完整设计(sandwich 协方差 + 设计 df)。

## 顺带修复的预存 bug

1. `survey::family` 的 binomial/poisson `dev_resid` 缺少 R 的 ×2 系数
   (IRLS 系数不受影响,但 deviance/dispersion 减半;已修 + 单测更新)。
2. `reg_term_test_with` Wald 的 ddf 用了 degf 而非 `degf + 1 − p`(已修,
   survey/tests/svy_lrt_reference.rs 对照 R Wald p 验证)。

## LRT 行集对齐(gotcha)

`svyglm` 对每次 fit 独立做 NaN 过滤。若被 drop 的预测变量含 NaN,reduced fit
会保留更多行 → deviance 对比失真。双重保障:
- 节点层(`reg_term_test` execute):凡 full 预测集任一列 NaN 的行,先把 y 置 NaN;
- 库层(`svy_lrt`):断言两 fit 的 `n` 与设计 `df` 相等,否则报错。

## 金标准与探针

| 测试 | 生成方式 | 容差 |
|---|---|---|
| `ml/tests/rotation_r_reference.rs` | 本地 Rscript `stats::varimax/promax` | 1e-8 |
| `statkit/tests/rrr_r_reference.rs` | 本地 Rscript(lm+svd 手写同估计器,自检) | 1e-9 |
| `epi/tests/xval_weighted_mediation_binary.rs` | 探针(survey 4.5)svyglm 系数 + 手工分解 | 系数 1e-6,分解 1e-5(bootstrap CI 不比,RNG 不可对齐) |
| `survey/tests/svy_lrt_reference.rs` | 探针 regTermTest LRT(gaussian+quasibinomial,含一条零权重行) | chisq 1e-6,p 1e-6(相对带) |
| `survey/src/pfsum.rs` 内联 | 探针 pFsum/pchisqsum(saddlepoint+satterthwaite 各 4 组) | satt 1e-12,saddle 5e-6(R 自身根容差) |
| `nodes-survey/tests/svy_rcs_r_reference.rs` | 探针(显式 knots + R 侧独立 Harrell 基 + svyglm + regTermTest) | 1e-8 |

探针镜像(一次性,用后删):

```
mkdir /tmp/survey-probe && cd /tmp/survey-probe
cat > Containerfile <<'EOF'
FROM docker.io/rocker/r-ver:4.5.1
RUN R -q -e 'options(repos = c(CRAN = "https://packagemanager.posit.co/cran/__linux__/noble/latest")); install.packages(c("survey", "jsonlite"))'
EOF
podman build -t survey-probe .
# 各 gen_*.R 头部注明运行方式;-v $PWD:/repo:Z -w /repo
podman rmi survey-probe
```

## 验证

- 逐 crate 串行(勿 `--workspace`,本机 OOM):
  `cargo test -p survey -p statkit -p epi -p ml -p nodes-survey -p nodes-causal -p nodes-regression -p nodes-ml`
- 已知 flaky(预存,与本次无关):`ml::centroid::tests::test_fit_predict_separable`
  在概率 0.898 vs 0.9 阈值边缘随机翻车。
- 下游 `cargo check -p data-engine`(default features 含全部 bundle)通过。
