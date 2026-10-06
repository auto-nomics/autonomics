# 修复对账账本（fix-ledger）

追加文件，不改动本目录任何原审计内容。基线：引擎 main@`825e9e0c`，审计基线插件见 nodes.json。
修复执行：2026-10-05，7 条并行 agent 线（3 引擎 worktree + 4 插件组）。全部交付经 team-lead 独立重跑复验。

## 范围

- **修复**：F01–F15 全部 15 项 + 审计「其他节点」4 项（magma_set GLS、magma_meta 连线输入、gsem 远尾分位数、table_from_df 类型渲染）。
- **不修**（按审计排期）：plan_* 适配层、MVMR gencov、singscore 新实现、MRlap LD 面板端到端、eaf 回文规则——已在各 PR 列为越界项。

## 引擎 PR（全部只开不合，base=main@825e9e0c）

| PR | 分支 | tip | 变更 | 覆盖缺陷 | 独立复验 |
|---|---|---|---|---|---|
| [#97](https://github.com/auto-nomics/autonomics/pull/97) | fix/audit-f06-misc-nodes | `62aa40e6` | 8 文件 +1277/−139 | F06 mrlap LDSC 吃 harmonised 表；F13 诚实零模型文档；magma 完整 GLS；magma_meta；gsem qchisq_sf 根求法（40 个 R 金标 ≤1e-12）；table_from_df 类型 | 189/189（verify-trackC-all-01.log） |
| [#98](https://github.com/auto-nomics/autonomics/pull/98) | fix/audit-f03-f05-hypothesize | `fabc0ad0` | 13 文件 +1482/−126 | F03 boolean_test NA 传播；F04 adjust_pvalues 全批家族+null 保行；F05 t_test 完整配对（审计反例 3.6667→4.5）+CI/SE/conf_level | 228/228（verify-trackB-all-01.log） |
| [#99](https://github.com/auto-nomics/autonomics/pull/99) | fix/audit-f02-f15-render-fingerprint | `65f69645` | 24 文件 +743/−116 | F02 布尔值语义 + `flag` presence 类型 + 27 处迁移 pin 翻转；F15 指纹纳入 source_revision（域 v1→v2）+ 增量元数据哈希 WARN | 336/336 部署插件根（verify-trackA-all-01.log） |

复验命令口径：`cargo test … --offline -j 2`，共享 target `~/.cache/fix-targets/shared`，日志在 `~/.cache/fix-targets/verify-track*.log`。

## 插件分支（本地仓，无远端 PR 机制）

| 家族 | 仓 | 分支 | tip | 测试证据 | 关键文件 SHA256（前16位） |
|---|---|---|---|---|---|
| single-cell (F01) | `~/.autonomics/plugins/single-cell` | fix/ucell-official-algorithm | `ba985f63` | 21 pytest，官方 UCell 2.16.0 R 金标 ≤1e-14 | workflow.py `0936dd22196356cf`* |
| spatial (F08/F09) | `~/Work/node-plugins/spatial` | fix/audit-f08-f09-spatial | `48a512ea` | 20 pytest；20k 细胞基准 208MiB/2.2s（旧 12k 细胞 3633MiB/200.8s） | xenium_ingest.py `d931326c14045a06`、spatial_nhood.py `171dcc1f8481686d` |
| pseudobulk (F07) | `~/Work/node-plugins/pseudobulk` | fix/audit-f07-pseudobulk-validation | `73a13c5a` | 14 pytest | pseudobulk_counts.py `41efc943b7b156f5` |
| lme4 (F10) | `~/Work/node-plugins/lme4` | fix/audit-f10-lme4-kenward | `dcf8765a` | 23 R checks（KR 与官方同会话对平） | lme4_fit.R `ec05e89a6883a8b8` |
| refdata (F11) | `~/Work/node-plugins/refdata` | fix/audit-f11-string-network-type | `30c3f691` | 21 pytest | string_ppi.py `389a4d79045b46f0` |
| netprox (F12) | `~/Work/node-plugins/netprox` | fix/audit-f12-proximity-contract | `95fa2ee3` | 12 pytest | network_proximity.py `1854446a4f9f8408` |
| cmap (F14) | `~/Work/node-plugins/cmap` | fix/audit-f14-gctx-orientation-direction | `75a2ca3a` | 11 pytest | cmap_connectivity.py `55ada0e98425ab29` |

\* workflow.py 哈希取自 fix 分支检出态（见下方事件记录）。

部署拓扑：`~/.autonomics/plugins/{spatial,pseudobulk,lme4,refdata,netprox,cmap}` 为指向 `~/Work/node-plugins/*` 的符号链接——上述仓当前均检出 fix 分支，即**部署面已在修复态**。single-cell 不是符号链接，是独立仓。

## 事件记录

- **deseq2_migration 环境红（非缺陷）**：审计缓存 `~/.cache/audit-plugins/deseq2` manifest 比仓库测试 golden 新（镜像 digest `8b2e2a78…`、无 `lfc_shrink`），未改动 main 上同样 3 红、同样原因；部署插件根下 4/4 过（post-all-03 与独立复验均 336/336）。已在 PR #99 描述定性。
- **single-cell 部署目录曾被切回旧版**：2026-10-05 验收时发现工作树 detached 于原始 `1d681ea`、磁盘 workflow.py 为旧公式（操作来源不明，疑并行会话）；已恢复检出 fix/ucell-official-algorithm@`ba985f63` 并复验（25 处 max_rank/ties_method 锚在位、树净）。**容器镜像是 baked（Dockerfile COPY workflow.py）——重建镜像前，实际容器运行仍是旧公式**。

## 待办（按序）

1. 人工合并 PR #97/#98/#99（仅开不合纪律）。
2. UCell 分支推送拍板：ZJ-2002 对 auto-nomics/single-cell-plugin 无 push 权（403 实测）——授 push 重推 / 授权 fork+跨仓 PR / 保持本地分支，三选一。
3. single-cell 镜像重建 + `test_single_cell_workflow.sh` 容器冒烟（其余 6 家 manifest 参数走 env 注入，现有镜像直接生效；lme4/pseudobulk Dockerfile 无需改）。
4. daemon 换件按既有流程（换件前查 `5b9f1160` 是否为部署二进制祖先；PR #96 已关，修复存档 wt-wedge/fix/delivery-confirm）。
5. main 合并后在冻结审计反例上重跑等价断言作换件后 smoke（UCell 反例=1.0、(0.001,NA)→null、双批 adjust、paired 4.5、mvmr as.logical）。

> **2026-10-05 状态批注**：待办 2——用户表态「现在还推送不了」，暂取本地部署路线（见下节），三选一留待权限到位再拍板。待办 3 已完成（见下节）。待办 4/5 由下节本地集成路线执行。

---

## 2026-10-05 本地部署收口（无推送权路线）

用户无 single-cell-plugin 推送权（403），要求本地即刻用上修复。两级落地：

### single-cell（F01）本地镜像部署 — 完成

- **分支拓扑**：`fix/ucell-official-algorithm` 保持可 PR 的干净形态，tip `806c2e9`（= UCell 修复 `ba985f63` + 新增 write 修复）；`local/ucell-deploy`（tip `25ac232`）叠加 manifest repoint，当前检出 = 部署态。
- **镜像**：`localhost/autonomics/single-cell-preprocessor:0.2.1-ucellfix`，manifest digest `sha256:5069c3ffaac0b11dfe950c1cd026f4195317be47b529b730572d9e670ad0f59b`；manifest `[image].reference` 已按 6 家兄弟惯例改为 digest 钉扎（localhost/ 前缀=本地存储，podman 不外拉）。上游旧 digest 保留在注释中供对照。
- **workflow.py 新 SHA256 前 16 位**：`d21d3fb1d83635cf`（806c2e9 检出态；上表 `0936dd22196356cf` 为 ba985f63 态，作废）。
- **验证链**（三层）：宿主 21 pytest 金标复跑全绿（uv 临时 env，官方 UCell 2.16.0 R 金标 ≤1e-14）；容器内冻结审计反例 **恰好 1.0**、exit 0；仓库 `test_single_cell_workflow.sh`（BUILD_IMAGE=0）PASS。镜像内无 pytest，宿主层已补。
- **新缺陷（容器冒烟发现）**：anndata 0.11.4 / pandas 2.2.3 运行时下 `write_h5ad` 拒写 `pd.arrays.StringArray`——uns 字符串列表（ucell missing_genes 汇总）触发，算法算对但落盘失败。宿主 pytest 从未覆盖（宿主无 anndata 环境）。修复=write 前 `ad.settings.allow_write_nullable_strings = True`（806c2e9），容器内最小复现验证：不 opt-in 失败、opt-in 写+回读 OK。

### 引擎本地集成（PR 未合先本地用）— 进行中

- 集成基座选 `fix/delivery-confirm@762dbcb4`（= main@825e9e0c 全量 + 楔死修复 5b9f1160/54f6f45e），再干净合入 #97(`62aa40e6`)/#98(`fabc0ad0`)/#99(`65f69645`) 三分支 → `local/engine-fixes-20261005@e94e82f1`，零冲突。任何部署物都是纯超集，换件无回退面。
- 编译 worktree `~/Work/wt-local-deploy`，CARGO_TARGET_DIR `~/.cache/localdeploy-target`，release build 进行中；随后 `-j 3 --release --offline` 全家测试（含 -p gateway -p runtime）作换件门禁。
- 与 wt-wedge 会话（autonomics-7a）协调：对方无换件计划，本超集二进制定为终态；其预警 gateway swagger 集成测试在楔死分支 7 挂 1（归属判定中），结论到达后决定是否放行换件。
- 换件后按 [[daemon-restart-agent-recovery]] 重放注册：换件前已快照活注册表 3 agent（network/environment/mr）于 `~/.autonomics/backups/agents-snapshot-20261005.json`。

---

## 2026-10-05 外部复审裁定与 R01–R09 处置

外部复审（`fix-review-2026-10-05.md`）结论：「不能验收为全部修复，也不能认证为全部与官方定义一致」。逐条独立复核后**认可该裁定**：R01–R09 九项反例中 8 项本地独立复现（R06 采信复审运行 + emreg toolbox 参考）。处置分三级：

### 阻断换件的引擎侧缺陷（已修，进 PR #98）

| 项 | 缺陷 | 修复 | 证据 |
|---|---|---|---|
| R02 | 非配对 wide 两列被按整对删缺失（把 paired 语义错扩到 unpaired）；R `t.test(x,y)` 逐列独立删 | `parametric.rs` y_column 分支改逐列提取；两个 wide 金标按逐列删除 R 运行重造 | `unpaired_wide_independent_na_matches_r_review_r02`（est 11/3、t 1.33394593769983、p 0.29254219697638）；hypothesize+nodes 全套 93 测试绿 |
| R03 | mu 只进单样本分支；paired/two-sample 静默忽略 mu | `t_test_paired/t_test_two` 线穿 mu0（t 统计量平移、CI 不移、extras 记 null_value），全分支传 `spec.mu` | `paired_mu_is_honored_matches_r_review_r03`（mu=1 → t=1、p=0.5）+ xval 2 金标 |

提交 `17f3955c`（fix/audit-f03-f05-hypothesize，已推，PR #98 只开不合）。金标教训：R02 的错误前提来自工单 WO-4 手推金标，违反「金标必须独立生成」——本轮全部金标改由 Rscript `options(digits=15)` 现场生成。

### 插件侧缺陷（不阻断换件，本轮已修）

| 项 | 家族 | 修复 | tip | 测试 |
|---|---|---|---|---|
| R01 | pseudobulk | `quote(safe="")` 永不转义 `_`（RFC3986 unreserved）→ 碰撞回退仍重复；显式 `.replace("_","%5F")` + 单射断言；测试改锚**原始 TSV 表头**（pandas 会把重复列名改名 `.1` 造成假通过） | `f0723c1` | 14 pytest |
| R05 | netprox | auto 表头嗅探弃用「非数字且不在其余行」启发式（单边文件 `A\tB` → 空图）；改已知表头记号**白名单小写精确匹配**；大写/非常规表头需 `has_header=yes` 显式声明 | `f204763` | 17 pytest |
| R06 | netprox | separation 对齐 emreg00/toolbox `get_separation`（jorg-closest）：d12 两向平均、对称（路径图 S={A,B}/T={B,H} 两向 −1.75；旧单向 −3.0/−0.5）；单基因集合内距按 toolbox `values=[0]` 代入（可估，不再 NA）；不可估组合保 NA 行 + log 记因，全表不可估仍退 2（证据文件先写）；mc_se 明示为随机距离均值的 MC SE | `f204763` | 同上（含反例锁定） |
| R08 | spatial | `block_column` 改为**同时限制建图**：逐块 kNN 并回全局索引，合并切片不再产生跨切片边（反例：两切片偏移 0.1、k=1，旧 3 条全跨切片；新 4 条全切片内）；块内细胞 < k+1 时有效 k 缩小并 WARN | `78d582d` | 24 pytest |
| R07 | single-cell | 「matches UCell 2.16.0」限定为**正向签名子集**：签名语义（Gene+/Gene− 后缀剥离、positive−w_neg·negative）、全缺失签名→0、w_neg 参数均**未实现**，为显式输入契约限制（文档限定，非算法改动） | `92d140e`（local/ucell-deploy，pin 已同步） | 21 pytest |

### 迁移测试卫生（R04 附带发现，已修，进 PR #99 分支）

复审证实：NODE_PLUGINS_ROOT 指向的根缺某家族时，该家族迁移测试打印 skipping 但计为 passed（账本 336/336 因此不可等价解读为全部迁移节点实际验收）。修复：28 个测试文件的 `plugin_root()` 助手改为**显式根缺家族即 panic**（红而非静默绿）；未设变量走默认路径时保持安静跳过。三态验证：默认根（mtag 4/4 真跑绿）、显式空根（4 panic，报错可操作）、编译无回归。

### 不本轮修复、立账处理

- **R04 本体**（旧 presence 脚本 mtag h2.sh / ldsc munge.sh 在 bool 值语义渲染下 `force:false` 生成 `--force`）：属既有插件升级工单——MTAG/LDSC 家族 manifest 迁移到新 `flag` 类型前不得安装这两家。工单落 `docs/audits/.../` 目录（见 workorders 附录）。
- **R06 深层**（表达/可检测性匹配、经验 P 的 MC 精度门控、箱不足缩样的零模型偏离）：WARN 措辞已升级为「偏离冻结零模型」；完整实现属 plan_network_background 任务范围。
- **R09**（指纹不含插件 manifest/script/image 身份；E01 资源门控 max_concurrency/内容哈希/内存 guard 默认未动）：静态设计缺口，无缓存污染运行反例；设计工单待立。
- **R13/R14**（CellPhoneDB/CellChat/NicheNet 官方实现、WTCS/NCS/tau）：复审认可现状为「诚实标记的探索性实现」，官方对拍不在本轮。

### 复审部署观察的回应

- plugins.toml 单 cell pin 确曾仍指 `1d681ea` 且 sync 会回退 checkout——已定位机制（`sync.rs` HEAD≠pin → reset 到 pin），pin 更新为本地 tip 后 sync 反而**固化**修复态（离线容忍：`cat-file -e` 本地命中即不 fetch）。现 pin `92d140e`。
- 7a（wt-wedge 会话）swagger 结论已到达：判「环境红/偶发」（同内容三跑 1 挂 2 绿、不可复现、main 对照绿、分支无 utoipa/路由面改动），**放行换件**；其 PR #96 头部 `762dbcb4` 已含进集成基座。
