# 文献检索人工判断工作流

此工具准备候选池并校验人工填写的结果，不生成真实查询、相关性等级或中文质量分数。尚未收到中文人工数据时，可以先完成工具与格式验证，不能据此宣布中文相关性验收完成。

已有可验证来源的正式人工判断可直接使用既有评价格式，不要求为其重做双人标注。以下是新建标注包的一种工作流，不把双人流程追加为所有现有数据集的验收条件。

`scripts/prepare_bib_judgments.py` 仅使用本地文件。先冻结真实文献与查询，再合并词法、向量和混合候选，向评审者隐藏模型名与原排名；两位评审者独立填写，分歧交由第三位评审者显式裁决。最终转换为既有评价程序接受的 BEIR 格式。

## 准备真实输入

- `corpus.jsonl`：每行包含字符串 `_id`、真实 `title`、`text`，可附真实 `url`。不猜测 DOI、链接或缺失正文。
- `queries.jsonl`：每行包含字符串 `_id` 与真实任务 `text`。可附 `group_id` 和预先指定的 `split: "train" | "test"`。同一任务的改写或近似重复必须使用同一 group；相同文本默认自动归组。
- 未指定 split 时，按冻结 seed 与 group 确定性分半，需同时存在 train/test。不能依据相关性等级或评价结果重新划分。这个分组不会自动识别所有语义重复，仍须人工审查。
- 检索 run 使用既有 JSONL：`query_id`、按 score 降序的 `hits[{article_id,score}]`。每个查询必须有一行，无结果也应保留空 hits；run 不含人工标签。

原文语言、查询语言、领域、任务类型、来源与许可应在研究记录中明确区分。中文查询检索英文论文、中文原生论文、翻译查询和问答检索分别报告，不互相替代。

下面的路径仅说明命令格式，不附带伪造的研究任务或判断：

```bash
python scripts/prepare_bib_judgments.py pool \
  --corpus /path/to/real-data/corpus.jsonl \
  --queries /path/to/real-data/queries.jsonl \
  --run lexical=/path/to/runs/lexical.jsonl \
  --run semantic=/path/to/runs/semantic.jsonl \
  --run hybrid=/path/to/runs/hybrid.jsonl \
  --top-k 50 --seed zh-paper-pilot-v1 \
  --directory /path/to/new-review-bundle
```

每个模型取 Top-K 后按文献去重，行序按独立 hash 打乱。没有任何候选的查询会报错，应扩展召回或加入研究者提出的候选 run；新增候选不自动视为正例。候选池不能证明完整召回，最终 Recall@50 只能相对于已判断池解释。

## 独立评审与裁决

仅把 `review/` 发给评审者，保留 `admin/` 和根 manifest 给实验管理员。工具不自动上传或发送文件。`admin/` 保存原文、冻结 split、候选来源和排名；评审目录不包含这些排名。`pool_id` 将返回表格与冻结配置关联，避免混用另一轮评审。

两位评审者各复制 `review/tasks.csv`，只填写最后三个字段 `grade`、`note`、`assessor_id`，不改 ID 或文本列。导入电子表格时把 ID 列作为文本，避免前导零被改写。

- `0`：不满足该任务的信息需要。
- `1`：有用但间接的相关信息。
- `2`：直接相关的研究或证据。
- `3`：对任务高度关键的直接研究或证据。

等级衡量检索相关性，不代表结论正确、证据强度或医疗建议。正等级必须注明判断依据及原文位置。`preview_only=yes` 表示仅展示前 16,000 字符，需要结合原文评审；不能把未读到当成不相关。空白不是 0，不能自动补齐。每份评审表须完整、且只使用一个评审者标识；两人不能共用标识。

```bash
python scripts/prepare_bib_judgments.py export \
  --pool /path/to/new-review-bundle \
  --assessment /path/to/assessor-a.csv \
  --assessment /path/to/assessor-b.csv \
  --declaration '请填写真实的评审来源、过程与材料范围' \
  --directory /path/to/new-judged-dataset
```

有分歧时程序拒绝导出。第三位评审者提供 `--adjudication /path/to/adjudication.csv`，列为 `query_id,doc_id,grade,note,assessor_id`，必须恰好覆盖所有分歧，并说明依据；不能把多数票或模型分数自动变成裁决。

所有候选均判为 0 的查询不会被悄悄删除。应核查候选池是否遗漏相关论文，或将真实无答案任务另行记录并设计相应指标；当前 nDCG/Recall 工作流要求每个纳入查询有已判断正例。

## 导出与验收边界

成功导出包含 corpus、原查询、按冻结 split 分出的检索输入、`qrels/train.tsv`、`qrels/test.tsv` 和 manifest，可直接使用 `evaluate_bib_relevance.py`。manifest 记录两份评审文件、裁决文件、池配置及输出文件的 hash，并报告一致率。

`ready_for_relevance_evaluation` 只表示格式与声明来源完整，不表示样本数足够、已达到质量门槛、身份已认证或中文发布已获验收。至少仍需记录：真实任务数量与分层、来源与许可、调参/独立验收隔离、模型及融合协议、退化案例、置信区间和未判断候选的限制。评审身份按提交者声明记录，工具不能独立证明文件由人填写。

工作流测试使用标明“测试夹具”的虚构内容，仅验证缺失标签、独立身份、分歧、篡改、泄漏和格式转换，不计入真实相关性证据：

```bash
python -W error::ResourceWarning -m unittest discover -s scripts -p test_prepare_bib_judgments.py
```
