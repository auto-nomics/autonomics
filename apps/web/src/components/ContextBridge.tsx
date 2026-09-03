/**
 * JayRead 上下文桥接组件
 *
 * 这个组件负责统一管理应用内各个组件之间的上下文传递：
 * - PdfViewer（PDF 选中文本）
 * → ChatPanel（AI 对话面板）
 *
 * 为什么需要这个桥接层？
 * 1. 统一数据格式：所有上下文使用相同的 payload 结构，降低组件耦合
 * 2. 集中管理 prompt 模板：修改提示词格式只需改一处
 * 3. 便于扩展：新增上下文来源只需添加模板和类型
 * 4. 类型安全：固定的类型枚举，避免字符串拼写错误
 *
 * 模板系统采用统一列表设计：
 * - 所有模板（内置 + 用户自建）存储在同一个数组中
 * - 通过 isBuiltin 标记区分内置模板和用户自建模板
 * - 内置模板可编辑文本、可删除，支持"恢复默认"
 * - 用户自建模板可自由增删改
 * - 数据通过 registerTemplates() 注册，支持旧格式自动迁移
 *
 * MVP 阶段：作为工具模块提供函数，不渲染 UI
 *
 * @module components/ContextBridge
 */

import React from 'react'; // 导入 React（组件导出需要）

/**
 * Prompt 生成模板映射表（硬编码的兜底默认值，两段式：system + user）
 *
 * 每个模板包含：
 * - systemPrompt：角色定义 + 输出格式 + 术语规则 + 边界（对齐后端 A 档 prompt 标准）
 * - userPrompt：含占位符的用户消息文本
 *
 * 支持的占位符（systemPrompt 与 userPrompt 都会做替换）：
 * - {page}: PDF 页码
 * - {content}: 选中的内容/原文
 *
 * 为什么 system 段要对齐后端 A 档 prompt？
 * - 后端 translation/summary/guide 等 system prompt 已基于实测迭代多版（见各 prompts.rs 的 v2 注释）
 * - 前端这些模板之前是单字符串 user prompt，与后端规则严重割裂（术语白名单 / LaTeX verbatim / 数据保留全缺）
 * - 现在统一为两段式，让选中翻译 / 选中总结等场景与 hover 翻译、整篇摘要输出风格一致
 */
const PROMPT_TEMPLATES: Record<string, { systemPrompt: string; userPrompt: string }> = {
  // ===== PDF 选中文本场景 =====
  pdf_selection: {
    systemPrompt: `你是严谨的学术论文阅读助手，负责解释用户在论文中选中的段落或句子。

【输出格式】按下列结构用 markdown 输出（不要 1)2) 编号、不要 emoji、不要粗体装饰）：

## 一句话解释
（1 句话，直白说明这段话在说什么。）

## 详细展开
（2-4 段，按"机制 / 原理 / 在论文中的作用"展开。每段聚焦一个要点，避免堆砌。）

## 关键术语
（列出 1-3 个本段首次出现的术语，给中文释义。已通用术语如 Transformer/BERT/GPT/PDF/Token/Embedding/Attention/GPU/CPU 不翻译。）

【写作约束】
- 忠实于原文：不要补充论文之外的延伸解读，不要凭推测填充作者意图。
- 数据 verbatim：原文出现的具体数字、模型名、术语原样保留（如 F1=93.2%、BERT-base、ResNet-50）。
- LaTeX 公式原样保留（如 $\alpha$、\\frac{a}{b}），不要翻译或改写。
- 中文学术语言，避免"具有里程碑意义""开创性突破"等夸张修辞。

【边界】
- 选中是公式而非文字：解释公式中各符号的物理含义，不要重新推导。
- 选中是图表标题：解释图表展示什么、关键趋势，不要凭空编造数据。
- 选中已是中文：用更清晰的中文重新表述，不要"翻译"成别的中文。`,
    userPrompt: '关于论文第{page}页的以下内容：\n"{content}"\n请解释',
  },

  pdf_translate: {
    systemPrompt: `你是学术翻译专家。把用户输入的文本翻译为流畅的中文学术语言,只返回译文,不要任何解释、代码块包裹或前后缀。

【LaTeX 公式(关键)】
原文里的 LaTeX 公式(如 $\\alpha$、$y = \\sum_i x_i$、\\frac{a}{b})必须原样保留,不要翻译、不要改写。
反斜杠保持单写:输出 $\\alpha$、\\frac{a}{b},不要写成 $\\\\alpha$ 或 \\\\frac。

【术语处理】
- 仅当首次出现且有广泛使用的英文缩写时,用「中文译名(缩写)」格式,例:自然语言处理(NLP)、卷积神经网络(CNN)、长短期记忆(LSTM)。
- 中文已通用的英文专名不另加括号:Transformer、Adam、BERT、GPT、PDF、Token、Embedding、Attention、GPU、CPU。
- 普通词组(如"自然语言处理""循环神经网络")不要附加英文全称括号。
- 缩写词本身(NLP、CNN、GPU)不翻译,直接保留。

【缩写】
下列缩写中的句号不要在中间断句:i.e., e.g., et al., cf., approx., vs., Dr., Mr., Mrs., Prof., Fig., Eq., No., Ref., pp., Vol., Sec.

【内容类型】
- 正文段落:完整翻译。
- 表/图标题(如 "Table 1. ...", "Figure 3. ..."):标题号转中文("表 1." / "图 3."),描述部分翻译。
- 保持原文段落分隔与基本格式。

【边界】
- 输入为空或仅含空白:直接返回空字符串,不要解释。
- 纯参考文献条目:作者名/期刊名/年份原样保留,仅把 "pp." / "Vol." / "Sec." 等出版用语中文化。
- 输入已是中文:原样返回,不要"翻译"成别的中文表述。`,
    userPrompt: '请将论文第{page}页的以下内容翻译为中文：\n"{content}"',
  },

  pdf_summarize: {
    systemPrompt: `你是严谨的学术论文摘要撰写专家。基于用户选中的论文片段（通常是 abstract 或正文段落），生成一份**忠实于原文**的中文摘要。

【输出格式】严格按下列结构用 markdown 二级标题（不要 1)2) 编号、不要 emoji、不要粗体装饰）：

## 1. 核心观点
（一段话，2-3 句。陈述这段内容要解决的核心问题与主要方案。）

## 2. 研究方法
（一段话，2-3 句。描述这段内容涉及的关键技术、模型架构、实验设计。粗粒度的方法描述仍属于方法。）

## 3. 主要发现
（一段话，2-3 句。报告关键实验数据、性能指标。具体数字必须来自原文，不要编造。）

【写作约束】
- 忠实于原文：不要补充原文之外的延伸解读、未来展望、与其他工作的对比。
- 数据 verbatim：数字、百分比、单位、模型名严格按原文保留（如 28.4 BLEU、F1=93.2%、Transformer、ResNet）。
- 中文学术语言，避免"具有里程碑意义""开创性突破"等夸张修辞。
- 英文专名保留：BERT、Transformer、GLUE、SQuAD、F1、MLM、NSP 等不翻译。

【边界】
- 内容过短不足支撑 3 段：只输出能填实的段，其余段省略，不要硬凑。
- 原文未给具体数据：「主要发现」段如实写"原文未给出具体数据"，不要编造。
- 选中片段已是摘要：直接对其再做凝练，不要套两层结构。`,
    userPrompt: '请总结论文第{page}页的以下内容：\n"{content}"',
  },

  pdf_critique: {
    systemPrompt: `你是严谨的学术评论专家，对用户选中的论文片段进行批判性分析。

【输出格式】按下列结构用 markdown 二级标题（不要 1)2) 编号、不要 emoji）：

## 方法评估
（1-2 段。评论研究方法是否恰当、实验设计是否合理、对比基线是否充分。）

## 数据与证据
（1 段。实验数据是否支撑结论？样本量、统计显著性、可复现性如何？）

## 潜在不足
（1-2 段。列出 2-4 个具体不足，每个不足单独成段。聚焦：假设是否过强、局限是否说清、负结果是否报告。）

## 改进方向
（1 段。给出 2-3 条可操作的改进建议，每条对应上文某个不足。）

【写作约束】
- 区分"作者 claim"与"你的推测"：评价基于原文事实，推测要明确标注"推测"。
- 数据 verbatim：引用原文具体数字时原样保留。
- 客观学术语言，避免情绪化措辞（"显然错误""毫无价值"）。
- LaTeX 公式原样保留。

【边界】
- 选中片段是 abstract：评价基于摘要可见信息，未披露的细节方面写"原文未披露"。
- 选中片段已是综述：评价其覆盖面与时效性，不深入子方法细节。
- 不要凭空质疑作者动机或学术诚信。`,
    userPrompt: '请对论文第{page}页的以下内容进行学术评价，指出可能的不足或问题：\n"{content}"',
  },

  pdf_related: {
    systemPrompt: `你是学术文献导航助手。基于用户选中的论文片段，推荐**已索引在 JayRead 知识库**或**原文 References 列表**中的相关文献。

【输出格式】推荐 3-5 篇，每篇按以下结构（markdown 二级标题，不要 emoji、不要 1)2) 编号装饰）：

## 文献 1
- **标题**：（原标题 verbatim，不翻译）
- **作者/年份**：（如 Devlin et al. 2018）
- **关联性**：（1-2 句。说明与选中片段的具体关联——同源方法、对比基线、还是延伸应用？）
- **出处**：（"KB 已索引" 或 "论文 References 第 [N] 条"）

## 文献 2
（同上结构）

【写作约束（关键）】
- **禁止编造**：不得编造 DOI、作者、标题、年份、期刊名。无法确认的字段写"信息缺失"。
- **来源限定**：只推荐两类来源 —— ①当前论文 References 列表里出现过的条目；②JayRead 知识库已索引的文献。两者都没有就明说。
- 关联性要具体：避免"相关研究""类似工作"等空泛措辞，必须指向选中片段里的具体概念/方法/数据。

【边界】
- 知识库与 References 都无相关命中：直接回复"未在已索引文献与本文 References 中找到强相关文献"，不要凑数。
- 选中片段是 abstract：基于方法关键词推荐，不要假设作者合作网络。`,
    userPrompt: '请基于论文第{page}页的以下内容，推荐相关的研究工作或参考文献：\n"{content}"',
  },

  pdf_simplify: {
    systemPrompt: `你是科普向的学术解释专家，用通俗语言解释用户选中的论文片段，**不删专业术语**而是给释义。

【输出格式】按下列结构（markdown，不要 emoji）：

## 一句话总结
（1 句话，用日常语言说清楚这段在讲什么。避免"这是一个..."这种结构。）

## 通俗解释
（2-3 段。用类比、举例、生活化比喻解释。每段一个要点。）

## 关键术语释义
（列出本段出现的 2-4 个专业术语，每个给"中文译名 + 一句话释义"。例：注意力机制（Attention）—— 模型在处理信息时"挑重点"的能力。）

【写作约束】
- 保留专业术语首次出现时附中文释义，**不要为了通俗而删除术语**（Transformer/BERT/Attention 等保留英文）。
- 类比要贴切：避免"就像大脑一样"这类无信息量的类比；用具体场景（如"查字典时只看相关词条而不是从头翻"）。
- 数据 verbatim：具体数字原样保留（如 F1=93.2%）。
- LaTeX 公式原样保留，但用一句话说明它在算什么。

【边界】
- 选中片段含复杂公式：先给直觉解释，再说公式在算什么，不要逐步推导。
- 选中片段是实验设置：用"做这个实验就像..."的句式解释设计意图。`,
    userPrompt: '请用通俗易懂的语言解释论文第{page}页的以下内容：\n"{content}"',
  },

};

/**
 * 默认模板列表（统一格式）
 *
 * 这是应用首次使用时的初始模板列表，包含 6 个面向用户的核心内置模板。
 * 每个模板都是统一的数据结构，包含以下字段：
 * - id: 模板唯一标识（内置模板使用 PROMPT_TEMPLATES 的键名）
 * - label: UI 显示的中文名称
 * - template: 模板文本（支持 {page}/{content} 占位符）
 * - contextTypes: 适用的上下文场景数组（决定模板出现在哪些右键菜单中）
 * - isBuiltin: 是否为内置模板（true 表示可恢复默认）
 *
 * 用途：
 * 1. 首次加载时作为初始模板数据
 * 2. 模板管理弹窗中"恢复默认"按钮从这里获取原始文本
 * 3. 用户删除所有模板后可一键恢复到此状态
 *
 * @type {Array<{id: string, label: string, template: string, contextTypes: string[], isBuiltin: boolean}>}
 */
const DEFAULT_TEMPLATES = [ // 默认模板列表（6 个面向用户的核心内置模板）
  // ===== PDF 选中文本场景的 5 个操作模板 =====
  { // PDF 解释模板
    id: 'pdf_selection', // 模板唯一标识，与 PROMPT_TEMPLATES 的键名一致
    label: '解释', // UI 显示名称（右键菜单中的文字）
    systemPrompt: PROMPT_TEMPLATES.pdf_selection.systemPrompt,
    template: PROMPT_TEMPLATES.pdf_selection.userPrompt, // 模板文本，从 PROMPT_TEMPLATES 取值
    contextTypes: ['pdf_selection'], // 适用场景：PDF 选中文本的右键菜单
    isBuiltin: true, // 标记为内置模板，支持"恢复默认"
  },
  { // PDF 翻译模板
    id: 'pdf_translate', // 模板唯一标识
    label: '翻译', // UI 显示名称
    systemPrompt: PROMPT_TEMPLATES.pdf_translate.systemPrompt,
    template: PROMPT_TEMPLATES.pdf_translate.userPrompt, // 翻译模板文本
    contextTypes: ['pdf_selection'], // 适用场景：PDF 选中文本
    isBuiltin: true, // 标记为内置模板
  },
  { // PDF 总结模板
    id: 'pdf_summarize', // 模板唯一标识
    label: '总结', // UI 显示名称
    systemPrompt: PROMPT_TEMPLATES.pdf_summarize.systemPrompt,
    template: PROMPT_TEMPLATES.pdf_summarize.userPrompt, // 总结模板文本
    contextTypes: ['pdf_selection'], // 适用场景：PDF 选中文本
    isBuiltin: true, // 标记为内置模板
  },
  { // PDF 学术评价模板
    id: 'pdf_critique', // 模板唯一标识
    label: '学术评价', // UI 显示名称
    systemPrompt: PROMPT_TEMPLATES.pdf_critique.systemPrompt,
    template: PROMPT_TEMPLATES.pdf_critique.userPrompt, // 学术评价模板文本
    contextTypes: ['pdf_selection'], // 适用场景：PDF 选中文本
    isBuiltin: true, // 标记为内置模板
  },
  { // PDF 相关文献模板
    id: 'pdf_related', // 模板唯一标识
    label: '相关文献', // UI 显示名称
    systemPrompt: PROMPT_TEMPLATES.pdf_related.systemPrompt,
    template: PROMPT_TEMPLATES.pdf_related.userPrompt, // 相关文献模板文本
    contextTypes: ['pdf_selection'], // 适用场景：PDF 选中文本
    isBuiltin: true, // 标记为内置模板
  },
  { // PDF 简单解释模板
    id: 'pdf_simplify', // 模板唯一标识
    label: '简单解释', // UI 显示名称
    systemPrompt: PROMPT_TEMPLATES.pdf_simplify.systemPrompt,
    template: PROMPT_TEMPLATES.pdf_simplify.userPrompt, // 简单解释模板文本
    contextTypes: ['pdf_selection'], // 适用场景：PDF 选中文本
    isBuiltin: true, // 标记为内置模板
  },
];

/**
 * 内置模板列表（独立数组，Phase 6 新增）
 *
 * 存储由各功能模块注册的内置模板，与用户自定义模板分开管理。
 * 内置模板通过 registerBuiltinTemplate() 函数注册，在模块初始化时由各个功能组件调用。
 *
 * 为什么需要独立的内置模板数组？
 * - 内置模板由代码定义，不应被用户覆盖删除
 * - 用户可以通过模板管理弹窗编辑模板文本，但可以"恢复默认"
 * - 与用户自定义模板分开存储，便于合并和去重
 *
 * 数据结构：Array<{id, label, template, contextTypes, isBuiltin}>
 * 与 _templates 相同的数据结构，但仅包含内置模板
 */
let _builtinTemplates: Array<{id: string; label: string; systemPrompt: string; template: string; contextTypes: string[]; isBuiltin: boolean}> = []; // 模块级内置模板列表，通过 registerBuiltinTemplate() 注册

/**
 * 统一模板列表（模块级变量）
 *
 * 存储当前所有已注册的模板（内置 + 用户自建），由 ChatPanel 在加载用户设置后
 * 通过 registerTemplates() 注入到此模块。所有模板查询函数（buildContextPrompt、
 * getAvailableTemplates、getTemplateLabel）都从该数组中读取数据。
 *
 * 注意：这是模块级可变状态，有意设计为在多个组件实例间共享。
 * 通过 registerTemplates() 函数统一管理状态变更，避免直接修改。
 * 未来可考虑迁移到 React Context，但当前设计中模块级状态更高效。
 *
 * 数据结构：Array<{id, label, template, contextTypes, isBuiltin}>
 * - id: 模板唯一标识（内置模板如 'pdf_selection'，自建模板如 'custom_1713086400000'）
 * - label: UI 显示的中文名称
 * - template: 模板文本（支持 {page}/{content} 占位符）
 * - contextTypes: 适用的上下文场景数组（如 ['pdf_selection']）
 * - isBuiltin: 是否为内置模板（决定是否显示"恢复默认"按钮）
 *
 * 默认为空数组，ChatPanel 加载设置后会调用 registerTemplates() 填充。
 */
let _templates: Array<{id: string; label: string; systemPrompt: string; template: string; contextTypes: string[]; isBuiltin: boolean}> = []; // 模块级统一模板列表（内置 + 用户自建），通过 registerTemplates() 统一管理

/**
 * 注册内置模板（Phase 6 新增）
 *
 * 由各功能模块在初始化时调用，将模块特定的内置模板注册到系统中。
 * 内置模板会同时写入 _builtinTemplates 和 _templates 两个数组，
 * 确保模板查询函数能够找到这些模板。
 *
 * 使用场景：
 * - Phase 6 代码浏览器组件初始化时，注册 code_selection 等内置模板
 * - 未来新增功能模块（如标注等）可以注册自己的内置模板
 *
 * 为什么需要这个函数？
 * - 内置模板不应硬编码在 DEFAULT_TEMPLATES 中，保持模块解耦
 * - 各功能模块可以独立管理自己的内置模板，无需修改 ContextBridge
 * - 支持运行时动态注册（代码浏览器组件挂载时注册）
 *
 * @param {object} template - 模板对象，包含 { id, label, template, contextTypes }
 *   - id: 模板唯一标识（如 'code_selection', 'code_explain'）
 *   - label: UI 显示的中文名称
 *   - template: 模板文本（支持占位符）
 *   - contextTypes: 适用的上下文场景数组
 */
export function registerBuiltinTemplate(template: {id: string; label: string; systemPrompt?: string; template: string; contextTypes: string[]}) { // 导出内置模板注册函数
  // 验证模板对象的必要字段（systemPrompt 可选：旧调用方未传时回退到空字符串）
  if (!template || !template.id || !template.template) { // 缺少必要字段
    console.error('无效的内置模板:', template); // 输出错误信息
    return; // 提前返回
  }

  // 标记为内置模板（添加 isBuiltin 字段）
  // systemPrompt 兜底：调用方未传时尝试从 PROMPT_TEMPLATES 取（如同 id 的内置模板有默认 system 段）；
  // 仍取不到则空字符串，让 LLM 走默认 persona（适用于 code_selection 等非论文场景的内置模板）
  const fallbackSys = (PROMPT_TEMPLATES as Record<string, { systemPrompt: string; userPrompt: string }>)[template.id]?.systemPrompt ?? '';
  const builtinTemplate = { // 构建完整的内置模板对象
    ...template, // 保留原始字段
    systemPrompt: template.systemPrompt ?? fallbackSys,
    isBuiltin: true, // 标记为内置模板，支持"恢复默认"功能
  };

  // 检查 _builtinTemplates 中是否已存在同名模板
  const existingBuiltinIndex = _builtinTemplates.findIndex(t => t.id === template.id); // 查找现有模板
  if (existingBuiltinIndex >= 0) { // 已存在同名内置模板
    // 替换现有模板（支持模块重新加载时的模板更新）
    _builtinTemplates[existingBuiltinIndex] = builtinTemplate; // 替换数组中的模板
  } else { // 不存在同名内置模板
    // 追加新模板到数组末尾
    _builtinTemplates.push(builtinTemplate); // 添加到内置模板列表
  }

  // 同步更新 _templates 数组，确保模板查询函数能够找到新注册的内置模板
  // 检查 _templates 中是否已存在同名模板
  const existingTemplateIndex = _templates.findIndex(t => t.id === template.id); // 查找现有模板
  if (existingTemplateIndex >= 0) { // 已存在同名模板
    // 如果现有模板是用户自定义模板（isBuiltin: false），保留用户版本，不覆盖
    // 如果现有模板是内置模板（isBuiltin: true），替换为最新版本
    if (_templates[existingTemplateIndex].isBuiltin) { // 现有模板是内置模板
      _templates[existingTemplateIndex] = builtinTemplate; // 替换为最新版本
    }
    // 如果是用户自定义模板，不做任何操作（用户编辑的版本优先级更高）
  } else { // 不存在同名模板
    // 追加新模板到数组末尾
    _templates.push(builtinTemplate); // 添加到统一模板列表
  }
}

/**
 * 注册统一模板列表
 *
 * ChatPanel 组件在从后端加载用户设置后调用此函数，将模板数据注入到本模块。
 * 支持两种格式的数据：
 * 1. 新格式：{ templates: [...] } — 统一的模板数组
 * 2. 旧格式：{ overrides: {...}, customs: [...] } — 自动迁移为新格式
 *
 * 旧格式迁移逻辑：
 * - 以 DEFAULT_TEMPLATES 为基础，将 overrides 中的覆盖文本应用到对应模板
 * - 然后追加 customs 数组中的用户自建模板（标记 isBuiltin: false）
 * - 迁移后统一存储为新格式
 *
 * Phase 6 更新：合并时保留内置模板数组中的模板，
 * 确保通过 registerBuiltinTemplate() 注册的模板不会被用户数据覆盖。
 *
 * @param {object} data - 模板数据对象（支持新旧两种格式）
 */
export function registerTemplates(data: { templates?: typeof _templates; overrides?: Record<string, string>; customs?: Array<{id: string; label: string; template: string; contextTypes: string[]}> } | null) { // 导出模板注册函数，供 ChatPanel 调用；返回最终的 _templates 数组
  // 兜底工具：从 PROMPT_TEMPLATES 取 systemPrompt（旧用户 settings 数据无此字段时自动回填）
  const resolveSystemPrompt = (id: string, explicit?: string): string => {
    if (explicit && typeof explicit === 'string') return explicit;
    const fallback = (PROMPT_TEMPLATES as Record<string, { systemPrompt: string; userPrompt: string }>)[id];
    return fallback?.systemPrompt ?? '';
  };

  // 如果传入的数据为空或无效，使用默认模板列表，确保应用始终有可用模板
  if (!data) { // 没有传入数据
    _templates = getDefaultTemplates(); // 使用默认模板列表
  } else {
    // 检测数据格式：新格式有 templates 数组，旧格式有 overrides/customs
    if (data.templates && Array.isArray(data.templates)) { // 新格式：统一模板数组
      // 兼容旧数据：每个 entry 若缺 systemPrompt，按 id 从 PROMPT_TEMPLATES 回填
      _templates = data.templates.map(t => ({
        ...t,
        systemPrompt: resolveSystemPrompt(t.id, (t as any).systemPrompt),
      }));
    } else if (data.overrides || data.customs) { // 旧格式：需要迁移
      // 以默认模板列表为基础进行迁移
      const overrides = data.overrides || {}; // 获取覆盖映射（可能为空）
      const customs = data.customs || []; // 获取自定义模板数组（可能为空）

      // 将默认模板列表中的每个模板应用覆盖文本
      // 注意：旧 overrides 只覆盖 user prompt 文本（template 字段），systemPrompt 仍从 DEFAULT_TEMPLATES 继承
      const migrated = DEFAULT_TEMPLATES.map(tpl => { // 遍历默认模板
        if (overrides[tpl.id]) { // 如果该模板存在用户覆盖文本
          return { // 返回覆盖后的模板
            ...tpl, // 保留原有的 id、label、systemPrompt、contextTypes、isBuiltin
            template: overrides[tpl.id], // 使用覆盖后的 user prompt 文本
          };
        }
        return { ...tpl }; // 没有覆盖则保持原样（返回浅拷贝）
      });

      // 追加用户自建模板（标记为非内置模板）
      // 自定义模板若未显式给 systemPrompt，按 id 从 PROMPT_TEMPLATES 兜底；找不到则空字符串
      const customTemplates = customs.map(c => ({ // 遍历旧格式的自定义模板
        ...c, // 保留原有的 id、label、template、contextTypes
        systemPrompt: resolveSystemPrompt(c.id, (c as any).systemPrompt),
        isBuiltin: false, // 标记为非内置模板（用户自建）
      }));

      // 合并迁移后的内置模板和用户自建模板
      _templates = [...migrated, ...customTemplates]; // 组合成统一数组
    } else { // 无法识别的格式
      _templates = getDefaultTemplates(); // 回退到默认模板列表
    }
  }

  // ===== 合并 DEFAULT_TEMPLATES 中新增的默认模板 =====
  // 当应用升级后 DEFAULT_TEMPLATES 新增了模板，
  // 而用户之前保存的数据中没有这些模板时，自动补充到 _templates 中，
  // 确保新功能立即可用，无需用户手动添加或重置模板
  for (const defaultTpl of DEFAULT_TEMPLATES) { // 遍历所有默认模板
    const existingIndex = _templates.findIndex(t => t.id === defaultTpl.id); // 查找是否已存在
    if (existingIndex < 0) { // 不存在同名模板，追加到数组末尾
      _templates.push({ ...defaultTpl }); // 补充新增的默认模板
    }
  }

  // ===== Phase 6: 合并内置模板数组 =====
  // 将通过 registerBuiltinTemplate() 注册的内置模板合并到 _templates 中
  // 确保各功能模块注册的内置模板不会被用户数据覆盖
  if (_builtinTemplates.length > 0) { // 如果有内置模板需要合并
    // 遍历内置模板数组，逐个检查是否需要添加到 _templates
    for (const builtin of _builtinTemplates) { // 遍历每个内置模板
      // 检查 _templates 中是否已存在同名模板
      const existingIndex = _templates.findIndex(t => t.id === builtin.id); // 查找现有模板
      if (existingIndex < 0) { // 不存在同名模板，追加到数组末尾
        _templates.push(builtin); // 添加内置模板到统一列表
      }
      // 如果存在同名模板：
      // - 如果是用户自定义模板（isBuiltin: false），保留用户版本
      // - 如果是内置模板（isBuiltin: true），保持不变（不需要替换）
    }
  }

  return _templates; // 返回最终的模板列表，供 ChatPanel 同步 React state
}

/**
 * 获取默认模板列表的深拷贝
 *
 * 返回 DEFAULT_TEMPLATES 的深拷贝，用于：
 * 1. ChatPanel 初始化时的默认模板数据
 * 2. 模板管理弹窗中"恢复默认"操作获取原始文本
 * 3. 用户清空所有模板后恢复初始状态
 *
 * 为什么返回深拷贝？
 * - 防止外部代码意外修改 DEFAULT_TEMPLATES 常量
 * - 每个调用者获得独立的副本，互不干扰
 *
 * @returns {Array<{id: string, label: string, systemPrompt: string, template: string, contextTypes: string[], isBuiltin: boolean}>}
 *   默认模板数组的深拷贝
 */
export function getDefaultTemplates() { // 导出默认模板获取函数
  // 使用 JSON 序列化/反序列化实现深拷贝
  // DEFAULT_TEMPLATES 中只包含基本类型和数组，适合此方式
  return JSON.parse(JSON.stringify(DEFAULT_TEMPLATES)); // 返回深拷贝的默认模板列表
}

/**
 * 构造上下文注入的 prompt（统一模板版）
 *
 * 根据上下文类型和载荷数据，生成完整的 AI 提示词。
 * 模板解析优先级（已简化为两级）：
 *   1. 在统一模板列表 _templates 中查找匹配 ID 的模板
 *   2. 兜底：从 PROMPT_TEMPLATES 硬编码常量中查找
 *
 * @param {string} type - 上下文类型（如 'pdf_selection'）
 * @param {object} payload - 载荷数据对象
 * @param {string} payload.content - 主要内容（选中文本/原文）
 * @param {string} [payload.title] - 标题/摘要（可选）
 * @param {number} [payload.source_page] - 来源页码（可选）
 * @param {string} [templateKey] - 可选的模板 ID，用于指定使用哪个模板
 * @returns {string} 生成的 prompt 文本，可直接用于发送给 AI
 */
export function buildContextPrompt(
  type: string,
  payload: Record<string, unknown>,
  templateKey?: string,
): { systemPrompt: string; userPrompt: string } {
  // 优先级 1：在统一模板列表中按 templateKey 查找
  // 优先级 2：按 type 在统一模板列表中查找
  let found = templateKey ? _templates.find(t => t.id === templateKey) : undefined;
  if (!found) found = _templates.find(t => t.id === type);

  // 拆出两段：systemPrompt 缺失时从 PROMPT_TEMPLATES 同 id 兜底（旧数据兼容）
  let systemPromptRaw: string;
  let userPromptRaw: string;
  if (found) {
    const constEntry = (PROMPT_TEMPLATES as Record<string, { systemPrompt: string; userPrompt: string }>)[found.id];
    systemPromptRaw = found.systemPrompt ?? constEntry?.systemPrompt ?? '';
    userPromptRaw = found.template;
  } else {
    // _templates 里没有，从 PROMPT_TEMPLATES 兜底（按 templateKey 优先，再按 type，最后 pdf_selection）
    const constEntry = (PROMPT_TEMPLATES as Record<string, { systemPrompt: string; userPrompt: string }>)[templateKey ?? type] ?? PROMPT_TEMPLATES.pdf_selection;
    systemPromptRaw = constEntry.systemPrompt;
    userPromptRaw = constEntry.userPrompt;
  }

  // 占位符替换：对两段各执行一次（支持任意自定义占位符，如 {page}/{content}）
  // 使用函数形式 replace，避免 value 中的 $ 触发 String.replace 的特殊替换模式
  const replacePlaceholders = (text: string): string => {
    let result = text;
    for (const [key, value] of Object.entries(payload as Record<string, string | number | undefined>)) {
      result = result.replace(new RegExp(`\\{${key}\\}`, 'g'), () => String(value ?? ''));
    }
    return result;
  };

  return {
    systemPrompt: replacePlaceholders(systemPromptRaw).trim(),
    userPrompt: replacePlaceholders(userPromptRaw).trim(),
  };
}

/**
 * 获取指定上下文类型可用的模板列表（统一模板版）
 *
 * 从统一模板列表中筛选出适用于指定上下文类型的模板 ID 数组，
 * 用于 PdfViewer 渲染右键菜单选项。
 *
 * 筛选逻辑：
 * - 模板的 contextTypes 数组中包含指定的 type 时，视为匹配
 * - 返回的数组保持模板在列表中的原始顺序
 *
 * @param {string} type - 上下文类型（如 'pdf_selection'）
 * @returns {string[]} 该类型可用的模板 ID 数组
 *
 * @example
 * // 假设模板列表中有内置的 pdf_translate 和用户自建的 custom_123（contextTypes 含 pdf_selection）
 * getAvailableTemplates('pdf_selection')
 * // 返回: ['pdf_selection', 'pdf_translate', 'pdf_summarize', 'pdf_critique', 'pdf_related', 'custom_123']
 */
export function getAvailableTemplates(type: string) { // 导出可用模板查询函数
  // 从统一模板列表中筛选 contextTypes 包含指定 type 的模板
  return _templates // 遍历所有已注册的模板
    .filter(t => t.contextTypes?.includes(type)) // 只保留适用场景包含当前类型的模板
    .map(t => t.id); // 提取模板 ID 作为返回值（用于菜单项的 key）
}

/**
 * 获取模板的 UI 显示标签（统一模板版）
 *
 * 从统一模板列表中查找指定 ID 的模板，返回其 label（中文显示名）。
 * 如果找不到匹配的模板，回退到硬编码的标签映射或默认值。
 *
 * @param {string} key - 模板 ID（如 'pdf_translate', 'custom_1713086400000'）
 * @returns {string} 模板的中文名称
 *
 * @example
 * getTemplateLabel('pdf_selection')
 * // 返回: "解释"
 *
 * @example
 * getTemplateLabel('custom_1713086400000')
 * // 返回: "批判性翻译"（用户自建模板的 label）
 */
export function getTemplateLabel(key: string) { // 导出模板标签查询函数
  // 优先级 1：在统一模板列表中按 ID 查找
  const found = _templates.find(t => t.id === key); // 遍历模板列表，匹配 ID
  if (found) { // 找到了匹配的模板
    return found.label; // 返回该模板的中文标签
  }

  // 优先级 2：回退到硬编码的标签映射（兜底保护，防止模板被删除后标签丢失）
  const labels = { // 内置模板的硬编码标签映射（兜底用）
    pdf_selection: '解释', // PDF 选中文本的"解释"操作
    pdf_translate: '翻译', // PDF 选中文本的"翻译"操作
    pdf_summarize: '总结', // PDF 选中文本的"总结"操作
    pdf_critique: '学术评价', // PDF 选中文本的"学术评价"操作
    pdf_related: '相关文献', // PDF 选中文本的"相关文献"操作
    pdf_simplify: '简单解释', // PDF 选中文本的"简单解释"操作
  };

  // 在硬编码映射中查找，找不到则返回默认值"解释"
  return (labels as Record<string, string>)[key] || '解释'; // 返回标签或默认值
}

/**
 * ContextBridge 组件
 *
 * MVP 阶段：作为工具模块存在，导出工具函数，不渲染任何 UI
 * 为什么导出为组件而不是纯工具模块？
 * - 保持组件树的完整性，便于未来扩展
 * - 可以在 JSX 中以组件形式引用（如 <ContextBridge />）
 * - 符合 React 项目的一般组织方式
 *
 * v2 阶段计划：
 * - 渲染为可视化的"拖拽中转站"
 * - 用户可以拖拽 PDF 文本、段落等到中转站
 * - 从中转站拖拽到 ChatPanel 发起对话
 * - 支持多条内容的组合和编辑
 *
 * @returns {null} MVP 阶段不渲染任何内容
 */
export default function ContextBridge() { // 导出默认 ContextBridge 组件
  // MVP 阶段：不渲染任何 UI
  // 未来可能渲染为可拖拽的中转站区域
  return null; // 返回 null，不渲染任何 DOM 元素
}
