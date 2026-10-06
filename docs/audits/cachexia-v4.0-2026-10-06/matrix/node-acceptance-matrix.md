# 研究节点验收矩阵 v1（2026-10-06）

机读详表：同目录 `NODE_ACCEPTANCE_MATRIX.tsv`（逐列：作用、输入输出、官方版本、支持范围、证据、未结项、状态）。本文件只定口径、汇总结论、登记未结项账本。

## 口径

- **验收范围声明制**：每行的"支持范围"只覆盖证据列所指的具体输入批次、方法与参数分支；范围外的用法一律视为 PENDING，不得由"同节点运行成功"外推。
- **状态词汇**：`ACCEPTED_SCOPED`（声明范围内通过）/ `ACCEPTED_PARTIAL`（部分方法或部分分支通过）/ `PARTIAL`（工程语义部分证据）/ `PENDING`（未开始）/ `FORBIDDEN_UNTIL_VERIFIED`（禁止进入正式链直至认证）/ `EXCLUDED`（永久排除）。
- **三层合规互相独立**：①官方方法正确（对照方法包参考实现）；②研究设计合规（对照 v4.0 冻结方案：G0、276 边/288 路径家族分母、LEGACY_SEEN、检验家族）；③部署工程验收（失败阻断、缓存失效、跨进程恢复、全局资源）。**官方方法正确 ≠ 研究设计合规 ≠ G0 PASS。**
- 版本钉扎是行级字段：容器 TwoSampleMR 0.7.9 与宿主 MRlap 环境的 0.7.11 是两个独立记录的环境，禁止跨版本混测或互相背书。

## 汇总

| 状态 | 行 |
|---|---|
| ACCEPTED_SCOPED | linear_regression(B 基因层)、sql(B 模块层) |
| ACCEPTED_PARTIAL | twosamplemr(A 五方法+IVW 独立核验；seed 缺口已闭，仍余 clump/回文边界) |
| ACCEPTED_SCOPED（增） | mrlap_official：host+容器+部署 registry 三模式 51/51（范围=冻结三边输入） |
| FORBIDDEN_UNTIL_VERIFIED | 原生 Rust `mrlap`（M/jackknife/面板/EAF 回文四项未验证偏差，`crates/node-bundles/nodes-mr/src/mrlap.rs:10`） |
| PARTIAL | file_reference、file_to_dataframe、dataframe_to_file、失败传播、增量缓存/内容失效、同进程续跑 |
| PENDING | twosamplemr_harmonise、UCell/singscore 评分合同、LD 面板 catalog 注册 |
| EXCLUDED | acceptance_pause（harness 专用，永不进正式链） |

## 未结项账本（按放行优先级）

1. **WO-R-03（本批开工）TwoSampleMR 随机方法 seed 冻结**：`frozen_plugin/scripts/mr.sh` 全文无 `set.seed`，且默认 `method_list` 含 `mr_simple_mode`/`mr_weighted_mode`（bootstrap 随机层）。修复＝新增 `seed` 参数＋脚本内 `RNGkind("Mersenne-Twister","Inversion","Rejection")`＋`set.seed`，随 canonical 先例同规。脚本改动经 loader 内联改变插件指纹 → 预期一次缓存失效；image digest 不变。
2. **WO-R-02（本批开工）mrlap_official 容器插件化**：canonical seed1 host 验收已过（51 字段一致），待：image 构建钉扎、LD 面板 45 文件（`~/tools/genetics/ldsc_assets`，211M：22 染色体 LD-score gzip + M_5_50 + `w_hm3.noMHC.snplist`）注册进 panel catalog、部署后 DAG 复现。注意 MRlap 的 `Z/sqrt(N)` 标准化与 HM3 IV 子集不同于 conventional IVW，两列效应不得直接相减或择优。
3. **B 成员映射与评分合同**：四个新负对照完整人类成员映射与模块级冻结参照仍不充分（1,030 成员记录 826 可估计/204 缺项已登记于 `NOT_ESTIMABLE_LEDGER.tsv`）；z-mean/UCell/singscore 不混用须在 DAG 模板层面钉死。
4. **部署验收缺口**：跨进程恢复、manifest/脚本/镜像缓存失效全量证明、best-effort 哈希失败阻断、宿主 CPU quota/任务级 8 GiB 硬限额、跨智能体全局并发。全机内存采样≠节点 RSS，不得引用为单任务限额证据。
5. **技术≠科学**：以上全部通过后仍不自动构成 G0；276 边/288 路径分母、预定检验家族、q 值口径不因技术验收改变。

## 维护规则

- 新增研究相关节点 kind 必须先在本矩阵登记行（含官方版本与范围声明）才允许接入正式 DAG。
- 每完成一个未结项，更新对应行 `evidence` 与 `status`，证据文件封口哈希进验收目录 `EVIDENCE_SHA256SUMS` 同级登记；矩阵历史版本不改写，只追加 v2。

## 工单状态（2026-10-06 收口追加）

- **WO-R-03（seed 冻结）：关闭。** 详 `WO-R02-R03-EVIDENCE.md`；矩阵行 1 相应未结项已划除。
- **WO-R-02（官方 MRlap 插件化）：关闭（本机部署面）。** 镜像改钉+面板发布+51/51 host/容器双 parity 完成；daemon 重启装载 21 families（`+ mrlap installed`），**部署 registry 三边正式复现 51/51 ≤1e-12**（三 run 哈希见 deploy-evidence/）；矩阵行 4 翻 ACCEPTED_SCOPED。残留挂账：ghcr 版本级 public 复核（用户已翻包级，匿名拉取实测仍 401）、中心 registry PR 待 wjixiang 合并、其余机器部署时面板需经中心索引拉取。
- **步骤 4（B 成员映射与评分合同）：技术面关闭。** 四负对照 76/76 逐行结论落表 `B_mapping_closure_NP0609_v1.tsv`（56 双源无同源=映射闭合结论、10 映射成立但不在矩阵、10 签名定义缺陷移交方案侧）；评分合同发布 `STEP4_B_MAPPING_AND_SCORING_CONTRACT.md`。696 名单与封口账本不动。
- **步骤 5（方案约束×部署）：证据文档发布，逐项如实分级。** `STEP5_DEPLOYMENT_ACCEPTANCE.md`：276/288/1030 全量登记复核 ✓；资源声明入家族合同（twosamplemr/mrlap manifest，golden 断言，待下重启窗口生效）；kill-恢复与 daemon 级活体失效实测仍 PENDING（不冒充）。
- 技术全部到位仍不构成 G0；10 个签名缺陷行处置、8GiB 预算收紧、跨进程实验三项为下批挂账。
