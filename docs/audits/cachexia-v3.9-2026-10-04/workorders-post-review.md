# 复审后续工单（fix-review-2026-10-05 遗留项）

复审裁定认可（R01–R09），本轮已修 R01/R02/R03/R05/R06/R07-doc/R08 及迁移
测试卫生（详单见 fix-ledger.md「2026-10-05 外部复审裁定与 R01–R09 处置」）。
以下为**不在本轮修**、已立账的工单。立账原则：每张工单开工前不得部署受
影响家族，避免带病上线后再返工。

---

## WO-R04：MTAG / LDSC 旧 presence 脚本 flag 语义迁移

- **来源**：复审 R04。旧渲染器把 bool 参数值渲染进 presence 检查脚本
  （`h2.sh` / `munge.sh`）的字符串比较，`force:false` 在旧语义下仍拼出
  `--force`。引擎已迁到新 `flag` 类型渲染，但两家插件 manifest 仍是旧
  presence 写法。
- **范围**：mtag、ldsc 两家族 manifest——bool 参数改为 `flag` 类型渲染
  （`--force` 仅在 true 时出现），presence 脚本不再按字符串比较渲染
  参数值；各自 golden/pytest 同步。
- **前置**：无。
- **硬门**：**迁移完成前不得安装/部署这两家**（当前本地 15 家部署不含
  它们，维持现状即可）。
- **验收**：`force:false` 渲染产物不含 `--force`；`force:true` 含；两家
  pytest 全绿；引擎侧 container-plugin 迁移测试对新 manifest 跑通且
  **不再静默跳过计绿**（本轮已把显式根缺家族改为 panic，见 fix-ledger
  「迁移测试卫生」节）。

## WO-R09：指纹纳入插件身份

- **来源**：复审 R09。`compute_node_fingerprint` 只覆盖节点 spec 与输入
  数据指纹，不含插件 manifest / scripts / 容器镜像 digest——同 fingerprint
  下插件换实现不会失效缓存。当前无缓存污染运行反例（静态设计缺口）。
- **范围**：engine 指纹计算加入插件侧身份：家族 manifest 内容哈希、
  script_file 内容哈希、镜像 digest（single-cell 已 pin digest，本地
  镜像家族需 manifest 写全 reference）。注意与 `main-lacks-deployed-
  engine-fixes` 备忘同理：指纹口径变更 = 全量缓存失效一次，需在换件
  后首次冒烟里观察 artifact 重建量，避免误判引擎回归。
- **验收**：改 manifest 一个字符 → 对应节点 fingerprint 变化；改镜像
  digest → 变化；纯数据重跑 → 不变。单测覆盖三条。

---

复审另有两项**不立工单**的备注：R13/R14（CellPhoneDB/CellChat/NicheNet
官方实现、WTCS/NCS/tau 对拍）维持「诚实标记的探索性实现」现状，官方
对拍属计划 §17 表达/可检测性匹配之后的优先级；R06 深层（经验 P 的 MC
精度门控、箱不足缩样）归 plan_network_background 范围，netprox v3 已
以 WARN 措辞「偏离冻结零模型」诚实化。
