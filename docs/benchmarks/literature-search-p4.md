# P4 真实相关性评价（进行中）

当前执行（2026-10-28）：有界清理版本的独立构建/360 项后端及严格 Clippy、浏览器/模拟桥接、真实 Qwen 功能复核均通过。100k 篇/200k 附件/700k 块完整功能运行成功结束，含写入收敛、非首选附件原始字节与证据、删除/替换及旧游标失效，最终修订 500073、700k 块、无错误。共享主机宽查询热 P95 约 3.25 秒、四客户端约 9.04 秒；40 次写入是短突发，不能当作长期可用性验收。性能、最终原生/恢复与阶段隔离仍需收口，P4/P5 均未提交。见 [有界清理资格](literature-search-p4-bounded-cleanup-linux.json)。

上一轮状态（2026-10-28）：P0–P3 已提交，P4/P5 均未提交。全量 NeuCLIRTech 中文独立评价已完成：395,927 篇、110 查询，冻结 Qwen 配置的 nDCG@20 为 0.446672，词法为 0.300555，相对提高 48.62%；18 查询退化，混合后端单次查询 P95 约 4.95 秒，不能作为普遍质量或性能承诺。详见 [中文质量收据](literature-search-p4-neuclir-hybrid-linux.json)。最新 100k 附件恢复运行已终止：200k 附件导入/验证完成，700k 块初次就绪约 873.9 秒，但后续验证报 `database is locked`，未通过完整容量验收，确切失败操作仍在定位。P4 分批复制版本的隔离后端 354 项通过；索引选择修正后的 27 项专项回归及严格 Clippy 通过。P5 引文探索、确认导入和不可变搜索快照已接入并分别测试；订阅已有独立持久规则、租约、页面原子存储与窗口规划，调度/API/GUI 尚未实现，见 [订阅设计](../literature-search-p5-subscriptions-design_zh.md)。后文旧轮次记录保留历史语境，当前状态以文首记录为准。

锁错误排查：独立副本中，原单条 SQL 复制 700k 绑定约 128.2 秒后复现相同 Busy 错误。现已改为每批 2,048 条、修订检查、续租和原子提交，并明确使用现有主键索引；Turso 默认给有界范围复制选择文章索引，会每批扫描整代。实际 700k 分批复制已完成，全部绑定内容 SHA-256 一致且旧发布头不变；调试构建耗时约 398.9 秒，期间存在其他构建/文件复制，不作生产性能或受控提速结论。100 篇端到端验证通过，100k 完整服务复测已终止：初始索引、宽/窄/四客户端测量和增量绑定复制完成，但持续写入后的发布阶段再次报 `database is locked`；初次 700k 块就绪约 1,531.9 秒，宽查询热 P95 约 1.58 秒，四客户端约 6.48 秒，均是共享主机观测且尚无完整成功收据。独立副本中，700k 绑定的未引用旧代级联删除也在约 159.2 秒后复现 Busy；真实发布阶段仍需细分定位，有界旧代/向量清理及该规模性能仍待处理，不能宣布容量通过。见 [故障与修复资格](literature-search-p4-binding-copy-linux.json)。

日期：2026-10-28。P4 尚未完成，当前产物包括评价工具、真实模型诊断、固定模型版本、持久向量索引、后台服务、V2 主链路和 GUI 主题检索闭环。hybrid 需显式启用配置并提交主题；默认 lexical 保持不变。重建/修复/暂停/恢复与 SIGKILL 接管已验证；完整容量/恢复和中文真实质量尚未完成，尚未形成 P4 阶段提交。

## GUI 与原生闭环资格

[GUI/Linux 收据](literature-search-p4-gui-linux.json) 记录：前端类型检查与构建通过；浏览器和模拟 Tauri 桥接覆盖显式主题提交、中文 IME、主题/关键词历史隔离、保留硬条件、嵌套 AST 规则与 URL 重载、候选计数和双路说明、语义原文、第二附件定位及返回、候选池导出、显式取消、故障保留旧结果、关键词回退和远端数据选择。760px 窄窗口无横向溢出，截图已在测试目录外保存。原有关键词/搜索阅读/发现回归和实际 Linux 词法 WebView 回归通过。

实际 Tauri WebKitGTK 经真实 Rust API、IPC、已安装 Qwen manifest digest 和 embedding 服务完成模型发现、GUI 配置启用、全文范围构建、主题/年份条件、候选说明、命中第二个 TXT 附件、原文定位及返回、持久规则、WebView 重载和显式停用。该测试为一个构造条目和两个真实上传提取的 TXT 附件，不是文献相关性或 PDF 页坐标验收。模型未下载，未覆盖原始标签；未启用配置的关键词界面不读取模型元数据或发起语义查询。

配套模型元数据 API 验证鉴权、显式远端选择和固定身份；本轮后端库/API 合计 241 项通过，3 项保持忽略，后端库与原生示例严格 Clippy 通过。特性测试直接挂载文献工作区，生产构建来自当前工作区；完整阶段提交前仍需排除其他并行 GUI 改动并独立构建，未同步覆盖已有 gateway 产物。复现命令为 `npm run test:bib-semantic` 与 `CARGO_BUILD_JOBS=2 npm run test:bib-semantic-native`（后者需要已安装匹配的 Qwen 模型和 Linux WebKitGTK/Xvfb）。

## 混合查询主链路资格

[混合查询收据](literature-search-p4-hybrid-linux.json) 记录本轮 270 项测试通过（库单元 217、API/发现/搜索工作流 53），3 项保持忽略；相关库与实际模型探针示例的严格 Clippy 通过。新增结构用例经公共 BibBase actor 执行主题检索，覆盖硬布尔、年份与集合在 Top-K 前生效、110 条排除结果不能挤掉合格候选、候选池计数、两路说明、无字面匹配时无高亮、Unicode 真实摘要、模型/源变更拒绝旧准备状态与游标、保存规则、规范 DOI 和约束导出。阻塞 embedding 不阻塞词法队列；HTTP 用真实本地模型协议 fixture 验证固定 digest、truncate=false、503/400 状态、主题检索、证据和候选池导出。

实际 Qwen release 探针另经 32 篇公开 SciFact 文献完成启用、混合查询、两页去重、真实证据、全部候选导出、年份硬筛选、内容更新、删除及重启。一次首屏后端查询观测约 73.1ms，包含查询 embedding，但不包含 GUI；不作为性能分位数或冷缓存指标。语义池导出 32 篇、删除后 31 篇，年份条件仅保留指定文献；重启维持删除后的向量 generation。游标 generation 同时绑定融合策略版本。

这项主链路资格不新增相关性分数，现有 SciFact 独立验收参数保持冻结；后续 GUI 资格另见上节。100k 容量、完整进程恢复和维护、中文真实判断与跨平台验收仍待完成。

## 后台服务阶段资格

2026-10-28 新增 [服务收据](literature-search-p4-service-linux.json)：全库单元 178 项、API/本地搜索工作流回归 47 项通过，相关库与两个索引/服务探针示例的严格 Clippy 通过。另有 2 项库单元和 1 项 API 用例保持忽略，未算作通过。新增服务用例覆盖默认关闭、配置验证与持久化、重开复用向量、跨句柄配置观察、损坏配置修复、模型切换、取消构建/查询以及模型故障时词法检索仍可用；HTTP 测试验证配置路由沿用鉴权。

实际 Qwen 0.6B digest 固定的 release 探针经 BibBase 导入 32 篇公开 SciFact 文献，再通过配置启用后台索引。首次 ready 观测约 3.49 秒，同期一次词法请求约 13.1ms；这是本机单次生命周期观察，不是冷缓存或性能分位数。元数据、摘要更新均发布新 generation，删除后剩余 31 块；数据库重开后 generation 保持，停用配置再次重开仍生效。向量存储文件加 WAL/共享内存文件合计 848,224 字节，不能只引用主文件的 4,096 字节。

上述服务初次资格是索引与服务接入证据，不新增相关性分数，也不单独证明混合查询、GUI、中文、100k 向量容量或进程 SIGKILL 恢复；后续主链路资格另见上节。

## 数据与评价边界

使用 [BEIR SciFact](https://huggingface.co/datasets/BeIR/scifact) 的 5,183 篇文献、809 条官方 train 查询和 300 条官方 test 查询。输入固定到 Hugging Face 仓库 revision 和逐文件 SHA256，原文、判断及向量留在操作者明确选择的目录，不放进 Git。数据集卡标注 CC BY-SA 4.0；保留原始 [SciFact 项目](https://github.com/allenai/scifact) 的来源及使用要求。

train 只用于选择模型、分块与 RRF 参数；test 用于参数冻结后的独立验收。本轮已经按此顺序完成一次英文诊断验收，冻结配置见 [candidate](literature-search-p4-frozen-candidate.json)，完整收据见 [SciFact/Linux](literature-search-p4-scifact-linux.json)。官方查询划分不等于能够证明模型预训练未接触该数据。

该任务评估英文科学论断与文献的相关性，不能替代中文论文任务、规范 ID、复杂布尔、权限、正文证据定位、真实库和 GUI 的验收。Recall 相对于已发布判断，未判断文献按不相关计；不能解释为全体相关文献召回。中文同义短句的真实模型探针只是集成验证，不是中文检索质量分数。

指标采用 [BEIR 的评价定义](https://github.com/beir-cellar/beir/blob/main/beir/retrieval/evaluation.py)：nDCG@10、MRR@10、Recall@50/100。nDCG 使用 [trec_eval 的线性 judgment gain](https://github.com/usnistgov/trec_eval/blob/master/m_ndcg_cut.c)。相同数值分数按运行文件已有的明确排序计算。所有已判断查询必须有结果行，空结果计零；缺行、重复结果、未知文献、非有限分数或调参/验收 ID 重叠会失败。成对 bootstrap 固定 seed 20261028，2,000 次重采样；区间只描述本判断集的查询采样不确定性。

## 可复现运行

在仓库根目录创建独立虚拟环境，使用 `scripts/requirements-bib-relevance.txt`。以下 `DATA` 指向操作者选择的数据目录。

```bash
python scripts/prepare_bib_relevance.py --directory "$DATA"
CARGO_BUILD_JOBS=2 cargo build -p bib-base --example search_relevance --release --locked --offline
target/release/examples/search_relevance "$DATA" "$DATA/queries-train.jsonl" "$DATA/lexical-or-train.jsonl" or
target/release/examples/search_relevance "$DATA" "$DATA/queries-train.jsonl" "$DATA/lexical-and-train.jsonl" and
```

词法运行经过生产 BibBase actor、规范数据库投影、Tantivy、文献聚合与 V2 返回，不另写一个近似 BM25。自然语句以显式 AST 输入，避免被当成高级查询语法。AND 把原句作为文本 term，保留生产分词的全部 token 约束；OR 以含字母数字的空白词组为子句，仍由生产 analyzer 处理科研符号。它是宽召回诊断基线，不声称等同 Lucene/BEIR 的标准 BM25。专业布尔查询的产品语义保持不变。

[Qwen3-Embedding-0.6B](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B) 的模型卡标注 Apache 2.0。实际安装的 Ollama 模型 manifest digest：`ac6da0dfba84a81fdbfbaf330198c33cd77c4cdfc53e8bc50eb581914a15621d`，输出 1,024 维。本轮创建独立别名 `bib-p4-qwen3-embedding-ac6da0dfba84:latest`，没有覆盖原有模型标签。诊断脚本只允许 loopback，缓存将 digest、精确输入文本和推理选项绑定；每批前后核对别名 digest，禁止混入变化后的模型。

```bash
OPENBLAS_NUM_THREADS=2 python scripts/benchmark_bib_embeddings.py \
  --dataset "$DATA" --queries "$DATA/queries-train.jsonl" \
  --model bib-p4-qwen3-embedding-ac6da0dfba84:latest \
  --digest ac6da0dfba84a81fdbfbaf330198c33cd77c4cdfc53e8bc50eb581914a15621d \
  --cache "$DATA/qwen-vectors.sqlite" --output "$DATA/qwen-whole-train.jsonl"

python scripts/fuse_bib_relevance.py tune --dataset "$DATA" --split train \
  --lexical "$DATA/lexical-or-train.jsonl" --dense "$DATA/qwen-whole-train.jsonl" \
  --parameters "$DATA/qwen-whole-rrf-parameters.json" --output "$DATA/qwen-whole-rrf-train.jsonl"

python scripts/evaluate_bib_relevance.py --dataset "$DATA" --split train \
  --run "$DATA/qwen-whole-rrf-train.jsonl" --baseline "$DATA/lexical-or-train.jsonl" \
  --output "$DATA/qwen-whole-rrf-train.metrics.json"
```

输出与参数文件要求使用新文件名，避免覆盖旧实验。SQLite 向量缓存可跨实验复用；原始文本变化导致新缓存键。两种分块输入分别为标题加完整摘要，以及标题加 2,000 Unicode code point 摘要窗口、重叠 200 code point。它们不是 tokenizer token 数。查询前缀固定为 `Instruct: Given a scientific claim, retrieve scientific papers that provide relevant evidence.\nQuery: `，文献无 instruction。[Ollama embed API](https://docs.ollama.com/api/embed) 显式 `truncate:false`，防止静默丢失尾部内容。

向量诊断采用单位向量精确余弦、同一文献多个窗口取最高分，返回前 100 篇。它不是生产向量索引，也没有被当作 ANN、增量 outbox 或 GUI 验收。RRF 在 train 上搜索 dense 权重 `{0.25,0.5,1,2,4}`、常数 `{10,30,60}` 共 15 组，词法权重 1，候选深度 100。参数文件记录 train 输入校验和；`apply` 使用固定参数，不在 test 上重新选择。

## 初步结果（仅调参集）

| 方法 | nDCG@10 | MRR@10 | Recall@50 | Recall@100 |
| --- | ---: | ---: | ---: | ---: |
| 生产词法严格 AND | 0.02472 | 0.02472 | 0.02472 | 0.02472 |
| 生产词法宽召回 OR | 0.58393 | 0.55505 | 0.82820 | 0.87536 |
| Qwen 完整标题摘要 | 0.74236 | 0.71507 | 0.93284 | 0.96168 |
| Qwen 2,000 字符窗口 | 0.74588 | 0.72009 | 0.93037 | 0.96168 |
| 完整标题摘要 + RRF（dense=4，常数=10） | 0.74448 | 0.71523 | 0.94335 | 0.96477 |
| BGE-M3 完整标题摘要 | 0.71196 | 0.68318 | 0.91595 | 0.94149 |
| BGE-M3 + RRF（dense=4，常数=10） | 0.71911 | 0.68876 | 0.92357 | 0.94994 |
| Qwen RRF 前 20 条按同模型余弦重排 | 0.74236 | 0.71507 | 0.94335 | 0.96477 |

RRF 相对 OR 的 nDCG@10 提升 27.50%，成对 bootstrap 绝对差区间约 `[0.1380,0.1834]`；298 条提高、63 条下降、448 条不变。它是 train 上选择参数后的诊断成绩，不能充当 +10% 独立验收通过证据。严格 AND 在完整论断上有约 97.4% 空结果，不能把它单独作为主题检索的弱比较基线。

窗口对完整摘要的 nDCG@10 相对提升约 0.47%，绝对差区间约 `[-0.00094,0.00846]`，不支持稳定收益结论；多了 565 个文献向量，常驻矩阵由约 21.23 MB 增至 23.54 MB。完整摘要诊断处理语料及 train 查询约 222.6 秒（含实际服务、缓存与校验），精确余弦扫描热 P95 约 0.44ms。后者排除了查询 embedding、数据库、筛选、证据和 GUI，不能当作端到端时延。当前没有基于该脚本证明实际使用 CUDA。

[BGE-M3 模型卡](https://huggingface.co/BAAI/bge-m3) 标注 MIT、1,024 维；依据其说明不加 query instruction。官方 Ollama GGUF blob SHA256 为 `daec91ffb5dd0c27411bd71f29932917c49cf529a641d0168496c3a501e3062c`，F16 文件 1,157,671,200 字节。通过官方 registry 下载并核验，使用官方 blob/create API 导入为独立测试模型；导入后的 manifest digest 为 `818da0ac2fdaa4b622ef97f76819ea24490fc60684fd267cb8261a4a4a353ae3`，不冒充官方原始 manifest。原 pull 的 CDN 解析被服务网络检查拒绝，未降低检查策略。

BGE 首次默认 token batch 在长摘要上返回 413；该部分运行不计算成绩。用最长摘要单独核验：仅增大 `num_ctx` 仍失败，同时显式设置 `num_ctx=8192,num_batch=8192` 后返回 1,024 维、2,156 个输入 token。完整实验用同样选项重新编码所有语料，选项参与缓存键，保持 `truncate:false`，没有复用旧默认选项向量。完整运行约 89.3 秒；由于推理选项和运行时状态不同，不把两次观察时长当作受控硬件速度对比。

有限重排实验只对 RRF 前 20 篇重新按同一 bi-encoder 的精确余弦排序，保留其余结果顺序；没有另用 cross-encoder。结果未优于融合，故冻结候选关闭重排。

## 参数冻结后的英文 test

模型与分块比较、RRF 网格及有限重排完成后，冻结 Qwen 完整标题摘要、dense 权重 4、RRF 常数 10、候选深度 100，不启用重排，再取得 test 查询向量并计算成绩；没有在 test 上选择模型或参数。

| 方法 | nDCG@10 | MRR@10 | Recall@50 | Recall@100 |
| --- | ---: | ---: | ---: | ---: |
| 生产词法宽召回 OR | 0.56873 | 0.54202 | 0.82339 | 0.86589 |
| Qwen 完整标题摘要 | 0.70262 | 0.66333 | 0.91333 | 0.94667 |
| 固定 Qwen + RRF | 0.71263 | 0.67320 | 0.93000 | 0.95000 |

相对词法 OR，nDCG@10 提高 **25.30%**，绝对差约 0.14390，成对 bootstrap 95% 区间 `[0.10794,0.18088]`；110 条提高、33 条下降、157 条不变。英文公开判断集的候选实验达到原定相对 +10% 目标；这没有验证生产混合索引，也没有通过中文、真实库、规范 ID/布尔或 GUI 门槛。后续主链路若改变预处理、候选或融合规则，须重新明确评价用途，不能把本次独立验收集反复用于选参。

## Provider 与后续实施

`PinnedOllamaEmbedder` 通过独立 `autonomics-embedding:<完整 digest>` 标签固定已安装模型，每批前后验证；签名含 digest、维度、截断规则和归一化版本，调用方仍须把分块和 instruction 版本放入索引键。不会下载模型或覆盖原始标签；目标别名存在且 digest 不符时返回错误。HTTP 错误正文和带凭据的地址不会放进错误输出，禁止跨 origin 重定向。provider 拒绝缺失/多余/错误维度/零向量及重复输入 ID。普通 Ollama provider 同时修正单输入空返回会 panic 的问题，并关闭静默截断。

真实中英探针经 Rust provider 成功产生两个 1,024 维向量；相似度约 0.814，仅作为集成回执。确定性测试、严格 Clippy 和探针回执分别记录，不混称相关性测试。

后续重点：容量、中文领域判断、长批次与更广恢复场景，以及原文映射/平台验收。查询、并发与失效细化见 [P4 实施设计](../literature-search-p4-design_zh.md)。下载失败时保持原有网络检查，不降低检查策略。P4 完成后再形成单独阶段提交；P5 尚未开始。

维护增量验证（2026-10-28）：重建复用有效向量，隔离修复损坏派生库，暂停/恢复跨重启持久化；进程 SIGKILL 后按所有权租约接管并复用已提交批次。HTTP、浏览器/模拟桥接与真实 Qwen/Linux WebView 均覆盖维护入口，详见 [维护记录](literature-search-p4-maintenance-linux.json)。10k/100k 内部合成容量已在后文记录；并发/端到端容量、真实长批次压力与完整恢复矩阵仍待验证；英文参数继续冻结，中文人工判断及其他平台另行验收。

## 容量基线与快照发布优化

新增 `semantic_capacity_benchmark FRESH_DIRECTORY COUNT`，使用生产 SemanticIndex、真实 Turso 持久文件和确定性 1024 维合成向量。此基准不运行模型，不计算相关性，也不包含 HTTP/GUI、硬条件求值或全文多块扩张；first observed 是建索引后的首次观测，不是操作系统冷缓存。每个范围单线程测 30 次排名，并分别报告元数据更新、重开、RSS、主文件及 WAL/共享文件体积。

[优化前收据](literature-search-p4-capacity-baseline-linux.json)保留 10k/100k 初始测量。在本机 release 的 100k 条、每篇一块场景，全范围热 P95 为 117.98ms，1% 范围为 1.92ms；仅改一条元数据的发布耗时 3,089ms，重开 1,368ms，新旧向量重叠时峰值 RSS 为 1,089,932KiB。该观察指出全量向量重载的成本，不能用排名时间替代整体查询或维护 SLO。

后续实现让新 reader 复用未变化对象的不可变向量，仅装载变更对象，保留旧 reader 的内容与身份。相同输入跨批次共享向量；发布前核对绑定数，重启仍从持久库校验加载。另增加请求等待期的临时代心跳，以及无需重启的过期代回收。正确性测试覆盖旧 reader、删除/替换、重复输入、重开和被阻塞请求期间的续期。

[共享向量后的收据](literature-search-p4-capacity-linux.json)记录相同合成数据和 1024 维配置的新一轮 release 测量：10k/100k 元数据发布单次约 136/1,978ms，100k 峰值 RSS 约 772MiB（优化前约 1,064MiB）。100k 全范围热排名 P95 约 123ms，1% 范围约 2.04ms；不能宣称排名提升。100k 首次构建约 9.52 秒，排除真实模型推理；重开仍约 1.37 秒。向量主文件、WAL 和共享文件合计约 879MiB，说明仅报向量数值大小不足以描述磁盘占用。

共享后后端库 195 项、API/本地工作流 48 项通过，3 项既有忽略项不计；其中 36 项为语义子集，不能重复累加。严格库与容量示例 Clippy 通过。仍保留发布瓶颈：元数据修改复制 bindings、全量扫描单 worker、模型及全文扩张成本，以及连续写入与备份分歧的完整恢复矩阵。当前容量结果支持下一轮优化判断，不构成整个 P4 完成或 GUI SLO 达标。

## 备份分支与运行恢复补充

恢复回归构造了一个较高 revision、但变更分布来自另一分支的源状态，重开旧 sidecar 后原逻辑确实漏掉 outbox 水位之前的变更；新增测试先复现失败，再修正首次同步策略。本地 Tantivy 和语义索引的新句柄均不再以较高 revision 作为 ancestry 证明。源版本一致时仍复用；源版本不同则先完整枚举并复用有效向量，运行中的正常更新保留增量路径。此回归验证受控分支状态，不能代表操作系统文件替换、远端备份服务或运行中覆写数据库的资格。

共享向量改动后，真实 SIGKILL 验证仍复用已提交 16 块并补算 4 块，实际 Qwen/Linux 原生 GUI 维护与证据流程也重新验证。较新代码、终态日志及范围以 [恢复补充收据](literature-search-p4-recovery-linux.json)为准；历史容量收据保留当时二进制和源 hash，不改写为最新代码的测量。

## 元数据发布与服务并发容量

[绑定复用收据](literature-search-p4-binding-alias-capacity-linux.json)记录后续优化：当变更对象的全部 chunk 身份均相同时，新逻辑 generation 引用原物理绑定集，旧根由外键与回收条件保护；连续元数据修改直接指向同一根，避免引用链增长。真实内容替换、删除及显式重建继续物化绑定，发布后回收不再引用的旧根。新路径保留 source/version、profile、head CAS 和绑定数量检查；失败事务不改变活动 head。

相同 1024 维合成库中，10k/100k 单次元数据发布约 5.83/62.19ms（前一轮约 136/1,978ms）。100k 运行中峰值 RSS 约 702MiB，主文件/WAL/共享文件合计约 507MiB。全范围精确排名热 P95 仍约 123ms，不宣称排名改善；这仍是索引内部测量。

新增 `semantic_service_benchmark` 与 `benchmark_bib_semantic_service.py`，沿生产 BibBase 服务/actor 执行分面、20 条文章装载、四客户端、变更时写入/词法检索及缓存重建。模型端是有界 loopback HTTP fixture，提供确定性 1024 维向量；不代表真实推理速度，也不含 HTTP API/GUI。每范围 30 次单客户端、20 次四客户端、维护期间各 10 次样本，首次观测与热分位数分别报告。

[服务容量基线](literature-search-p4-service-capacity-baseline-linux.json)显示：10k/100k 宽查询单客户端热 P95 约 75/866ms，四客户端约 290/3,359ms；100k 1% 范围仍约 581ms。100k 缓存重建期间，词法 P95 约 8.2ms、混合约 949ms，写入样本 P95 约 2.7ms。100k 宽查询未达计划时延目标，不能用内部 123ms 扫描数字宣称服务达标。代码存在逐词项重复读取存储字段的路径，下一轮通过 segment 身份缓存减少该成本，再以相同配置比较。

中文候选审查见 [数据集来源记录](literature-search-p4-chinese-dataset-audit.json)。固定 revision 的 MedicalRetrieval、CovidRetrieval 数据卡没有给出 license metadata，viewer 示例分别是医疗问答/网页回答及疫情政策/新闻，不能据此声称已取得中文科研论文相关性判断。未下载语料或计算中文质量分数；后续另行论证可用的迁移诊断与真实论文验收集。来源：[MedicalRetrieval](https://huggingface.co/datasets/C-MTEB/MedicalRetrieval)、[CovidRetrieval](https://huggingface.co/datasets/C-MTEB/CovidRetrieval)。

[文档身份缓存对照](literature-search-p4-doc-identity-capacity-linux.json)记录下一轮相同配置：10k/100k 宽查询单客户端热 P95 约 57/620ms，四客户端约 227/2,439ms；100k 1% 范围约 354ms。缓存按不可变 segment ID 复用，更新/删除/失败回滚及显式段合并均有资格用例。100k 宽查询仍未达目标，继续优化逐词项排序和合格范围外对象的中间装载；当前不把这轮改善视作容量收口。

[中间排序/范围前置对照](literature-search-p4-scored-collection-capacity-linux.json)中，100k 1% 范围热 P95 降到约 78.5ms，宽查询仍约 654ms，不能宣称宽查询改善。随后借用不可变元数据、仅为最终词法 Top-100 创建完整对象，并使用已关闭的合成库副本进行查询对照：[借用候选收据](literature-search-p4-borrowed-candidates-capacity-linux.json)。100k 宽查询热 P95 约 565ms、四客户端约 2.11 秒，仍未达目标。该轮重用既有持久向量，fixture 每库只处理 92 个查询输入，不作为初次建库耗时；单客户端首次和热分位数仍分别记录。副本来源收据 hash 与复用标记随记录保存。

[CPU 采样](literature-search-p4-query-profile-linux.json)在 100k 宽查询热阶段收集 957 个 cpu-clock:u 样本，未丢样；排他样本中 String clone、哈希及词法候选处理仍有明显占比，精确向量排名约 10.76%。这是诊断，不合并到无采样的容量分位数。由此继续借用临时匹配键，并在内部映射复用已有依赖 ahash 的随机状态；持久 SHA256 指纹和结果确定性排序不变，后续以回归和新测量验证。

## 最新查询优化复核

[最新资格记录](literature-search-p4-query-optimization-linux.json)汇总完整回归、真实模型/原生 GUI/强杀恢复与[新的服务容量收据](literature-search-p4-borrowed-keys-capacity-linux.json)。按采样结果，临时匹配键改为借用不可变文档身份，内部映射使用已存在、固定版本 ahash 的随机状态；全覆盖范围经过实际成员核对才省略重复过滤，不以集合大小推定权限。持久 SHA256、BM25、证据优先级和 RRF 配置不变。

本机合成 fixture 中，10k/100k 宽查询单客户端热 P95 约 48/386ms，四客户端约 136/1,466ms。100k 单客户端观测值低于建议的 400ms 目标，四客户端仍有明显队列成本；这不是 HTTP API/GUI 或真实模型推理的端到端验收。副本复用的 92 个 embedding 输入全是查询，未重算 10k/100k 文献向量；初次建库证据继续引用独立的 fresh 运行。

当前后端库及 API/本地工作流共 252 项通过、3 项既有用例忽略，严格 Clippy 与目标格式检查通过。实际 digest 固定的 Qwen 对 32 篇 SciFact 文献完成初建、混合查询、元数据更新、正文替换、删除、重开及停用；元数据更新报告 1 块复用、0 块新增。Linux 实际 WebKitGTK 再次通过主题/硬条件、第二附件证据定位、规则/重载和维护操作；真实 SIGKILL 后仍复用 16 块、补算 4 块并返回 20 篇候选。这些是功能和恢复资格，不新增相关性分数。

该轮之后的 HTTP/浏览器、磁盘文件缓存与隔离 GUI 构建验证见下节。更多全文块和持续写入、中文真实论文判断、其他平台/人工可访问性、持久缓存配额与 Rust 阶段隔离构建仍需收口。P4 尚未形成阶段提交，P5 尚未实施。

## HTTP、浏览器与磁盘启动复核

新增 `crates/api-server/examples/semantic_http_benchmark.rs`，通过真实 loopback TCP、Bearer 鉴权和 JSON 编解码调用生产路由。模型仍为确定性 1024 维 HTTP fixture，使用已关闭合成库的独立副本，不能据此推断真实模型速度或论文相关性。未授权请求返回 401，延续分页无重复；每库 84 个 embedding 输入均为查询，未重算文献向量。每种单客户端查询 30 次、四客户端共 20 次，返回 20 篇文章及分面。

[磁盘 HTTP 收据](literature-search-p4-http-capacity-linux.json)使用 `/mnt/projects` 的 ext4 副本，逐文件 fsync 后请求 `POSIX_FADV_DONTNEED`。只对测试副本操作，不全局清理缓存；该建议不能保证操作系统及物理存储所有缓存均冷。先前 `/tmp/bib-p4-http-cold-capacity-20261028` 位于 tmpfs，不能解释为磁盘冷启动，其较短启动数字不用于磁盘 SLO。

| 库规模 | 配置至索引就绪，单次 | 进程至首个结果，单次 | 首个查询，单次 | 宽查询热 P95 | 1% 范围热 P95 | 四客户端 P95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 10k | 1,088ms | 1,449ms | 251ms | 46.6ms | 34.7ms | 152ms |
| 100k | 16,144ms | 20,538ms | 3,794ms | 414ms | 64.6ms | 1,558ms |

100k 的约 16 秒加载和约 20.5 秒首结果暴露了磁盘启动成本，热查询也不能一概宣布满足 400ms 目标。共享参考主机、首次单次观测、热分位数与四客户端排队分别解释，不把向量内部扫描数字替代这些结果。

[浏览器收据](literature-search-p4-browser-capacity-linux.json)使用真实 Chromium、生产 `gatewayRequest`、现有 Vite 鉴权代理与同一真实 HTTP 后端，没有 route mock。后端就绪后，每库 20 次不同主题的 Enter 提交；每次恰好一个请求，page size 为 50。10k/100k 从 Enter 至首条新结果可见并经过两帧动画的 P95 约 **178/532ms**。它包含自动化观察开销，是绘制代理指标；使用热文件缓存与开发环境功能挂载，不代表生产整站、原生 IPC 或物理显示延迟。初次功能挂载时延另存收据，不能混入 Enter 分位数。

截图检查发现左侧 Library 曾误用候选数，已改为独立的库总数，候选数继续留在结果区。浏览器回归和实际 10k/100k 链路均断言库总数；100k 截图确认显示 Library 100,000、当前候选 200。生产主界面没有增加额外计数请求。

复现入口（目录必须全新，源须是已关闭且成功的受控 fixture）：

```bash
CARGO_BUILD_JOBS=2 cargo build --release -p api-server --example semantic_http_benchmark
python scripts/benchmark_bib_semantic_service.py \
  --worker target/release/examples/semantic_http_benchmark \
  --directory /path/on/disk/new-http-run \
  --reuse-from /path/to/completed-synthetic-run \
  --counts 10000 100000 --evict-file-cache

BIB_GUI_CAPACITY_DRIVER="$PWD/apps/gui/tests/bibliography-capacity.mjs" \
python scripts/benchmark_bib_semantic_service.py \
  --worker target/release/examples/semantic_http_benchmark \
  --directory /path/on/disk/new-browser-run \
  --reuse-from /path/to/completed-synthetic-run --counts 10000 100000
```

另完成 [隔离 GUI 构建](literature-search-p4-isolated-gui-linux.json)：从 HEAD 导出前端，仅覆盖 P4 搜索文件与两条测试脚本配置，排除其他正在修改的 Agent、主入口、PDF 和 workspace 文件。类型检查及 Vite 生产构建通过，复用已安装依赖；保留既有大于 500kB 的单包提示。仅验证构建，不将开发环境容量结果改称生产构建交互结果；未覆盖用户正在修改的 gateway 生成资源。

## 排名不变性与人工判断工具

[排名回归](literature-search-p4-ranking-recheck-linux.json)使用当前 release 词法代码重跑原 5,183 篇、300 条 SciFact test，全部 Top-100 文献顺序与原词法基线相同，nDCG@10 仍为 0.56873。它仅检查内部收集/借用优化没有改变排名，没有重新选择模型、参数或借此新增一轮独立质量验收；原冻结 dense/RRF 实验保持不变。

新增 [人工判断工作流](../literature-search-judgment-workflow_zh.md)与[工具资格](literature-search-p4-judgment-workflow-linux.json)：从本地真实论文与查询建立盲评候选池，冻结任务分组、划分和输入 hash，保留空白标签；人工填写后检查完整性、独立身份与显式分歧裁决，再导出 BEIR 判断。6 项格式/一致性测试通过，测试数据明确为虚构夹具。尚未提供真实中文任务及人工等级，不生成中文质量分数，也不把工作流测试当成中文验收。已有可信人工 qrels 可直接使用现有评价器，无需重做这套新建标注流程。

## 推理参数与兼容性补充

[参数资格记录](literature-search-p4-inference-options-linux.json)补齐设计中尚缺的 `inference.num_ctx` / `inference.num_batch`。生产配置持久保存这两个可选数值，连接器把它们传给固定 digest provider；provider 将显式值写入 `/api/embed` options 和缓存签名。旧配置缺省、空对象及 null 子字段不发送覆盖项，保持原签名；默认 Qwen 的冻结实验未改参数。GUI 只在高级模型设置中显示，验证了保存、重开与清空。文献输入批大小 `batch_size` 与模型 token 批大小分开，未知选项与越界参数拒绝。

本轮 36 项 embedding 库测试通过；扩大至 bib-base/api-server 全部默认库及集成目标后，333 项通过、3 项既有提取器用例忽略。333 是更广的测试范围，不与早期 252 项重复累加。真实 HTTP 集成检查建库/查询均收到显式 options；固定版本 Qwen 与已安装 BGE-M3 都通过 `8192/8192` 参数的中英短输入，分别产生两个 1024 维向量。这些短文本结果不是 BGE 长文、相关性或容量验收。

浏览器/模拟桥接及更新后的隔离 GUI 生产构建通过。相关库 `--lib` 严格 Clippy 与 embedding `--tests` 严格 Clippy 通过；扩大到所有相关库 `--tests` 的严格 Clippy 被既有 `stored_files.rs` 测试模块位置告警阻断，该项未算作通过。没有改动该文件，也没有屏蔽告警。新选项的原生 IPC、实际长输入与 Rust 隔离构建仍待验证；前文容量收据保留其当时 source hash，不改称新可选分支已重新测量。留空使用服务默认，复现实验应冻结服务配置和版本，不能以模型 digest 推定服务器默认永远不变。

## 磁盘启动优化与长输入后续验证

以上各节保留当时的验证范围。[启动优化收据](literature-search-p4-startup-linux.json)记录后续改动和同源副本对照：原加载按 chunk 哈希逐条连接向量表，查询计划确认存在逐项索引查找。先做同版本存储诊断，100k 所需向量共 409,600,000 字节，三种方式的内容指纹相同；建议清理各自副本文件缓存后，原连接读取约 13.29 秒、全表扫描约 0.79 秒、按 rowid 分批读取约 2.33 秒。这是投影读取诊断，排除完整快照构建、归一化、元数据、模型及传输，不能当作搜索延迟。

生产加载采用按 rowid 分批读取：在同一读事务内解析当前绑定需要的向量、取得当前 signature 的临时行号，排序后每批最多 256 个，再按原 chunk 顺序组装快照。行号仅是事务内访问位置，不是持久身份，也不承诺数据文件的物理排列；不扫描其他模型的向量 BLOB。保留所需向量的 checksum/维度/归一化、缺失向量、证据偏移及绑定数量校验。新增回归覆盖跨批次 302 块、重复输入共享 Arc、其他模型及未引用损坏缓存隔离，所需损坏向量仍须失败。

| 100k ext4 同源副本 | 原加载 | 分批读取后 |
| --- | ---: | ---: |
| 配置至索引就绪，单次 | 16,144ms | 5,938ms |
| 进程至首结果，单次 | 20,538ms | 10,597ms |
| 首次查询，单次 | 3,794ms | 3,904ms |
| 宽查询热 P95，30 次 | 414ms | 415ms |
| 四客户端 P95，20 次 | 1,558ms | 1,529ms |
| 进程峰值 RSS | 790,992KiB | 823,056KiB |

优化主要改善加载，峰值 RSS 增加约 31.3MiB；不宣称首次查询或热查询提速。10k 本轮就绪单次约 468ms、进程至首结果约 908ms。仍仅为共享主机、明确 ext4 文件缓存建议操作和受控模型的观测，不能推断真实模型、生产 GUI 或所有缓存均冷时的表现。

复现存储诊断：

```bash
CARGO_BUILD_JOBS=2 cargo build --release -p bib-base --example semantic_storage_probe
python scripts/benchmark_bib_storage_reads.py \
  --worker target/release/examples/semantic_storage_probe \
  --source /path/to/closed-synthetic-run/100000 \
  --directory /path/on/disk/new-storage-diagnostic
```

本轮完整默认后端库及集成目标 **334 项通过、3 项忽略**。既有 Clippy 告警通过把 `structured_object_path` 原函数移到测试模块之前消除，函数内容不变；相关库和测试的 `-D warnings` 检查、两个新诊断示例的严格 Clippy 均通过。历史失败记录保留，不屏蔽规则，也不把 334 与前轮结果重复累加。

[长输入与原生收据](literature-search-p4-long-input-linux.json)使用当前读取器重新验证已安装 BGE-M3：从同一公开语料按长度和 ID 选出 4 篇 6,846–10,000 字符的摘要记录，不读取 qrels 选择输入。`128/128` 的小 context/token batch 返回 HTTP 400，没有可查询 generation，词法仍可用；改为 `8192/8192` 后 4 块就绪。token batch 改为 4096 后新 signature 重新计算 4 块；恢复原参数重用原 generation，新增计算为 0。它们是提供的摘要文本，不证明上游原文未被数据集截断。

随后按 1000 字符、100 重叠分为 38 块，返回的语义证据是规范文本的实际子串；元数据修改复用 11 块且不新增计算。仅为验证失效追加明确标记的测试后缀，复用 11 块、计算 1 块。删除后余 28 块、3 篇候选，重开保留参数与 generation；旧游标拒绝。运行时的词法采样只说明请求能完成，不作为容量 P95。

真实 Linux Tauri/WebKitGTK 同时新增验证高级参数经 IPC 保存与页面重载保留 `8192/8192`，并重新通过真实 Qwen 的第二附件证据定位、保存规则、重建/修复/暂停/恢复和停用。该原生功能挂载使用当前工作区阅读器，不将其描述为排除所有其他修改的原生阶段构建。短 TXT、4 篇长摘要及少量分块不能代替多附件规模、持续写入、长时间批次、缓存配额、PDF 坐标或中文质量验收；P4 仍未提交，P5 尚未开始。

## 资源预算、修复续算与设置页

[资源资格记录](literature-search-p4-resource-limits-linux.json)补充持久 `limits`：默认最多 1,000,000 块、每输入 256KiB、每物理向量库 2GiB 向量载荷、该文献库全部语义缓存文件 8GiB。完整配置与边界见 [API 说明](../api-server_zh.md#p4-语义索引配置与后台服务工作区实现)。预算不进入 embedding/profile signature，不能通过改变预算混用不同模型或分块的向量。

载荷由事务触发器统计，旧缓存一次性迁移计数；重复插入、删除、修复和回滚保持一致。模型计算前预检，写事务提交前再次检查，验证了两个 profile 竞争同一额度时后到者回滚。UTF-8 输入包含标题/换行，查询包含 instruction；块数与输入超限不静默截断，旧已发布代及规范文献保留。普通资源失败只在源/配置或相关空间改善后重试，避免多个 worker 因彼此的 WAL 增长反复规划。

载荷预算与按文件大小检查的磁盘预算分别解释：后者在批次边界及新修复库创建前协作检查，临时/并发写入可超过额度，不是 OS 硬配额，也不限制 RAM/GPU。不会自动逐出已发布模型或压缩文件。仅修改预算时，配置和未完成维护任务在同一事务重绑定，旧所有者不能继续激活；保留原 request_id、私有缓存及明确暂停状态。模型或其他连接/预处理配置变化继续使用原替换规则。

实际固定版本 Qwen 对前 20 篇公开 SciFact 记录完成块数、载荷、磁盘预算失败与恢复，期间文献新增和词法检索可用。第 20 篇在提高预算后只新增计算 1 个向量，20 块载荷为 81,920 字节。另将修复预算设为 16 块：提交 16 个真实向量后失败，仅提高预算便在同一私有库复用 16 块、补算 4 块并激活；原 request_id 保留。磁盘预算恢复及重开复用完成的修复代，未产生相关性分数。

GUI 新增高级预算输入和最近就绪时的缓存占用。检查长设置页发现标题/保存操作会滚出视野，现将它们固定，正文独立滚动；浏览器和模拟桥接在编辑最后一项时检查标题与保存按钮仍位于可见对话框内，截图已人工查看。共享高级查询、规则、阅读返回等回归通过。实际 Linux Tauri/WebKitGTK 验证预算经 IPC 保存/重载、占用返回、暂停期间只改预算仍保持暂停、之后显式恢复，以及当前固定标题/操作区。隔离 GUI 类型检查与生产构建通过，排除其他 Agent、主入口和 PDF 修改；原生功能测试仍使用当前工作区阅读器，不冒充隔离原生发布构建。

本轮 `cargo test -p bib-base -p api-server --lib --tests -- --include-ignored` 共 **349 项通过、0 失败、0 忽略**，包括原来默认忽略、现已具备依赖的 3 项 Zotero 提取/迁移用例；默认 ignore 标注没有删除。相关库、测试和资源探针的严格 Clippy 通过。资源新增覆盖还包括旧计数迁移、事务回滚、Unicode 字节边界、降低块数后减少源数据恢复、两个失败 worker 保持稳定、修复前拒绝新增空库，以及预算重绑定后的所有者隔离与 16+4 续算。

当前核心代码的 10k/100k ext4 同源副本再次执行文件缓存建议清理和真实鉴权 HTTP：

| 库规模 | 配置至就绪，单次 | 进程至首结果，单次 | 宽查询热 P95 | 四客户端 P95 | 峰值 RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| 10k | 386ms | 889ms | 44.3ms | 169ms | 231,336KiB |
| 100k | 5,131ms | 9,488ms | 420ms | 1,557ms | 853,452KiB |

每库 84 个 fixture 输入仍全为查询，没有重算文献向量；载荷计数从旧派生库迁移。模型是受控确定性 fixture，文件建议操作不保证物理缓存全冷；首次观测不是分位数，不能将不同单次运行的波动归因为预算改善。100k 热 HTTP 仍未普遍满足 400ms 目标。多附件扩张、持续写入、真实长时间批次、中文真实质量、其他平台及阶段隔离构建继续验收；P4 尚未提交。

## 多附件链路、删除失效与隔离原生验证

[多附件资格记录](literature-search-p4-attachments-linux.json)使用真实鉴权 HTTP 上传、后台 TXT 提取、独立 ext4 VFS、生产存储 actor 和查询接口。每篇两个附件，每个附件三个不同的 512 字符窗口，加上一块摘要，共七块；每个窗口包含文献、附件和窗口编号，避免重复文本使向量缓存占用失真。向量仍来自便宜的确定性 1024 维 HTTP fixture，不代表实际推理速度或语义质量。

100 篇试跑发现删除非首选附件没有推进检索 outbox，数据库已删而旧正文仍在索引中。现将附件删除和 revision/token 更新放进同一立即事务。新增回归覆盖旧正文、旧证据和游标失效，首选附件保留，以及重复删除不推进版本。修复后的 100 篇与 10,000 篇全链路通过；后端 **350 项通过、零失败、零忽略**，相关库、测试和基准示例的严格 Clippy 通过。

| 10,000 篇 / 20,000 附件 / 70,000 块 | 观测结果 |
| --- | ---: |
| 真实上传及提取 | 426.5 秒 |
| 首次计算全部向量并就绪 | 65.6 秒 |
| 宽查询热 P95，30 次 | 137.9ms |
| 1% 硬条件热 P95，30 次 | 41.9ms |
| 四客户端 P95，20 次 | 561.7ms |
| 峰值 RSS | 811,776KiB，约 793MiB |
| 原文件与提取文件 | 40,000 个，逻辑内容 61,440,000 字节 |

这次是首次构建，包含模型 fixture 调用和向量写入，与前述已计算向量的重开计时不同。验证了非首选附件原文件字节与正文证据、删除后三块及旧词消失、新附件增加三块且新词可检索，以及旧游标明确返回过期错误。40 次、间隔 100ms 的标题编辑属于短时写入突发；此间 132 次混合查询全部明确返回 409，词法仍可用，停止写入后能追平最新版本。它不证明持续写入期间的语义查询可用性，也不能代替长时间批次验收。

100,000 篇、200,000 附件、700,000 块的独立测试正在运行，尚无结果。700,000 个不同的 1024 维向量需要 2,867,200,000 字节载荷，因此显式将向量预算配置为 4GiB、缓存文件预算为 16GiB，保留默认一百万块上限；不以默认额度不足为由缩减规模。

另从 HEAD `222a37d` 生成独立源码快照，叠加阶段文件，排除其他 Agent、主入口和 PDF 阅读器修改；固定为基线阅读器并在运行后核对源码哈希。该快照实际构建 Rust 原生示例，并通过 Linux Tauri/WebKitGTK、真实 Qwen 的设置保存、预算/维护暂停保留、主题与硬条件、非首选附件证据、二进制 IPC、阅读定位、规则与重载验证。它是隔离的原生功能测试，不是完整签名桌面发行版、跨平台、人工屏幕阅读器或规模测试。

## 中文论文判断来源与冻结流程

已取得 [NeuCLIRTech 官方材料](https://huggingface.co/datasets/neuclir/tech)，使用其配套 [中文 CSL 文献](https://huggingface.co/datasets/neuclir/csl)和人工判断；方法来源见 [作者论文](https://arxiv.org/abs/2602.05334)与 [NIST 技术文献任务](https://trec.nist.gov/data/neuclir2024.html)。固定版本实际文件含 395,927 篇去重后的中文文献、110 条查询、35,962 条判断。文献声明 Apache 2.0，查询/判断声明 CC BY 4.0，固定版本与文件校验值见 [来源审查](literature-search-p4-neuclir-source-linux.json)。原文与判断只存于忽略的验证目录。

`prepare_bib_neuclir.py` 校验固定原文件，逐字保留中文标题、摘要与官方查询第二列，转换原始 0/1/3 gains，不生成判断、不翻译、不按相关性缩小语料。全部 110 条作为验收查询，没有伪造训练划分；继续使用英文选择的 Qwen、完整摘要、深度 100、RRF k=10 和 dense weight=4，见 [中文冻结记录](literature-search-p4-neuclir-frozen.json)。评价器增加官方要求的 nDCG@20 与对应配对比较，既有 nDCG@10 路径保留。转换、对象引用、缺失正例拒绝和指标边界等共 **19 项 Python 测试通过**。

全量中文词法基线现已完成，实际固定模型混合检索正在计算向量。结果与范围见下一节；尚未证明混合收益。后续仍需报告不完整判断池、配对区间和预训练污染限制。P4 尚未提交，P5 尚未开始。

## 中文主题连接修正与首次全量基线

[主题策略资格记录](literature-search-p4-topic-policy-linux.json)记录一次在官方中文打分前发现的语义问题：无空格中文句子先被当作单个 Term，内部要求全部词典词命中，使主题 OR 没有覆盖其实际分词。现将连续中文片段按索引使用的固定 Jieba 词典分词，再连接 OR；混合文本里的 `IL-6` 等非中文片段保留原样。硬查询解析未改变，权重、模型、深度和完整摘要政策不变。词法基线与混合执行共享 `topic_search_expression`，不再分别实现主题策略。

构造用例证明中文两词可分别召回、显式 NOT 仍生效、科研符号保持相邻；英文 SciFact 的 300 条官方查询与旧策略生成完全相同的 AST，未读取 qrels，也不冒充一次新的英文质量打分。保留 [首次冻结版本](literature-search-p4-neuclir-frozen-v1.json)，[当前冻结版本](literature-search-p4-neuclir-frozen.json)记录修正理由及时间；修正发生在任何官方中文检索指标产生之前，没有依据中文成绩调整参数。

353 项后端测试、相关库/测试/新示例的严格 Clippy 和最新独立源码快照的实际 Linux Tauri/Qwen 功能测试通过。默认 `/tmp` 启用了用户配额，期间曾有 24 项 API 用例连空库创建都因 `QuotaExceeded` 失败；保留失败日志，切换到本任务 ext4 临时目录后全量通过。另将三个确认无打开句柄的旧任务样本复制到项目磁盘，逐文件校验后保留原路径符号链接；历史测量仍解释为原来的 tmpfs，不把归档副本当作新磁盘测量。

`semantic_relevance` 在生产 actor/service 中分两阶段运行，不读取判断文件。导入每 500 篇记录检查点，已完成输出不能覆盖；数据、模型、词法策略和工具身份绑定。先以十篇构造文献、两条构造查询和真实固定 Qwen 验证两个阶段，无虚构相关性分数，随后使用全量 395,927 篇原生中文文献与全部 110 条官方查询。

| 首次全量词法基线 | 结果 |
| --- | ---: |
| nDCG@20，主指标 | 0.300555 |
| nDCG@10 | 0.302288 |
| MRR@10 | 0.601551 |
| Recall@50 / @100 | 0.323912 / 0.394555 |
| 无返回结果的查询 | 0 / 110 |
| 110 条不同查询各执行一次的 P95 | 1,898.9ms |
| 峰值 RSS | 1,206,480KiB |

上述时延为生产 actor 直调、不同主题各一次的分布，未计 HTTP/GUI，也不是同一主题的重复热 P95。基线不是 Lucene/BEIR 标准 BM25 等价实现，相关性仅相对于不完整的公开判断池；未判断文献按零分处理。全量中文查询性能仍需优化。相同语料、参数和实际固定 Qwen 的混合向量正在持久计算，尚无混合分数、提升比例或配对区间。

100k 附件测试的原尝试已因 HTTP 500 退出，旧工具丢失了响应正文，原因为何尚未证明。原始目录、可执行文件、源码和日志已保留。独立副本恢复时发现两个文件元数据未写完的附件；恢复程序验证已有原文件及提取字节，对未完成项通过真实 API 删除/重新上传并计数，不把缺文件当作完成。新工具保留错误正文、阶段检查点和失败/运行收据。恢复导入仍在运行，不能报告 100k/700k 块完成；测试临时目录的配额错误不作为原上传 500 原因的证明。
