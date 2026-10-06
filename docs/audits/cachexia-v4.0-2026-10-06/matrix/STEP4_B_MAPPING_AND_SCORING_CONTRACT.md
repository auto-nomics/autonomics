# 步骤 4：B 的成员映射与评分合同（2026-10-06）

## 4.1 四负对照映射闭合 —— 76/76 逐行结论已出

对象（v4.0 方案 L764 首批固定校准面板，非"已发现新靶点"）：
NP06 PanInflam_HallmarkIR（200 成员/5 缺）、NP07 N5P 全签名（537/63）、
NP08 CiM_UP 原始签名（47/5）、NP09 GMP_origin 来源定义（17/3）。
逐行结论表：`B_mapping_closure_NP0609_v1.tsv`（76 行，含 NCBI GeneID /
Ensembl mouse ID / 人类同源列表 / 结论列）。

方法与证据链（三源，如实分层）：
1. 冻结映射基线：`p2_gse130563_ortholog_map.tsv`（tier 词含
   `alliance_reciprocal_best`）——上一批即在此源下判 ID_MAPPING_UNAVAILABLE。
2. **NCBI Datasets symbol 解析**（现行权威，74 唯一符号）：64 解析成功
   （并规范化 Chil3→Chi3l1）；10 解析失败。
3. **Ensembl Compara 人鼠正交对查询**（按 Ensembl mouse ID）：64 个可解析
   符号 **0 个返回人-鼠 orthologue**——与前源结论一致，双现行权威源互证。

结论分类（76 行）：
- `NO_ORTHOLOG_CURATED_AND_COMPARA` × 56：curated 映射源与 Ensembl Compara
  双源均无人鼠正交对（多为先天免疫快速演化族：Ngp/Chil 族/Trem 簇/Itgb2l
  等）。**映射声明就此闭合：无人类同源是结论，不是缺口。**
- `MAPPING_CONFIRMED_NOT_IN_MATRIX` × 10：人类 ortholog 映射成立，供体矩阵
  （23,786 列）无该列——矩阵覆盖事实，维持 NOT_ESTIMABLE 且不再挂"待映射"。
- `SYMBOL_UNRESOLVED_SIGNATURE_DEFECT` × 10：签名定义表本身缺陷——
  `BC100530/BC117090/AC121583.1/CT010467.1` 为 cDNA 克隆号、
  `Lilrb4a.1` 亚等位写法、`CXCL8` 为人符号混入小鼠列表、Gm 预测基因
  NCBI 亦不解析。**这 10 个是签名源文档质量问题，登记给方案侧处置；技术侧
  不得为其编造映射。**

边界（与验收口径一致）：
- 本轮只关闭"映射声明完整性"；**696 可估计基因名单、模块成员集合、
  NOT_ESTIMABLE_LEDGER 的封口字节全部不动**。若方案侧日后决定把
  `MAPPING_CONFIRMED_NOT_IN_MATRIX` 或替代成员纳入家族，那是科研决定，
  需重开分母评审，不由本技术结论触发。
- `NO_ORTHOLOG_*` 56 项按方案规则属"四重特异性对照"的组成缺员——负对照
  解释力受影响一事，已作为科研注记移交方案文档，不改变技术验收状态。

## 4.2 评分合同（不混用，矩阵行 5/6/11 的约束文本）

| 评分/模型 | 使用对象 | 实现 | 禁止事项 |
|---|---|---|---|
| z-mean 模块评分（每基因跨 46 供者 z，样本 SD/ddof=1；模块=在场基因均值） | NP06-09 + 旧共享四模块的**模块层检验**（MW 全枚举、Spearman） | 引擎 `sql` 节点冻结合同（ab-run v3 三源一致 184 格） | 不得称 UCell、不得称 singscore；不得与下两行互换引用 |
| 基因层一元 OLS（expression ~ condition，1/0 编码，无新增协变量） | 696 唯一基因**新探索性成员** | `linear_regression`（对 R `stats::lm` 已验收 ≤8.43e-15） | 不生成对 696 的事后 BH 家族；term 名 x1 不自带变量名；无自带 CI |
| UCell / singscore | **本轮不在链**（未装载） | 若要引入：单细胞插件路线 + 独立官方参考验收 + 各自矩阵行 | 引擎原生值不得称"UCell 分"（量纲差异有既往记录）；不得与 z-mean 结果同列比较或择优 |
| conventional IVW 与 MRlap 校正效应 | A 链 | twosamplemr(0.7.9) / mrlap_official(0.7.11 环境) | 两列效应非同一尺度与 IV 集合，不得直接相减或择优（seed1 报告口径） |

合同生效方式（部署面）：DAG 模板层引用哪个 kind 即落入哪行——本次 A/B/
模块三图分别只含单一评分实现（`NODE_INVENTORY.tsv` 逐实例记录可查），
不存在同图混跑两种评分的路径；后续新增图须按本表选行并在矩阵登记。

## 4.3 遗留登记

- 10 个签名定义缺陷行的处置权在方案/科研侧（可寻址替换、删除或注记，技术不代答）。
- 三源复核的原始数据：/tmp/ortholog_stage2.json（NCBI+Ensembl 逐符号）、
  closure 表已入库；NCBI 下载件（gene_orthologs.gz/hs_gene_info）中
  gene_orthologs.gz 实测为**新 ID 空间增量快照**（经典 GeneID 零交集，
  不可作 symbol 复核源）——留此一句防止后人再踩。
