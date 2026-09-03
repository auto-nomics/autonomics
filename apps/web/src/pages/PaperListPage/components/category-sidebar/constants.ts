/**
 * 分类侧边栏常量
 *
 * 分类侧边栏组件共享的常量定义
 */

/**
 * 支持导入的参考文献文件扩展名
 * 用于在分类侧边栏中验证文件导入
 */
export const BIBLIOGRAPHY_EXTENSIONS = [
  '.bib',   // BibTeX 格式
  '.ris',   // RIS 格式
  '.nbib',  // PubMed/MEDLINE 格式
  '.enw',   // EndNote 格式
  '.xml',   // 通用 XML 格式
  '.txt',   // 纯文本格式
] as const;

/**
 * 文件输入框的 accept 属性值
 * 包含 PDF 和参考文献格式
 */
export const FILE_INPUT_ACCEPT = '.pdf,.bib,.ris,.nbib,.enw,.xml,.txt';

/**
 * 分类树中使用的虚拟节点键名
 */
export const VIRTUAL_KEYS = {
  ALL: 'all',                    // 全部
  UNCATEGORIZED: 'uncategorized', // 未分类
} as const;

/**
 * 根级别的虚拟节点数量
 * 用于拖放排序时的偏移量计算
 */
export const VIRTUAL_NODE_COUNT = 2;
