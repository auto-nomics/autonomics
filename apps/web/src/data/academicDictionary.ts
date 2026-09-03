/**
 * 内置学术英汉词典
 *
 * 模块加载时将 JSON 数据构建为 Map，提供 O(1) 查找。
 * 用于双击术语翻译的第一层缓存，命中时即时返回，无需网络请求。
 *
 * @module academicDictionary
 */

import academicTerms from './academicTerms.json';

// 模块加载时一次性构建查找 Map
const termMap = new Map();
for (const [en, zh] of Object.entries(academicTerms.terms)) {
  // 标准化：小写 + trim + 合并多余空格
  const key = en.toLowerCase().trim().replace(/\s+/g, ' ');
  termMap.set(key, zh);
}

/**
 * 在内置学术词典中查找术语
 *
 * @param {string} text - 英文术语
 * @returns {string|null} 中文翻译，未找到返回 null
 */
export function lookupAcademicTerm(text: string) {
  if (!text || text.length < 2) return null;
  const key = text.toLowerCase().trim().replace(/\s+/g, ' ');
  return termMap.get(key) || null;
}

/**
 * 获取词典词条总数
 */
export function getDictionarySize() {
  return termMap.size;
}
