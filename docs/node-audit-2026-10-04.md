# autonomics 节点/插件全面审计报告

日期：2026-10-04 ｜ 分支：feat/multiclass-supervised-loop（工作树 clean）
方法：9 路领域并行深度审计，全程只读；公式逐条对照 R 参考实现/金标测试，可疑处用 python3/Rscript 数值验证（MC 最多 4000 次重复）；外部插件 26 家族全部钉定 rev 浅克隆审读。每条发现均带 file:line 证据。

## 覆盖范围

- 内置 20 个 node bundle（约 172 kind）+ biofusion + data-engine/dag-core 引擎内部
- 未安装的 manifest 插件 26 家族 74 kind（github.com/auto-nomics/*-plugin，本机 ~/.autonomics/plugins 为空）
- 明确封存：container_command、legacy visualization（有测试锁死，非缺陷）

## 总体统计

| 域 | P0 | P1 | P2 | P3 |
|---|---|---|---|---|
| 因果/中介（nodes-causal） | 2 | 4 | 8 | 6 |
| 遗传/LDSC/MAGMA（nodes-ldsc/genetics/opengwas） | 0 | 5 | 6 | 6 |
| MR/共定位（nodes-mr/coloc + bio 后端） | 1 | 0 | 6 | 7 |
| IO/SQL/写作/引擎（nodes-io/sql/writing + dag-core + vfs） | 2 | 4 | 11 | ~13 |
| 经典统计（nodes-hypothesize/mice/regression/survival） | 5 | 3 | 9 | 5 |
| RD/GRF/LCMM（nodes-rd/grf/lcmm + bio 后端） | 2 | 7 | 10 | 10 |
| 调查/流行病（nodes-survey/epi + stat 后端） | 8 | 3 | 16 | 12 |
| ML/DL（nodes-ml/dl + stat 后端） | 0 | 13 | 19 | ~12 |
| 外部插件（26 家族 + container-plugin 框架） | 0 | 4 | 6 | ~8 |
| **合计** | **20** | **43** | **91** | **~79** |

---

# P0 全清单（会产生科学错误结果，共 20 条）

## 统计推断（5）

1. **wilcoxon_paired** — `crates/node-bundles/nodes-hypothesize/src/ranks.rs:105-117` — 配对模式从不读 y，统计量只用 x 计算。修复：配对时用 x−y 差值调 signed_rank。
2. **mann_whitney 单侧** — `stat_crates/hypothesize/src/primitives/ranks.rs:162` — u=u1.min(u2) 后单侧分支基于 min(U)，单侧 p 恒 ≥0.5。
3. **gof AD 检验 p 值** — `stat_crates/hypothesize/src/primitives/gof.rs:424-445` — 各分支公式错，可出现 p>1；A² 对应真 p=0.05 算出 0.413/0.752。按 D'Agostino/Stephens 重写。
4. **donor_composition 斜率 SE** — `stat_crates/.../donor_composition.rs:408` — 取 (H⁻¹)₀₀=h[1][1]/det 而非 (H⁻¹)₁₁，SE 低 26%、z 虚高。
5. **mice logreg 插补 β\* 抽样** — `stat_crates/mice/src/logreg.rs:248-255` — cov=(XᵀWX)⁻¹ 后 `Llt::new(cov).solve(&z)` 得 V⁻¹z（faer solve 是解方程不是乘因子），β\*=β̂+V⁻¹z 协方差 V⁻²，**插补值≈噪声**。修复：β̂+Lz（Cholesky 因子直乘）。

## 调查/流行病（8）

6. **svyivreg** — `stat_crates/survey/src/nonlinear.rs:245,262-272` — estfun 用原始 X × 结构残差，应为 X̂ × 残差（R ivreg component="projected"）；MC 4000 次：SE 虚高 ×3.6。节点层 p 另硬编码 0（`survey_model.rs:1108`）。
7. **svycoxph** — `stat_crates/survey/src/survival.rs:255-266` — dfbeta 逐观测分解≠R 的 score 残差（缺均值中心化）；MC：SE 虚高 52–68%。点估计正确。
8. **rcs_basis 分母平方（svy_rcs + epi_rcs 共享）** — `stat_crates/epi/src/rcs.rs:93,99-100` — denom=(t_max−t_min)² 使 λ+μ≠1，**样条失去尾部线性约束，模型空间≠Harrell RCS**；一行修复惠及两个节点。knot 百分位与 LR 检验本身正确。
9. **svyloglin** — 节点 t/p 硬编码 0（`survey_model.rs:1009-1010`）+ 后端系数取各格比例算术均值的对数而非 IPF 对数线性参数（`nonlinear.rs:669-726`）。
10. **svysurvreg** — 节点 p 硬编码 0（`survey_model.rs:850`）+ 后端删失行直接丢弃、对 log(T) 做 WLS（`survival.rs:454-482`），非加权 AFT MLE。
11. **svyolr** — 节点 p 硬编码 0（`survey_model.rs:938`）；后端 proportional-odds Newton 正确。
12. **（并入 6–11 的口径）4 个节点 p=0 意味着任何结果"永远显著"**：svyloglin/svyolr/svyivreg/svysurvreg。

## 因果/中介（2）

13. **mediation** — `stat_crates/epi/src/mediation.rs:273-274` — interaction:true 时 `m_under_control = alpha_0`（注释自称 "at covariate mean" 却丢掉 α₁x+Σα_cC̄）；数值验证 NDE 误差 **74%**，同一数据 mediation=0.34 vs cmest=1.47 互相矛盾。NHANES 年龄/BMI 等非零均值协变量全部触发。
14. **cmest_weighting** — `stat_crates/epi/src/cmest.rs:552-619` — 宣称 VanderWeele 2014 加权估计，实际是 IPTW 加权 `Y~X+M` 回归的 X 系数（**条件于中介**，重新引入该方法要避开的混杂）；MC：NDE +36%、NIE −25%；CI 恒 NaN、n_bootstrap 静默忽略。

## MR（1）

15. **mrlap** — `bio_crates/mrlap/src/pruning.rs:84` — 反向过滤把 R 的单侧 `qnorm(p)` 错译为双侧 `−qnorm(p/2)`（1e-3 时 −3.09→+3.29 且方向语义反转）；**默认参数每次运行都启用**，恰在结局信号接近暴露时大量误删 IV，向衰减方向偏倚。去偏校正本体（get_correction.R 移植）正确。

## IO（2）

16. **file_transform** — `crates/node-bundles/nodes-io/src/file_transform.rs:138-148` — GzDecoder 单成员解压，BGZF/多成员 gzip 大文件解出首块（~64KB）即"成功"静默截断；压缩侧自己写多块 → 往返不对称。修复：MultiGzDecoder（仓库内两处已是正确姿势）。
17. **dataframe_to_file append** — `crates/node-bundles/nodes-io/src/dataframe_to_file.rs:431-438` — append 模式旧文件多出的列被静默丢弃并覆写（新列集为子集时 select 是合法投影）；持久化数据无声丢失。

## RD/GRF（2）

18. **rdrobust** — `crates/node-bundles/nodes-rd/src/rdrobust_node.rs:63,229-246` — schema/doc 宣称支持 fuzzy RD，但 execute 从不把 fuzzy 传给后端（`..Default::default()`）；后端 `_fuzzy` 排序后丢弃 → **用户设 fuzzy 拿到的是 sharp ITT**，无警告。
19. **grf_average_treatment_effect overlap** — `bio_crates/grf/src/nodes/average_treatment_effect.rs:136` — target_sample="overlap" 实现为简单过滤后未加权均值 E[τ|0<ê<1]，R 是 Li–Morgan–Zaslavsky overlap 加权 WLS；同名不同估计量，doc 却称 "Mirrors grf R"。

---

# P1 重点清单（契约错误/明显 bug，共 43 条）

## nodes-causal
- cmest_binary_y prop_mediated clamp 到 [0,1]：抑制效应（真实 pm=6.00）静默报 1.00；prop_eliminated 同比值却不 clamp，两列口径不一（`epi/src/cmest.rs:376-377`）
- causal(psm) caliper 量纲错配：Austin 0.2×SD(logit) 生效阈值 vs 概率尺度距离；1350 配对中 24.9% 违反 logit 约束（`epi/src/causal.rs:313-334`）
- cmest_binary_m/weighting/gformula CI 恒 NaN、n_bootstrap(默认1000) 静默忽略，与 bundle 头注释 "bootstrap 95% CIs" 承诺不符（`epi/src/cmest.rs:523,601,718`）
- mediation_moderated bootstrap 间接效应漏乘 Δx（点估计带 dx、bootstrap 循环不带，同一循环自相矛盾；当前节点不暴露 x_treated 故为库级，`epi/src/mediation_moderated.rs:249-265`）

## 遗传
- cpassoc 权重 ∝n 而非文档/R 的 √n（`nodes-genetics/src/cpassoc.rs:422` + `bio_crates/cpassoc/src/stats.rs:55-61`）；SHom 示例 2.99 vs 1.75
- magma_set DataFrame 路径：mac 恒 100 → 常数协变量列 → X'R⁻¹X 奇异 → NaN → max(NaN,0)=0 → 静默空表或垃圾值；相关阵恒等静默丢基因相关（`nodes-genetics/src/magma.rs:884-918`）
- magma_set competitive 回归非 GLS（β 缺 R⁻¹、SSR 非加权）：31% 情形 SSR<0 被 max(0,·) 钳掉（`bio_crates/magma/src/setanalysis.rs:495-521`）
- magma_set/covar beta_std：sets 用 β×SD、covars 用 β/SD，相差 SD²（`setanalysis.rs:555-567`）
- magma_meta 声明变长输入端口但 execute 忽略 `_inputs` 只读 config 文件（`magma.rs:963-1084`）

## IO/引擎
- hypergeometric_ora_ondf fold_enrichment 倒置（第5元 expected/hits 应为 hits/expected；hits=2,expected=1 输出 0.5）（`nodes-sql/src/hypergeometric_ora.rs:224-232`）
- file_to_dataframe normalize_path 无条件 strip '.'：/data/.env/x.csv → /data/env/x.csv 静默读错文件（`nodes-io/src/file_to_dataframe.rs:440-451`）
- biofusion read_vcf_region 空批次 batches[0] panic（`biofusion/src/ext.rs:358`）
- table_from_df 未覆盖 Arrow 类型（UInt32/UInt64/Date/Timestamp/Decimal）静默空白格——ORA 的 hit_n/set_n 正是 UInt32（`nodes-writing/src/table_node.rs:283-284`）

## 经典统计
- chol_factor 是返回全零矩阵的桩（`stat_crates/mice/src/linalg.rs:164`，estimice.rs:86 调用）→ norm/pmm 的 β 后验抽样恒等于 β̂（先验不起作用）
- KM 标准误缺乘 Ŝ：报告的是 SE(log Ŝ)（`stat_crates/epi/src/survival.rs:120-135`；Ŝ=0.9315 时 0.0290 vs R 0.0270）
- hiernet prox 非声称的 hierNet onerow 精确解，改用软阈+等比缩放近似（`stat_crates/hierint/src/hiernet/prox.rs:75-140`）

## 调查/流行病
- svyolr 因变量水平按字母序编码："High"<"Low"<"Mid" 排错，阈值/斜率方向错（`survey_model.rs:911`）
- svylogrank ≥3 水平组静默折叠 A vs (B∪C)，无告警（`nodes-survey/src/survey_survival.rs:243-256`）
- epi_wqs n_quantiles=0 越界 panic，build 未校验（`stat_crates/epi/src/wqs.rs:442-446`）

## RD/GRF/LCMM
- grf auto-X 不排除 sample_weights_column → **权重列悄悄成为特征**（regression/causal/instrumental 三训练器；`bio_crates/grf/src/nodes/regression_forest.rs:174` 等）；Int64 特征列被静默丢弃
- grf_test_calibration / grf_best_linear_projection：声称 HC3 稳健 SE，实际 statkit wls 是经典同方差 SE，vcov_type 静默忽略（`stat_crates/statkit/src/regression/ols.rs:149-153`）；R 用 vcovCL HC3
- grf ATE treated/control：按臂 DR-score 均值 ≠ R 的 OOB 臂内均值+IPW γ 修正；训练权重被忽略（`average_treatment_effect.rs:110,134-135`）
- grf_predict_forest 按列位置而非列名对齐训练列：[y,w,x0..] 顺序的表静默产出错数，无 n_features 校验（`bio_crates/grf/src/nodes/predict_forest.rs:82-94`）
- rdrobust cluster 列字符串/整型静默丢失 → 按非聚类 SE 输出（`nodes-rd/src/rdrobust_node.rs:224-227`，extract_opt_f64 遇非 Float64 返回 None）
- bio_crates/grf lib.rs:5 "bit-identical to the R package" 总括声明失实（树核心是，分析层不是）

## ML/DL（13）
- ml_one_hot 多批次输入只保留第一个 chunk，>8192 行必坏（`one_hot_encode.rs:93-102`）
- ml_multinomial_enet_fit 默认 CV 折 λ 网格未共享按位置混池 → λ.min/λ.1se 失真（`stat_crates/ml/src/multinomial.rs:706-728`）
- ml_hierarchical 默认 "ward" 实为 average linkage（代码注释自认 approximation）（`cluster.rs:615-623`）
- ml_tsne 默认 θ=350（linfa 默认 0.5）→ 几乎所有细胞在根节点被汇总，**嵌入无效**；seed 从未传入不可复现（`tsne.rs:28-29`、`dimred.rs:206,229-230`）
- ml_svm/ml_predict：spec 的 C 被当核带宽 γ 用，SVM 代价硬编码 1:1，未知 kernel 静默回退 rbf，多类 `l!=0` 折叠（`svm_ensemble.rs:122-138`）
- DL 全部 6 个训练节点从不调 Backend::seed → **权重初始化全局不可复现**（burn 从熵取种子；同 seed 两次运行不同模型）
- dl_mlp_train/deepsurv：优化器种类/调度/dropout/BN/梯度裁剪全死，doc 逐项承诺（`models/mlp.rs:120-189`）
- dl_transformer_train **不是 transformer**，纯 MLP+GELU，attention/n_heads/positional 全死，doc 虚构（`models/transformer.rs:104-112`）
- dl_rnn_train **不是 RNN**，特征摊平进 MLP；validation 端口宣告不收集（`models/rnn.rs:105-113`）
- dl_deephit_train 删失样本对似然零贡献、单 softmax 头无竞争风险，doc 谎称 CIF（`models/deephit.rs:129,161`）
- dl_autoencoder_train(vae) 非变分：无重参数化，"KL"是确定性二次惩罚；loss 配置死恒 MSE（`models/autoencoder.rs:151-163`）
- dl_train_val_test_split n_test+n_val>n 时 usize 下溢 panic；stratify Utf8 列静默单层（`split_node.rs:182-204`）
- dl_survival_metrics Brier 的 S(t|x)=exp(−exp(r)) 与 t 无关 → "Brier 曲线"是平的；bootstrap CI spec 死而 doc 承诺（`metrics_nodes.rs:129-137`）

## 外部插件（4）
- **mtag/ldsc_munge 可选 bool 陷阱**：DSL 显式 false 渲染成非空 "false"，脚本 `[ -n "$VAR" ]` 判存在 → **传 false 反而开启** --no_overlap/--equal_h2/--force 等（框架 `compile/render.rs:43` + `mtag/scripts/h2.sh:7-23`、`ldsc/scripts/munge.sh:48-50`）；twosamplemr/hdl 的 `= "true"` 判等写法是对的
- gcta_cojo_select 声明必需输出 cma.cojo 但脚本只跑 --cojo-slct（cma.cojo 是 --cojo-cond 产物）→ **每次运行必 MissingOutput**（`gcta/manifest.toml:44`）
- mutation 两 kind：maf_path REQUIRED 却是死参数；1 输入 kind 读 AUTONOMICS_INPUT1（`mutation/scripts/mutation_analysis.R.sh:114`）
- bulk-rnaseq limma_voom quantile 路径：先在原始 counts 估权重再覆写归一矩阵 → 权重/表达错位，偏离官方 voom(normalize.method="quantile")（`limma_voom_runner.R:191-199`）

---

# 高影响 P2（择要，全 91 条见各域审计记录）

- **gsem 深尾 p 崩溃**：qchisq_sf 用 1−cdf，p≲1.1e-16 时 1−p==1.0 → z=∞ → 严格 finite 过滤**静默丢掉最强 SNP**（`bio_crates/genomic_sem/src/stats.rs:62-64`）——强 GWAS 必触发，接近 P0
- gsem_munge REF/ALT 别名映射方向（REF→A1）与 VCF 惯例相反，整表符号翻转无校验（`munge.rs:34-57`）；OR 启发式注释与代码相反（:200-215）
- sldsc 无 chisq_max 过滤，偏离 Python LDSC 默认 30（`nodes-ldsc/src/ldsc_sldsc.rs:438-456`）
- two_sample_mr clump 默认文档失实（kb=5000/p1=5e-8 vs R 10000/1e-5），宽松工具集被静默裁剪（`two_sample_mr.rs:601-604`）；local_ld 缺染色体表仅 warn，IV 独立性静默失效（:829-836）
- registry.register 用 HashMap insert **同名 kind 静默后写覆盖**；OpenTargets 两工厂重复注册两遍（`dag-core/src/registry/registry.rs:214`、`nodes-io/src/lib.rs:113-137`）
- get_node_ports_for_spec 不走 spec_normalize：同一 spec 在 build 被修复、在动态端口接线处报错（`registry.rs:285-291`）
- 全 svy 系 CI 用 z=1.96 而后端用 t(degf)：小自由度 NHANES 子域 CI 偏窄（`survey_common.rs:551` 等 3 处）
- svyglm std_errors(Bell-McCaffrey)/svyquantile qrule/svychisq statistic/svyciprop asin,beta/svykm group,se/svylogrank method,rho/svyolr method/calibrate 4 种 calfun/svynls/svymle——**大量 spec 字段是死的而 doc 虚标支持**（各文件行号见域报告）
- ml_classification_metrics macro/per-class 只兑现二类、roc_auc 硬编码 0/1 类；ml_knn 平票按 HashMap 迭代序决胜（进程间随机）；ml_isolation_forest c(n)≠标准公式分数偏低；ml_select_k_best 零方差完美特征 F=0 排最差；ml_stl 非 STL（刚性季节）；ml_changepoint 名为 PELT 实为 O(n²) DP
- radiomics directed Hausdorff 两列实为对称值（`radiomics_runner.py:808-813`）；ldsc .results 数值表未声明为输出端口；wgcna adjacency_stats 实为模块大小表、tom_stats 含 object.size 字节数
- 框架 render_template 非 ASCII 逐字节 Latin-1 化（`compile/render.rs:89`）；scan_template_refs 引号盲扫描
- figure_embed doc 称 JSON 输出实为 LaTeX 列且无转义；source_openalex group_by 文档两列实际三列
- 标识符列（gene_id/snp_id 等）CSV 推断成 Int 再 cast → **前导零丢失**（"079501"→"79501"）（`file_to_dataframe.rs:1043-1067`）

---

# 横切/系统性结论

1. **"doc 过度承诺、spec 字段死亡"是全仓最大模式**：至少 25 个 kind 存在 schema 收参数/字段但 execute 从不读、doc 宣称不支持的东西。审计发现的最危险子类是"科学含义不同的静默回退"（ward→average、fuzzy→sharp、C→γ、false→开、REF/ALT 方向、p→0）。
2. **p 值硬编码 0 共 4 个节点**（svyloglin/svyolr/svyivreg/svysurvreg）——任何用它们跑的结果都"显著"。
3. **后验抽样基础设施两处失效**（mice chol_factor 全零桩、logreg solve 误用）→ mice 家族多数插补器的参数不确定性没有进入插补。
4. **注册表无冲突检测**（last-write-wins），插件后加载会静默覆盖内置 kind；目前仅靠约定（coloc_abf 手工注销）。
5. **金标覆盖缺口**：cmest_weighting 有偏路径、mrlap mr_reverse 分支、ML 默认 CV 路径、插件 golden 测试在插件目录缺席时静默 skip（CI 上等于零覆盖）；e2e 表 24/26 用 SSH URL 且 #[ignore]；74 kind 只钉 73（h5ad_qc_filter 漏钉）。
6. **DL 层结构性名实不符**：transformer/RNN/VAE/deephit 四个名字对应的都不是该架构；训练配置大面积死亡；全局不可复现（无 Backend::seed）。
7. **engine 执行层本身干净**：runtime 三层隔离、spec_normalize 修复层、vfs 原子写、container_command 收尾校验均核验正确——问题全部在节点/插件层，不在调度层。

---

# 验证为正确、可直接信赖的锚点

- **survey 描述/检验层**：svymean/total/var/ratio/quantile(Woodruff)/table/ttest/ranktest/chisq(Rao-Scott)/ciprop(mean,logit)/glm(三明治)/km/contrast/by/standardize/rake/post_stratify/calibrate(linear)/trim_weights/regTermTest(Satterthwaite 1e-12)
- **MR 核心**：IVW/Egger(截距+Rücker Q')/加权中位数/众数/协调化(action1/2/3+回文容差)——79-SNP R golden 1e-7；coloc.abf ABF/先验/PP 归一 1e-12；BKMR MCMC bit-exact；mrlap 去偏校正本体
- **遗传后端**：LDSC 全链（regress/IRWLS/jackknife/twostep）、LCV 逐行对 R、genomic_sem 回归与 V 组装代数、magma_gene/meta(√N)、opengwas 取数
- **经典统计**：t_test/logistic/rlm(8位)/rrr/cox(Breslow,7位)/log-rank/cuminc(AJ+Aalen)/fine_gray(cmprsk 逐行移植)/nested_oof 与 fusion 防泄漏
- **RD/GRF/LCMM**：rdrobust 主路径(Senate 金标)/rdpower/rdmc；grf 树核心(grf-sys 官方 C++)/dr_scores/variable_importance/ATE "all" 聚类三明治/R-learner 一阶段/TrainOptions 默认值；lcmm 逐行移植+金标
- **ML 抽样**：pca/ica/nmf/varimax(对 R)/kmeans++/dbscan/lof/CART/PAM/OOF 泄漏防护/group kfold/C-index/Graf IPCW Brier(后端)/apriori
- **外部插件 21/26 家族干净**：deseq2/coloc/susie(0.16.6 逐行)/twosamplemr/hdl(默认值逐位一致)/lava/mixer/music/mvmr/mrpresso/hyprcoloc/smr/magma/pathway-gsea/plink2/pathology/timesfm/twas/visualization/single-cell(除 ucell 量纲已知裁定)/radiomics(主指标)
- **evalue 全链**对 R EValue 包一致；epi_roc AUC/DeLong/Youden/PPV-NPV 正确

---

# 修复优先级路线

**第一批（科学错误、行数少）**
1. rcs_basis 分母一次幂（1 行 ×2 节点）
2. 4 个节点 p=0 → t(df)/χ² 真值
3. mediation m_under_control 补协变量均值项
4. wilcoxon_paired 用 x−y；MW 单侧方向
5. mrlap qnorm 单侧化
6. mice chol_factor 实现 + logreg β̂+Lz
7. rdrobust fuzzy 接线或硬报错
8. file_transform MultiGzDecoder

**第二批（系统性）**
9. mtag/ldsc bool = "true" 判等（或框架层 bool presence 语义）
10. gcta cma.cojo 输出修正
11. svyivreg/svycoxph estfun 重写（R 公式已给出）
12. grf auto-X 排除 weights 列；predict 按列名对齐
13. DL Backend::seed + 优化器/调度分派；假架构改名或实现
14. ml_tsne θ=0.5；ml_svm C/γ 分离
15. registry 冲突检测（error on duplicate）

**第三批（契约清欠）**
16. 全仓"死 spec 字段"清单化：删除或实现（约 25 kind）
17. 深尾 p 用 sf 路径（gsem/chi_square/survival/sem/cmprsk 五处 1−cdf）
18. 插件 e2e 表补 https + h5ad_qc_filter；golden 测试缺目录时 warn 而非 skip

---

## Batch-1 执行记录（2026-10-04，分支 fix/node-audit-batch1）

8 项全部落地，本地测试全绿：epi 119 lib + xval、hypothesize 113、mice 6、
mrlap 6、nodes-hypothesize 11、nodes-rd 3、nodes-io 98、nodes-survey 40 lib
+ svy_rcs golden 1。

补充发现（第 1 项执行中）：

- **svy_rcs golden 原为循环验证**：`gen_svy_rcs_reference.R` 注释自称
  "identical closed form to epi::rcs::rcs_basis"，把 Rust 的平方分母 bug
  原样抄进 R 侧生成参考值——两侧同错故 golden 一直绿。已同步修正 R 生成
  脚本（一次幂分母 + 注释说明 λ+μ=1 约束），本机安装 survey 4.5 后重新
  生成 `svy_rcs_reference.json`。修正后 Rust 与独立修正的 R 参考在 1e-12
  内一致（截距 6.7424627421641…），golden 1e-8 门通过；旧代码对修正参考
  确认失败。教训入档：**"与实现 X 同一公式"生成的 golden 不构成独立验证**。
- rdrobust fuzzy 采纳硬报错方案（SpecRejection at build），doc/spec 注释
  同步标注 NOT IMPLEMENTED。
- golden 再生成依赖：本机 R 4.6 用户库装 survey 4.5 + jsonlite；注意
  `R_LIBS_USER` 与实际安装目录（`~/R/x86_64-pc-linux-gnu-library/4.6/`）
  不一致时，install.packages 子进程看不到已装依赖，需 `export R_LIBS=…`
  直传。

---

## Batch-2a 执行记录（2026-10-04，分支 fix/node-audit-plan-touch）

范围裁定：只修「与 BONE_MARROW_CACHEXIA_MASTER_PLAN_v3.9 相触」的 4 项
（P0 #4、P0 #17、P1 two_sample_mr local_ld、P1 hypergeometric fold）；
svycoxph/cmest_weighting 等不在该方案主方法链上的仍留 batch-2b。

1. **donor_composition 斜率 SE**（P0 #4）+ **golden 追加发现：线性预测子
   先 `.exp()` 再 sigmoid**— `fit_binomial` 有两个独立 bug：
   (a) 收尾取 `h[1][1]/det`（= Var(intercept)），改为 `h[0][0]/det`
   （= (H⁻¹)₁₁ = Var(slope)）；
   (b) 迭代与最终 Hessian 两处的概率均为
   `(intercept + slope*x).exp().sigmoid()` = σ(e^η)，拟的根本不是
   logistic 回归——报告的 `log_odds_ratio` 量纲全错（6 供者夹具上报
   −4.777，真值 −1.099），且 SE 在错误模型上计算。审计时只核了 H⁻¹
   代数没做端到端对照，漏了 (b)；R glm golden 一跑即现形——再次印证
   「golden 必须独立实现」。两处 `.exp()` 删除。
   原节点零测试覆盖；新增 `tests/donor_composition.rs` golden，参考值由
   R 4.6 base `glm(cbind(k, n-k) ~ x, family=binomial,
   control=glm.control(epsilon=1e-14))` 独立生成（非本 crate 公式回放），
   并以闭式解双重锚定（平衡两组夹具：slope=−log 3 精确、SE=√(52.5/675)）。
   注意 R 默认 `epsilon=1e-8` 下 IRLS 提前停：slope 差 2e-10、SE 差 1.2e-6
   ——「R 输出」当 golden 必须收紧收敛控制，否则门设在 R 自身噪声里。
   6 供者×2 细胞类型用例下 slope 1e-12、SE/z/p 1e-9~1e-10 内一致；
   修复前 slope −4.777/SE 0.394 对参考 −1.099/0.27889 确认失败。
2. **two_sample_mr local_ld 缺染色体表**（P1）— 由 warn+跳过改为硬报错：
   `HarmoniseInput` 无染色体列，无法判定哪些 SNP 落在缺表染色体上，
   继续执行等于静默断言"未检验的独立性成立"。报错信息指向
   `wjixiang/catalog-ldmatrix-1000g-eur` bundle 或切回 opengwas 模式。
   新增 `local_ld_missing_chromosome_table_errors` 单测（deregister
   chr2 表后断言 Err 且报错点名染色体与"independence"）。同节点顺带修正
   spec doc 失实：默认 r²=0.001/kb=5000/p1=5e-8 **严于** R TwoSampleMR
   的 clump_kb=10000/clump_p1=1e-5，外部预选工具集会被裁剪，需显式调
   p1——原文错误宣称这些是 "standard TwoSampleMR parameters"。
3. **hypergeometric_ora_ondf fold_enrichment 倒置**（P1）— 第 5 元由
   `expected/hits` 改为 `hits/expected`。新增
   `fold_enrichment_is_observed_over_expected`（双基因集、population=8
   用例：hits=2、expected=0.75 → fold≈2.667，倒置方向输出 0.375 必挂）。
   原有单测只数行数，对该字段零断言——单基因集夹具下 fold 恰为 1.0，
   正反两方向都过，故须双集夹具才能锁定方向。
4. **dataframe_to_file append 模式丢列**（P0 #17）— `append_existing`
   原对旧文件按新 schema `select` 投影：旧文件多出的列被静默丢弃并覆写。
   现两个方向都硬报错：旧文件列 ⊄ 新 schema（丢列）或新列 ⊄ 旧文件
   （union 必炸）均拒写，报错点名冲突列；写失败不触碰原文件。新增
   wider-onto-narrow 与 narrower-onto-wider 两个方向的单测，断言报错含
   列名且原文件字节未变（id 列回读校验）。

测试：nodes-hypothesize（lib + donor_composition golden）、nodes-sql、
nodes-mr（lib + 新增 2 例）、nodes-io（lib + 新增 2 例）。

合并注意（PR #95）：origin/main 的 25ed57c7 已把原生 two_sample_mr 节点
整体迁到容器插件并删除源文件（"complete the TwoSampleMR container
migration"），故本分支的 two_sample_mr.rs 为 modify/delete 冲突，
dataframe_to_file.rs 亦被同提交重写（Utf8View 物理投影）。处置：
local_ld 硬报错修复只对原生节点血统有效（含已部署 daemon，batch-1
血统仍带原生节点）；donor_composition 与 hypergeometric 两处上游未动
可干净落位；append 丢列修复需在 25ed57c7 的新写路径上人工移植。
容器插件路径（R TwoSampleMR 脚本）不走 local_ld，无此缺陷。
