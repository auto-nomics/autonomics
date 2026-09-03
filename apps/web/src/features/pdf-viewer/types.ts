/**
 * PDF 查看器内部共享类型
 *
 * 统一管理 pdf-viewer 内部组件/ hooks 之间共享的数据结构。
 * 注意：这些类型与 @/types/models 中的 API 层类型不同。
 */

/** 高亮标注（PDFium 字符索引坐标） */
export interface Highlight {
  id: string;
  page: number;
  char_start: number;
  char_end: number;
  text: string;
  color?: string;
}

/** 搜索匹配结果 */
export interface SearchMatch {
  pageNum: number;
  charStart: number;
  charEnd: number;
  text: string;
}

/** 句子对齐（原文 ↔ 译文） */
export interface SentencePair {
  original: string;
  translated: string;
}

/** 页面尺寸（PDF 点单位） */
export interface PageSize {
  width: number;
  height: number;
}

/** 页面信息（含 viewport 与旋转） */
export interface PageInfo {
  width: number;
  height: number;
  view: [number, number, number, number];
  rotate: 0 | 90 | 180 | 270;
}
