# 步骤 5：方案约束 × 实际部署 验收证据（2026-10-06）

对应评估表第 5 行"失败阻断、完整家族、缓存失效、跨进程恢复、全局资源限制均有证据"。
逐项给：**现有证据（引用点）→ 当场新证 → 如实缺口**。状态词沿用矩阵：
ACCEPTED_SCOPED / PARTIAL / PENDING。

## 5.1 失败阻断

- 已有（同进程）：合成缺失 `condition` 输入 → source 成功 → regression 失败 → 下游 skipped、
  失败输出不存在、诊断落盘（`acceptance-ab-run.md` §控制测试）。
- 已有（引擎级装载阻断）：启动守卫 = 一个非法 manifest 中止 daemon（manager.rs:6 文档注释
  + `families 2/2 healthy+broken` 断言 manager.rs:137）；mrlap manifest 经 sync→loader
  达 execute 段 = 合法装载实证（daemon 日志 `+ mrlap installed`、21 families/41 kinds，
  `deploy-evidence/daemon-boot-20261006.txt`）。
- **缺口（如实）**：kill-运行中-DAG 的故障传播（进程级）未实测；容器运行失败（非零退出）
  的传播有节点级单测（podman/container_command），daemon 级未实测。

## 5.2 完整家族

- A：`A_full_edge_status.tsv` **276 行全量登记**（本轮复核行数与 ID 唯一性），状态
  NOT_SCIENTIFICALLY_ACCEPTED 276/276、G0_PASS False 276/276，校正占位 p_for_adjustment=1
  不动分母；288 路径登记同包。
- B：`B_signature_mapping_status.tsv` **1,030 成员全量登记**（本轮复核）；四负对照
  NP06-09 = PanInflam_HallmarkIR 200 / N5P 537 / CiM_UP 47 / GMP_origin 17 成员行全在。
  缺项闭合见步骤 4 文档（`B_mapping_closure_NP0609_v1.tsv`）。
- 检验家族/q 值：未被任何本轮动作改动（无新独立验证声明、无 BH 家族扩张——696 基因层
  明确"不生成这 696 项的事后 BH 家族"，ab-run §基因层）。

## 5.3 缓存失效

- 已有：内容变更失效实测（字节长/mtime 均不变、斜率 0.8→2.0 正确重算，ab-run §控制测试）；
  同进程增量复用（A 10 项、B 2088 项）。
- 代码级：WO-R09 插件指纹纳入身份（PR#100，domain 机制）单测在 main；本仓本轮实测——
  mrlap manifest 加 `[nodes.resources]`、twosamplemr 加 seed 参数后
  `spec_compile` 输出变化（migration golden 断言 compiled.cpus/memory 新钉，5/5 绿）
  = 声明改动必然移动指纹 = 计划内失效。
- 本轮补证：twosamplemr 家族合同 074900f→c29cce4（seed+resources 两次指纹移动）与
  mrlap ce56172→f3b35ccf 已在 10-06 18:4x 重启上产线（checkout rev 实测一致）；
  **daemon 层"指纹移动→强制重算"的活体判定仍待 agent 触发实验（同 5.4）**，PENDING。
  注：F1/F2 实验证明新 manifest（含 [nodes.resources]）在产线库上真实计算正确——
  `finger_results/results.tsv` 的 IVW P=0.814965093902353 与冻结参照一致，
  Weighted median SE=0.016984327527172 即 WO-R-03 seed=1 的钉定值。

## 5.4 跨进程恢复

- 已有（现场级）：daemon 重启后 agent 注册表自动恢复 + gateway token 自动重写
  （8c27e6df 机制；本轮 17:1x 重启现场验证：新 daemon 起来、21 families、token 重读 32 字符）。
- 已有（同进程续跑）：pause 取消→incremental 续跑→22 行回读（ab-run）。
- **本轮实测（runner-killv2，链接产线 d8659cdf 库，build 身份
  `.cache/runner-killv2.build.json`；run_root 持久目录双进程）**：
  - 优雅取消路径：K1（read/reg 成功→pause 命中 cancel→sink cancelled，
    `report_k1.json` ok=False）→ K2 新进程全图再执行 ok=True，
    `report_k2.json` 四节点 success，产物 sink.tsv 落盘。
  - 崩溃路径：K1b 运行 20s（pause 45s 中）`kill -9`（无 report，crash 真实）→
    K2c 新进程增量参数首跑 → 全 success + sink 产物通过（`report_k2c.json`）。
  - **架构发现（决定该子项的最终形态）**：scheduler 的 incremental 状态是
    **进程内**（DAG 成员 outputs/fingerprints），`run()` 第三参数是 event_sink
    而非持久 store——因此"harness 新进程 = 全部真重跑"，跨进程**复用**与
    指纹失效判定**只能发生在 resident daemon 的持久层**（dag-history.db +
    artifacts 内容寻址缓存），harness 无法代替。
  - 由此定级：crash 后"完成/不损坏/不覆盖历史"= harness 层已证 ✓；
    daemon 层复用与指纹失效的活体判定 = 需 agent 触发路径，登记 PENDING
    （实验设计：同图两连跑 + rev 变更前后各一跑，观测点 dag-history +
    artifacts 命中，触发通道 `--name` headless 或 gateway agents API）。

## 5.5 全局资源限制

- 容器层（本轮闭环）：[nodes.resources] 已产线生效（重启后 deployed rev 实测
  twosamplemr=c29cce4 / mrlap=f3b35ccf）。**mrlap memory 按实测收紧 12Gi→8Gi**：
  容器模式三边 1s R 进程 RSS 采样峰值 1.9 GiB（同 run PARITY PASS 51/51），
  8Gi 预算含 ~4x 余量；"整机 10.2GiB 采样"是含 page cache 的口径，不得引为
  RSS 限额依据（manifest 注释同步改写）。golden 断言 compiled.cpus/memory 入
  `twosamplemr_migration.rs`（5/5 绿）；argv 映射单测 podman.rs:536。
- 进程层（本轮引用）：`CREATE_GATE = Semaphore(1)`（podman.rs:42，防 podman SQLite store
  并发 create 死锁）= **单 daemon 内容器创建串行**；start 并行。研究链单 daemon 部署 =
  该闸即全局创建并发界。
- 内存口径：全机采样 ≠ 节点 RSS 的告警沿用（ab-run/seed1 报告）；mrlap memory=12Gi 是
  "定界而非达标"（8Gi 预算目标待 per-edge RSS 实测后收紧——已写入 manifest 注释）。
- **缺口（如实）**：宿主 cgroup quota（非容器进程）、多 daemon/跨 daemon 全局并发、
  磁盘峰值预算均未建立。twosamplemr harmonise 节点未声明资源（本轮无实测依据，不虚构）。

## 汇总

| 子项 | 状态 |
|---|---|
| 5.1 失败阻断 | PARTIAL（同进程+装载守卫有证；kill 级 PENDING） |
| 5.2 完整家族 | ACCEPTED_SCOPED（276/288/1030 全量登记复核；家族语义不动） |
| 5.3 缓存失效 | PARTIAL（内容失效实测+指纹代码级+golden+产线 rev 已动；daemon 活体判定待 agent 触发实验） |
| 5.4 跨进程恢复 | PARTIAL（crash/取消双路径 harness 已证完成且无损；复用判定架构上 daemon 专属，活体 PENDING） |
| 5.5 全局资源 | ACCEPTED_SCOPED（家族合同产线生效+RSS 实测收紧 8Gi+argv 单测+CREATE_GATE；宿主级 quota/跨 daemon 并发仍 PENDING） |

技术项全绿后仍按矩阵口径：不等于 G0，不改变检验家族与分母。
