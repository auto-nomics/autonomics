/**
 * Autonomics 文献类型前端常量定义
 *
 * 本文件定义了前端使用的所有文献类型显示配置，包括：
 * - 类型标签的颜色和图标
 * - 筛选下拉框的选项列表
 * - 类型感知的元数据字段映射
 *
 * 数据来源：后端 GET /papers/types 端点返回的 LITERATURE_TYPES 注册表
 * 设计原则：后端为类型的单一事实来源，前端仅负责展示逻辑
 */

import i18next from 'i18next';

/**
 * 文献类型显示配置
 *
 * 键名与后端 literature_types.py 的 LITERATURE_TYPES 保持完全一致。
 * 每种类型包含：
 * - label:     中文显示名称（用于标签、下拉框等）
 * - color:     Ant Design Tag 组件的颜色名
 * - icon:      Ant Design 图标组件名
 * - hidden:    是否在筛选下拉框中隐藏（如 dataset 为预留类型）
 * - fields:    该类型特有的元数据字段列表（对应 extra_metadata JSON 中的键名）
 *
 * @type {Object.<string, {label: string, color: string, icon: string, hidden?: boolean, fields: string[]}>}
 */
export const TYPE_DISPLAY = {
  // ---------- 期刊论文 ----------
  // 最常见的学术文献类型
  "article": {
    label: "期刊论文",                    // 类型标签显示文本
    color: "geekblue",                    // 标签颜色（Ant Design 蓝色系）
    icon: "FileTextOutlined",             // 图标：文件文本
    hidden: false,                        // 不隐藏，在筛选下拉框中显示
    fields: [                             // 特有元数据字段
      "volume",                           // 卷号
      "issue",                            // 期号
      "start_page",                       // 起始页码
      "end_page",                         // 结束页码
      "pages",                            // 页码范围
    ],
  },

  // ---------- 综述论文 ----------
  // 对某一领域的系统总结
  "review": {
    label: "综述",                        // 类型标签显示文本
    color: "purple",                      // 标签颜色（紫色系）
    icon: "FileTextOutlined",             // 图标：文件文本
    hidden: false,                        // 不隐藏
    fields: [
      "volume",
      "issue",
      "start_page",
      "end_page",
      "pages",
    ],
  },

  // ---------- 会议论文 ----------
  // 在学术会议上发表的论文
  "conference-paper": {
    label: "会议论文",                    // 类型标签显示文本
    color: "cyan",                        // 标签颜色（青色系）
    icon: "TeamOutlined",                 // 图标：团队（代表学术交流）
    hidden: false,                        // 不隐藏
    fields: [
      "conference",                       // 会议名称
      "start_page",
      "end_page",
      "pages",
    ],
  },

  // ---------- 书籍 ----------
  // 完整的学术专著或教材
  "book": {
    label: "书籍",                        // 类型标签显示文本
    color: "orange",                      // 标签颜色（橙色系）
    icon: "BookOutlined",                 // 图标：书籍
    hidden: false,                        // 不隐藏
    fields: [
      "isbn",                             // ISBN 号
      "editor",                           // 编辑者
      "address",                          // 出版地址
    ],
  },

  // ---------- 书籍章节 ----------
  // 学术书籍中的独立章节
  "book-chapter": {
    label: "书籍章节",                    // 类型标签显示文本
    color: "orange",                      // 标签颜色（与书籍同色系）
    icon: "BookOutlined",                 // 图标：书籍
    hidden: false,                        // 不隐藏
    fields: [
      "isbn",
      "editor",
      "address",
      "start_page",
      "end_page",
      "pages",
    ],
  },

  // ---------- 学位论文 ----------
  // 博士、硕士或学士学位论文
  "thesis": {
    label: "学位论文",                    // 类型标签显示文本
    color: "green",                       // 标签颜色（绿色系）
    icon: "ReadOutlined",                 // 图标：阅读
    hidden: false,                        // 不隐藏
    fields: [
      "institution",                      // 学位授予机构
      "address",                          // 机构地址
      "degree",                           // 学位级别
    ],
  },

  // ---------- 报告 ----------
  // 技术报告或研究报告
  "report": {
    label: "报告",                        // 类型标签显示文本
    color: "default",                     // 标签颜色（默认灰色）
    icon: "ProfileOutlined",              // 图标：文档
    hidden: false,                        // 不隐藏
    fields: [
      "institution",                      // 发布机构
      "number",                           // 报告编号
    ],
  },

  // ---------- 专利 ----------
  // 发明专利、实用新型专利等
  "patent": {
    label: "专利",                        // 类型标签显示文本
    color: "red",                         // 标签颜色（红色系）
    icon: "BulbOutlined",                 // 图标：灯泡（代表发明）
    hidden: false,                        // 不隐藏
    fields: [
      "patent_number",                    // 专利号
      "filing_date",                      // 申请日期
    ],
  },

  // ---------- 预印本 ----------
  // 尚未经过同行评审的研究手稿
  "preprint": {
    label: "预印本",                      // 类型标签显示文本
    color: "geekblue",                    // 标签颜色（蓝色系）
    icon: "ExperimentOutlined",           // 图标：实验
    hidden: false,                        // 不隐藏
    fields: [
      "repository",                       // 预印本平台名称
    ],
  },

  // ---------- 标准文献 ----------
  // 国家标准、行业标准等规范性文件
  "standard": {
    label: "标准",                        // 类型标签显示文本
    color: "volcano",                     // 标签颜色（火山色系）
    icon: "SafetyOutlined",               // 图标：安全/规范
    hidden: false,                        // 不隐藏
    fields: [
      "standard_number",                  // 标准号
      "issuer",                           // 发布机构
      "effective_date",                   // 实施日期
    ],
  },

  // ---------- 数据集 ----------
  // 学术研究数据集合（预留类型，暂不在筛选框显示）
  "dataset": {
    label: "数据集",                      // 类型标签显示文本
    color: "default",                     // 标签颜色（默认灰色）
    icon: "DatabaseOutlined",             // 图标：数据库
    hidden: true,                         // 隐藏：不在筛选下拉框中显示（预留类型）
    fields: [
      "repository",                       // 数据仓库
      "doi",                              // 数据集 DOI
    ],
  },

  // ---------- 其他 ----------
  // 无法归入上述类型的文献
  "other": {
    label: "其他",                        // 类型标签显示文本
    color: "default",                     // 标签颜色（默认灰色）
    icon: "FileOutlined",                 // 图标：通用文件
    hidden: false,                        // 不隐藏
    fields: [],                           // 无特有字段
  },

  // ---------- 以下为较少见类型，不在筛选框中显示 ----------

  "editorial": {
    label: "社论",
    color: "default",
    icon: "FileTextOutlined",
    hidden: true,
    fields: [],
  },
  "letter": {
    label: "通讯",
    color: "default",
    icon: "FileTextOutlined",
    hidden: true,
    fields: [],
  },
  "short-communication": {
    label: "短通讯",
    color: "geekblue",
    icon: "FileTextOutlined",
    hidden: true,
    fields: [],
  },
  "erratum": {
    label: "勘误",
    color: "default",
    icon: "FileTextOutlined",
    hidden: true,
    fields: [],
  },
  "data-paper": {
    label: "数据论文",
    color: "default",
    icon: "DatabaseOutlined",
    hidden: true,
    fields: [],
  },
  "software": {
    label: "软件",
    color: "default",
    icon: "CodeOutlined",
    hidden: true,
    fields: [],
  },
  "reference-entry": {
    label: "参考条目",
    color: "default",
    icon: "FileTextOutlined",
    hidden: true,
    fields: [],
  },
  "peer-review": {
    label: "同行评审",
    color: "default",
    icon: "FileTextOutlined",
    hidden: true,
    fields: [],
  },
};

/**
 * 用于筛选下拉框的类型选项列表
 *
 * 从 TYPE_DISPLAY 中过滤掉 hidden 为 true 的类型（如 dataset），
 * 生成适合 Ant Design Select 组件的 options 数组格式。
 *
 * @type {Array<{value: string, label: string}>}
 */
export const FILTERABLE_TYPES = Object.entries(TYPE_DISPLAY)
  .filter(([_, config]) => !config.hidden)  // 过滤掉 hidden 为 true 的预留类型
  .map(([key, config]) => ({
    value: key,      // 选项值：内部类型键名（如 "article"）
    label: config.label,  // 选项标签：中文显示名称（如 "期刊论文"）
  }));

/**
 * 类型别名映射表
 *
 * 将 BibTeX 原始类型名和 OpenAlex 类型名映射到 TYPE_DISPLAY 中的规范键名。
 * 例如 BibTeX 的 "inproceedings" 和 "phdthesis" 对应到前端已有的类型定义。
 */
const TYPE_ALIASES = {
  // BibTeX 常见别名
  "inproceedings": "conference-paper",
  "incollection": "book-chapter",
  "mastersthesis": "thesis",
  "phdthesis": "thesis",
  "techreport": "report",
  "misc": "other",
  "unpublished": "preprint",
  "proceedings": "conference-paper",
  "conference": "conference-paper",
  // OpenAlex 后端归一化后的旧值兼容（已修正，但数据库中可能有旧数据）
  "book_chapter": "book-chapter",
  "short_communication": "short-communication",
  "data_paper": "data-paper",
  "reference_entry": "reference-entry",
  "peer_review": "peer-review",
};

/**
 * 获取类型的显示配置
 *
 * 根据内部类型键名查找对应的显示配置，如果类型不存在则返回默认配置。
 * 自动解析 TYPE_ALIASES 中的别名。
 *
 * @param {string} typeKey - 内部类型键名（如 "article"、"thesis"）
 * @returns {{label: string, color: string, icon: string, fields: string[]}} 类型的显示配置
 */
export const getTypeDisplay = (typeKey: string) => {
  const resolved = (TYPE_ALIASES as Record<string, string>)[typeKey] || typeKey;
  return (TYPE_DISPLAY as Record<string, { label: string; color: string; icon: string; hidden: boolean; fields: string[] }>)[resolved] || TYPE_DISPLAY["other"];
};

/**
 * 获取类型的中文显示名称
 *
 * 便捷函数，直接返回类型的中文标签文本。
 * 用于在卡片、表格等位置快速显示类型名称。
 *
 * @param {string} typeKey - 内部类型键名
 * @returns {string} 类型的中文显示名称（如 "期刊论文"、"学位论文"）
 */
export const getTypeLabel = (typeKey: string) => {
  const resolved = (TYPE_ALIASES as Record<string, string>)[typeKey] || typeKey;
  const translationKey = `paperList:types.${resolved}`;
  const translated = i18next.t(translationKey);
  // i18next returns the key itself if no translation found, fall back to TYPE_DISPLAY label
  if (translated === translationKey) {
    return getTypeDisplay(typeKey).label;
  }
  return translated;
};

/**
 * 获取类型的标签颜色
 *
 * 便捷函数，直接返回类型标签应使用的 Ant Design 颜色名。
 *
 * @param {string} typeKey - 内部类型键名
 * @returns {string} Ant Design Tag 颜色名（如 "geekblue"、"purple"）
 */
export const getTypeColor = (typeKey: string) => {
  return getTypeDisplay(typeKey).color;  // 从配置中提取 color 字段
};

/**
 * 获取类型特有的元数据字段列表
 *
 * 返回该类型在 extra_metadata JSON 中应当展示的字段名数组，
 * 用于前端根据文献类型动态渲染不同的元数据展示区域。
 *
 * @param {string} typeKey - 内部类型键名
 * @returns {string[]} 字段名数组（如 ["volume", "issue", "start_page"]）
 */
export const getTypeFields = (typeKey: string) => {
  return getTypeDisplay(typeKey).fields;  // 从配置中提取 fields 数组
};

/**
 * extra_metadata 字段的中文显示名称映射
 *
 * 用于在前端将 JSON 字段名（如 "volume"）映射为用户友好的中文名称（如 "卷号"）。
 * 覆盖所有可能出现在 extra_metadata 中的字段。
 *
 * @type {Object.<string, string>}
 */
export const METADATA_FIELD_LABELS = {
  "volume": "卷号",                      // 期刊卷号
  "issue": "期号",                       // 期刊期号
  "start_page": "起始页",                // 文献起始页码
  "end_page": "结束页",                  // 文献结束页码
  "pages": "页码范围",                   // 页码范围（如 "1-10"）
  "keywords": "关键词",                  // 文献关键词
  "url": "链接",                         // 在线链接
  "isbn": "ISBN",                        // 国际标准书号
  "editor": "编辑",                      // 书籍编辑者
  "address": "出版地",                   // 出版地址
  "pmid": "PMID",                        // PubMed 唯一标识号
  "pmcid": "PMCID",                      // PubMed Central 唯一标识号
  "article_number": "文章编号",          // 部分期刊使用的文章编号
  "issn": "ISSN",                        // 国际标准连续出版物号
  "doi": "DOI",                          // 数字对象标识符
  "conference": "会议名称",              // 会议论文所属的学术会议
  "institution": "机构",                 // 学位论文的授予机构或报告的发布机构
  "degree": "学位级别",                  // 学位级别（如 "博士"、"硕士"）
  "patent_number": "专利号",             // 专利编号
  "filing_date": "申请日期",             // 专利申请日期
  "repository": "平台",                  // 预印本或数据集的存储平台
  "standard_number": "标准号",           // 标准文献的编号（如 "GB/T 7714-2015"）
  "issuer": "发布机构",                  // 标准的发布机构
  "effective_date": "实施日期",          // 标准的实施日期
  "number": "编号",                      // 报告编号
};

/**
 * 获取元数据字段的翻译显示名称
 */
export const getFieldLabel = (fieldKey: string) => {
  const translationKey = `paperList:metadata.${fieldKey}`;
  const translated = i18next.t(translationKey);
  if (translated === translationKey) {
    return (METADATA_FIELD_LABELS as Record<string, string>)[fieldKey] || fieldKey;
  }
  return translated;
};

/**
 * 格式化 extra_metadata 中的页码信息
 *
 * 根据 start_page 和 end_page 字段，生成统一的页码显示字符串。
 * 优先使用 start_page/end_page 组合，其次使用 pages 字段。
 *
 * @param {Object} extraMetadata - extra_metadata JSON 对象
 * @returns {string} 格式化后的页码字符串（如 "1-10"），无页码信息时返回空字符串
 */
export const formatPages = (extraMetadata: Record<string, unknown> | null) => {
  if (!extraMetadata) return "";  // 空对象直接返回空字符串

  // 优先使用 start_page + end_page 组合
  if (extraMetadata.start_page && extraMetadata.end_page) {
    return `${extraMetadata.start_page}-${extraMetadata.end_page}`;  // 如 "1-10"
  }
  // 仅有 start_page 的情况
  if (extraMetadata.start_page) {
    return extraMetadata.start_page;  // 如 "1"
  }
  // 回退到 pages 字段
  return extraMetadata.pages || "";  // 如 "1-10" 或空字符串
};

/**
 * 生成类型感知的期刊/出版信息行文本
 *
 * 根据文献类型，从 extra_metadata 中提取并格式化该类型的代表性元数据，
 * 用于在列表卡片中显示一行简洁的出版信息摘要。
 *
 * @param {string} paperType - 内部类型键名
 * @param {Object} extraMetadata - extra_metadata JSON 对象
 * @returns {string} 格式化后的信息行文本，无信息时返回空字符串
 */
export const formatTypeMetaLine = (paperType: string, extraMetadata: Record<string, unknown> | null) => {
  if (!extraMetadata || typeof extraMetadata !== "object") return "";  // 防御性检查

  switch (paperType) {
    case "article":    // 期刊论文：显示 卷(期): 页码
    case "review": {
      const parts = [];  // 收集各个显示部分
      if (extraMetadata.volume) parts.push(`Vol.${extraMetadata.volume}`);  // 卷号
      if (extraMetadata.issue) parts.push(`(${extraMetadata.issue})`);      // 期号
      const pages = formatPages(extraMetadata);                             // 页码
      if (pages) parts.push(`: ${pages}`);                                  // 添加页码
      return parts.join("") || "";  // 拼接为如 "Vol.123(45): 1-10"
    }

    case "conference-paper": {  // 会议论文：显示会议名称
      const parts = [];
      if (extraMetadata.conference) parts.push(extraMetadata.conference);  // 会议名称
      const pages = formatPages(extraMetadata);
      if (pages) parts.push(`: ${pages}`);
      return parts.join("") || "";
    }

    case "book":         // 书籍：显示 ISBN
    case "book-chapter":
      return extraMetadata.isbn ? `ISBN: ${extraMetadata.isbn}` : "";

    case "thesis":       // 学位论文：显示授予机构
      return extraMetadata.institution || "";

    case "report":       // 报告：显示发布机构
      return extraMetadata.institution || "";

    case "patent":       // 专利：显示专利号
      return extraMetadata.patent_number || "";

    case "standard":     // 标准：显示标准号
      return extraMetadata.standard_number || "";

    case "preprint":     // 预印本：显示平台
      return extraMetadata.repository || "";

    default:
      return "";  // 其他类型不显示额外信息
  }
};
