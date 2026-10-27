# P1 本地检索规模与恢复验收

使用生产 `BibBase` actor、只读数据库投影视图、事务 outbox、后台索引器和 V2 查询接口。所有数据为合成数据，程序不访问外部文献 API；结果不替代人工相关性判断。

```sh
cargo run --release -p bib-base --example search_benchmark -- --count 10000 --query-repeats 20 --output docs/benchmarks/literature-search-p1-10k-release.json
cargo run --release -p bib-base --example search_benchmark -- --count 100000 --query-repeats 20 --output docs/benchmarks/literature-search-p1-100k-release.json
```

默认每篇摘要 1000 字符，每十篇增加一份 10000 字符全文；每 50 篇包含中文免疫词/短语，每 100 篇包含 IL-6 或 IL-10/chr6 排除样例。11 类查询各采样 20 次，另做 100 个精确 DOI Top-1 检查、附件与中英文命中计数检查。`--abstract-chars`、`--fulltext-every`、`--fulltext-chars` 可调整。

首次导入要求 `--directory` 下没有 library.db；数据库与索引保留供复查。对既有合成数据追加 `--reuse true` 跳过导入（报告导入时间为 null），重新验证、追平、显式重建及采样。最终代码的复用报告为 `*-final-release.json`；`*-release.json` 保留新建混合语料时的完整导入收据。`baseline-v1/` 是早期 ASCII 原型收据，不用于当前性能结论。

2026-10-27 参考环境：Linux x86_64、Intel Core i7-13700KF，可见 CPU 0–23。容器未暴露 cpu.max/memory.max，不能推断独占整机。内存来自 `/proc/self/status`，磁盘采样包含运行中数据库/WAL；其他平台内存项可能为空。

最终 release 代码采样：

| 文献 / 全文 | 全量重建 | 精确 DOI P95 | 标题短语 P95 | 标题 + 年份 P95 | NOT 查询 P95 | 峰值 RSS |
| --- | --- | --- | --- | --- | --- | --- |
| 10k / 1k | 259.2 ms | 0.70 ms | 5.92 ms | 12.07 ms | 12.02 ms | 216.8 MiB |
| 100k / 10k | 2571.9 ms | 1.09 ms | 35.89 ms | 157.48 ms | 150.97 ms | 627.6 MiB |

以上是各类别热查询 P95，不能外推真实查询总体分布、HTTP 或 GUI 时延。两种规模的 100 个精确 ID Top-1 检查均正确；最终状态无积压、无错误。

## 重开、维护和升级

```sh
cargo build --release -p bib-base --example search_concurrency_benchmark
target/release/examples/search_concurrency_benchmark DATABASE OUTPUT.json
```

程序重开数据库并执行首次搜索，随后异步全量重建，同时执行 30 次元数据更新、状态轮询及 20 次标题短语查询；检查确实观察到 building，最终追平写入。搜索与重建共用检索队列，首个维护查询会等待；元数据与状态由独立存储 actor 处理。

| 文献 | 重开 | 重开后首查 | 维护首查/最大等待 | 写入 P95 | 状态读取 P95 |
| --- | --- | --- | --- | --- | --- |
| 10k | 21.2 ms | 6.6 ms | 317.5 ms | 4.45 ms | 1.83 ms |
| 100k | 202.7 ms | 35.9 ms | 2652.1 ms | 3.90 ms | 0.14 ms |

原始收据为 `*-maintenance-release.json`。旧版 materialized projection 的 100k 库升级收据为 `*-100k-upgrade-release.json`：open 478.2ms，升级后首查 2613.9ms（包含索引追平）。进程重开样本的操作系统缓存为 warm/uncontrolled，并非 OS 冷缓存测试。维护搜索 20 次的 P95 不能掩盖首查最大等待，必须分别报告。

## 实际进程恢复

```sh
cargo build --release -p bib-base --example search_recovery_probe
python3 scripts/qualify_bib_search_recovery.py --probe target/release/examples/search_recovery_probe --output docs/benchmarks/literature-search-p1-process-recovery-linux.json
```

Linux 收据记录两次真实 SIGKILL：源事务提交后、索引尚未提交时分别中断；恢复后检查新/旧词、删除及证据身份。还验证持久 writer 与另一进程 RAM 索引同时运行时的更新同步，及关闭数据库后复制备份、不携带派生索引的离线恢复。结果 passed。测试进程/临时库隔离，不修改用户库。

`.github/workflows/bib-search.yml` 已配置 Linux/macOS/Windows 的本地语义、恢复和 API 资格测试；macOS/Windows 尚未取得实际 CI 运行结果，仍为生产发布门槛。真实语料相关性、OS 冷缓存和组织级并发也不能由本机合成样本证明。
