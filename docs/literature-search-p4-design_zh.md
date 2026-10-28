# P4 混合检索实施细化

P4 精确排序补充（2026-10-28）：借用 Top-K 和最多 4 线程的逐块扫描已验证，保持块内 f64 累加与并列规则。冻结 release 合成测量中，100k 篇/700k 块/1024 维宽范围排序 P95 约 629→340ms，中位数约 515→240ms，所比结果/评分逐位一致；窄范围存在波动，完整分布如实保留。这里只测排序，不能代替端到端性能或持续写入验收。最终源码索引/API、单 CPU 回退与严格 Clippy 已通过，先前并行版本另有 419 项回归。见 [精确排序资格](benchmarks/literature-search-p4-exact-rank-linux.json)及[交付核对](literature-search-delivery-audit_zh.md)。

持续写入验收（2026-10-28）：真实 `BibCore` 词法检索与规范写入在语义 embedding 暂停期间连续运行 60.74 秒，至少 100 次新增写入均可立即检索；暂停中的语义代未发布，旧代保持可见，待处理同步显式返回 revision conflict，解除暂停后完整收敛。此项是有界结构验收，不代表生产吞吐或端到端延迟。见[持续写入收据](benchmarks/literature-search-p4-sustained-write-linux.json)。

当前执行（2026-10-28）：有界清理版本的独立构建/360 项后端及严格 Clippy、浏览器/模拟桥接、真实 Qwen 功能复核均通过。100k 篇/200k 附件/700k 块完整功能运行成功结束，含写入收敛、非首选附件原始字节与证据、删除/替换及旧游标失效，最终修订 500073、700k 块、无错误。共享主机宽查询热 P95 约 3.25 秒、四客户端约 9.04 秒；40 次写入是短突发，不能当作长期可用性验收。性能、最终原生/恢复与阶段隔离仍需收口，P4/P5 均未提交。见 [有界清理资格](benchmarks/literature-search-p4-bounded-cleanup-linux.json)。

上一轮状态（2026-10-28）：P0–P3 已提交，P4/P5 均未提交。全量 NeuCLIRTech 中文独立评价已完成：395,927 篇、110 查询，冻结 Qwen 配置的 nDCG@20 为 0.446672，词法为 0.300555，相对提高 48.62%；18 查询退化，混合后端单次查询 P95 约 4.95 秒，不能作为普遍质量或性能承诺。详见 [中文质量收据](benchmarks/literature-search-p4-neuclir-hybrid-linux.json)。最新 100k 附件恢复运行已终止：200k 附件导入/验证完成，700k 块初次就绪约 873.9 秒，但后续验证报 `database is locked`，未通过完整容量验收，确切失败操作仍在定位。P4 分批复制版本的隔离后端 354 项通过；索引选择修正后的 27 项专项回归及严格 Clippy 通过。P5 引文探索、确认导入和不可变搜索快照已接入并分别测试；订阅已有独立持久规则、租约、页面原子存储与窗口规划，调度/API/GUI 尚未实现，见 [订阅设计](literature-search-p5-subscriptions-design_zh.md)。后文旧轮次记录保留历史语境，当前状态以文首记录为准。

锁错误排查：独立副本中，原单条 SQL 复制 700k 绑定约 128.2 秒后复现相同 Busy 错误。现已改为每批 2,048 条、修订检查、续租和原子提交，并明确使用现有主键索引；Turso 默认给有界范围复制选择文章索引，会每批扫描整代。实际 700k 分批复制已完成，全部绑定内容 SHA-256 一致且旧发布头不变；调试构建耗时约 398.9 秒，期间存在其他构建/文件复制，不作生产性能或受控提速结论。100 篇端到端验证通过，100k 完整服务复测已终止：初始索引、宽/窄/四客户端测量和增量绑定复制完成，但持续写入后的发布阶段再次报 `database is locked`；初次 700k 块就绪约 1,531.9 秒，宽查询热 P95 约 1.58 秒，四客户端约 6.48 秒，均是共享主机观测且尚无完整成功收据。独立副本中，700k 绑定的未引用旧代级联删除也在约 159.2 秒后复现 Busy；真实发布阶段仍需细分定位，有界旧代/向量清理及该规模性能仍待处理，不能宣布容量通过。见 [故障与修复资格](benchmarks/literature-search-p4-binding-copy-linux.json)。

日期：2026-10-28。本文细化尚未完成的 P4，延续总体方案。评价工具、固定版本 provider、持久向量模块、后台配置服务、V2 混合查询/证据/导出，以及 GUI 主题模式和规则恢复已在工作区接通；默认仍为 lexical，需显式启用已安装模型与主题请求。重建、隔离修复、暂停/恢复和真实进程恢复已有验证；完整容量与恢复、中文质量尚未完成，尚未形成 P4 提交。真实实验见 [P4 评价记录](benchmarks/literature-search-p4.md)，服务/主链路分别见 [服务收据](benchmarks/literature-search-p4-service-linux.json)和[混合查询收据](benchmarks/literature-search-p4-hybrid-linux.json)，GUI 与真实 Linux 原生验证见 [GUI 收据](benchmarks/literature-search-p4-gui-linux.json)，剩余工作与排期见 [下一阶段计划](literature-search-next-plan_zh.md)。

## 查询意图与硬约束

给 V2 请求增加可选 `semantic_query`，专门承载主题描述。现有 `query` / `expression` 继续表示硬条件，仍二选一；filters、scope、回收站与权限保持原有含义。词法模式拒绝非空 `semantic_query`；混合模式要求非空主题描述，限制输入字节与词数，明确返回配置未就绪或索引未就绪。

例：主题描述“影响免疫细胞基因表达的调控机制”，高级条件 `year:[2020 TO 2026] NOT tag:excluded`。语义向量仅来自主题描述，后面的布尔条件在融合、计数、分面和分页之前生效。模型不负责重新解释或改写硬条件。

主题描述不经过查询语法 parser，避免把科学论断里的 AND、NOT、冒号和引号误当运算符。主题的词法分支采用生产 analyzer 的宽召回 OR，向量分支在相同硬条件与范围内召回，之后使用经调参集选择并冻结的 weighted RRF。标识符和专业布尔仍经原有契约执行；不把原有 AND 悄悄改成 OR。

GUI 的普通词法入口保持原行为；用户显式选择“主题检索”后，主输入对应 `semantic_query`，高级条件单独显示为硬筛选。粘贴规范 DOI/PMID 时可提供精确查找入口，不把标识符嵌入检索当作精确查找成功。无主题描述的纯高级查询继续使用词法执行。

## 固定配置与数据边界

文献语义服务独立启用，默认关闭。现有通用 embedding 服务的默认 `enabled=true` 不代表用户已经启用文献索引。配置至少包含 origin、已安装模型名称、预期 manifest digest、维度、query instruction、分块版本、窗口/重叠、推理 context/batch 参数、候选深度和 RRF 参数。配置在数据库持久保存；校验后计算 signature，不把可变标签或含凭据 URL 当作模型身份。

默认只使用 loopback。远端 endpoint 需要独立明确配置，GUI 在启用前显示会发送的标题、摘要、正文/批注范围；不沿用发现数据源的授权推定用户同意发送本地正文。首次不自动下载模型。模型未安装、digest 不符、输入过长和服务不可用均有可恢复状态。

已补充生产 `inference.num_ctx` / `inference.num_batch` 配置与 GUI 高级输入，分别校验 context 和 token 批大小；它们不是输入文献的 `batch_size`。缺省保留旧 provider 签名，显式参数进入签名并隔离向量缓存。参考 [Ollama embedding API](https://docs.ollama.com/api/embed) 和 [Runner 定义](https://github.com/ollama/ollama/blob/main/api/types.go)。后续 [长输入/原生验证](benchmarks/literature-search-p4-long-input-linux.json)已覆盖 4 篇真实 BGE 长摘要的超限拒绝、恢复、参数切换、38 块增量和重开，以及 Qwen/Linux 原生高级配置持久化；不将这些集成结果当作检索质量或大规模全文资格。

`PinnedOllamaEmbedder` 创建独立 digest 标签并逐批核验，失败丢弃整批；调用者的索引 signature 还应绑定 instruction、分块、推理选项和 schema。BGE 等非因果模型需要验证实际 token batch 能覆盖输入，不能仅依据模型卡的 context_length。任何截断必须成为明确的文本预处理规则及覆盖指标；首版优先拒绝超限并报告。

## 服务与一致性

向量索引 worker 使用独立数据库连接和有界队列。读取规范投影、文章 outbox revision 与 commit token 后结束读事务，再进行网络 embedding；模型调用期间不占用写入 actor 或词法检索队列。只有完成并验证的批次才能持久提交。写入、取消和服务重启都不能让半批向量成为可查询 generation。

索引键至少包含对象 docID、articleID、anchorKey、内容 hash/revision、窗口位置与预处理版本、模型 signature。持久状态记录 source revision 与 commit token，防止数据库恢复到同一数字 revision 的另一条历史时误认为已同步。原文始终来自规范对象；向量存储不是文本事实来源。

首版从标题/摘要开始，随后加入具备可靠对象位置的 SDT 段落及批注，分别显示覆盖。每次仅重算内容发生变化的对象；元数据筛选变化不重复 embedding。删除、移入回收站、附件替换与模型更换各有失效路径。更换模型建立新 generation，完成后原子切换；不同模型或预处理向量不混算。旧 generation 清理有明确边界。

查询 embedding 在调用方进入搜索队列前异步准备，携带模型 generation；搜索 actor 在数据库快照中验证配置和 source revision，再对范围及硬条件合格的候选计算向量分数并融合。索引落后时返回明确状态，避免把陈旧向量结果伪装成完整最新结果。该错误不阻断词法查询和存储写入。长计算在 blocking worker 中执行，不持有跨 await 的同步锁。

游标绑定规范索引 token、向量 generation、完整请求和有序候选快照。向量或源数据变化后旧游标失效。候选截断需在结果契约说明；“已召回候选数”不能当作全体语义相关文献数。分面同样注明统计对象。

## 排名、证据与容量

结果保持一篇文献一行，保留 lexical / semantic / both 的召回说明。语义命中展示真实窗口原文、对象身份及内容版本；没有字面匹配时不生成高亮范围，不把相似度伪称相关概率。阅读跳转复用 P2 的对象定位；位置不可靠时降级到真实文本查找。

先测量固定模型真实向量的精确余弦扫描，再决定是否引入 ANN。报告 10k/100k 的内存、持久体积、筛选后召回、首次/热查询、embedding 与总体时延，不能把纯矩阵扫描时间当作 GUI 时延。若采用 ANN，需相对同一合格候选集合的精确 Top-K 单独验收候选召回；科研相关性和 ANN 近似误差分别报告。候选有限重排同样在 train 决策，报告额外模型、成本和各组退化。

## GUI 与验收顺序

当前增量资格：真实 HTTP 与浏览器容量、ext4 测试副本的文件缓存建议清理、300 条英文词法排名不变性，以及隔离 GUI 生产构建已完成，见 [新增验收记录](benchmarks/literature-search-p4.md#http浏览器与磁盘启动复核)。浏览器计时为开发环境可见性加两帧动画代理，不能称为原生 IPC 或生产整站时延；`/tmp` 的早期记录来自 tmpfs，不作为磁盘冷启动证据。实际 ext4 100k 索引就绪单次约 16 秒，启动及资源边界仍需优化。中文盲评/导出工具已有 [工作流](literature-search-judgment-workflow_zh.md)，仍缺真实人工判断，不据此提前收口 P4。

主题模式有显式开关、模型和数据发送范围说明、覆盖/进度、故障与重建操作。模型不可用时允许切回词法；若客户端自动发起词法回退，必须在当前结果旁说明实际执行方式。新索引任务不清空已显示列表，也不改变当前选中对象。

实施顺序：真实模型与分块比较 → 冻结参数后的独立判断集 → 配置及持久增量 worker → 生产查询准备与融合 → 证据与 GUI → 正确性、容量与恢复 → 单独 P4 提交。维持现有工作区边界，不混入其他 GUI 改动。

确定性测试覆盖范围/权限/布尔排除前置、规范 ID Top-1、跨对象 AND、取消迟到、模型替换、内容替换与删除、同 revision 不同 token 的数据库恢复、故障批次与重启恢复、过期游标和词法独立可用。质量分别记录英文官方判断、中文真实/衍生判断的来源及限制；未达独立收益或中文发布条件时保留实验入口，不能凭训练集提分宣布 P4 全部验收。

维护增量验证（2026-10-28）：重建复用有效向量，隔离修复损坏派生库，暂停/恢复跨重启持久化；进程 SIGKILL 后按所有权租约接管并复用已提交批次。HTTP、浏览器/模拟桥接与真实 Qwen/Linux WebView 均覆盖维护入口，详见 [维护记录](benchmarks/literature-search-p4-maintenance-linux.json)。10k/100k 内部合成容量已测，见 [容量收据](benchmarks/literature-search-p4-capacity-linux.json)；并发/端到端容量、真实长批次压力与完整恢复矩阵仍待验证；英文参数继续冻结，中文人工判断及其他平台另行验收。

最新容量/恢复复核：[查询优化收据](benchmarks/literature-search-p4-query-optimization-linux.json)记录 252 项回归、真实 Qwen 与 Linux 原生 GUI、强杀恢复，以及受控模型 fixture 的 10k/100k 服务测量。100k 宽查询单客户端热 P95 约 386ms，四客户端约 1.47 秒；冷缓存、HTTP/GUI、多全文块与持续写入仍单独验证，中文真实论文质量和阶段隔离构建继续保留门槛。

后续 [磁盘启动优化](benchmarks/literature-search-p4-startup-linux.json)在同一读事务内解析所需向量行号，每批最多 256 个，避免按 chunk 哈希随机读取 BLOB，同时保留 checksum、归一化、缺失/范围检查及 chunk 排序。100k ext4 同源副本的配置至就绪单次约 16.1→5.94 秒、进程至首结果约 20.5→10.6 秒；热 HTTP P95 约 415ms，没有热查询改善的主张，RSS 增加约 31MiB。334 项回归和库/测试/新示例严格 Clippy 通过。多附件容量、持续写入、长时间批次、缓存配额、中文真实质量与隔离阶段构建仍待收口。

资源预算后续已完成实现及本机资格，见 [资源记录](benchmarks/literature-search-p4-resource-limits-linux.json)。默认 1M 块、256KiB 输入、每库 2GiB 向量载荷、同一文献库语义文件 8GiB 协作预算；预算独立于向量身份，载荷计数/并发核验在写事务内完成。达到额度保留规范数据与已发布代，普通 worker 等待相关变化；预算变更与未完成维护任务原子重绑定并隔离旧所有者。真实 Qwen 证明 16 块修复检查点保留、提高额度后只补 4 块；用户暂停保持暂停。349 项测试（含原 3 项可选提取用例）、严格 Clippy、实际原生及隔离 GUI 构建通过。该轮 100k 热 HTTP P95 约 420ms、就绪单次 5.13s，不宣称普遍满足性能目标；持续/多附件压力和质量门槛仍保留。

最新增量（2026-10-28）：100/10k 多附件真实上传/提取/VFS 与删除失效验证已通过；非首选附件删除漏发 outbox 已改为事务内发布，350 项后端测试通过。100k/700k 块测试仍在运行，连续写入的语义可用性与长时间批次尚未验收。独立阶段源码快照的实际 Linux Rust/Tauri/Qwen 功能测试通过，见 [多附件收据](benchmarks/literature-search-p4-attachments-linux.json)。中文材料现已取得并固定为 NeuCLIRTech 全量原生中文文献和 110 条官方人工判断查询；转换及 nDCG@20 工具验证完成，尚无中文检索分数，见 [来源记录](benchmarks/literature-search-p4-neuclir-source-linux.json)和 [冻结策略](benchmarks/literature-search-p4-neuclir-frozen.json)。P4 未提交，P5 未开始。

最新主题/中文增量：中文主题按固定词典分词后 OR，科研符号保留原样；300 条英文查询 AST 与旧策略完全一致，353 项后端回归及最新隔离 Linux 原生通过。全量中文 395,927 篇、110 查询的词法 nDCG@20 为 0.300555，实际固定 Qwen 混合计算仍在运行，尚无收益结论。100k 原上传尝试的 500 已保留现场，恢复导入在独立副本继续；不提前报告容量完成。详见 [主题资格](benchmarks/literature-search-p4-topic-policy-linux.json)。P4 未提交；之后已开始独立 P5 快照原型，见 [P5 设计与提交边界](literature-search-p5-design_zh.md)，尚未接入产品。
