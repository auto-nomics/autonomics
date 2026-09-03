/**
 * @file cdn-fonts.ts
 *
 * CDN 字体配置 —— 浏览器环境的默认字体回退方案
 *
 * 【文件概述】
 * 本文件提供了基于 jsDelivr CDN 的字体回退配置，用于在浏览器环境中
 * 为 PDFium 按需加载缺失的字体。字体文件来源于 @embedpdf/fonts-* 系列
 * npm 包，通过 jsDelivr CDN 分发。
 *
 * 【设计决策】
 * 本文件故意与 font-fallback.ts 分离，原因如下：
 * - Node.js 用户通常从本地文件系统加载字体，不需要 CDN URL
 * - 分离后可以避免在 Node.js 打包产物中包含 CDN 地址和字体定义
 * - 仅在浏览器端代码（如 Web Worker）中导入此文件
 *
 * 【字体包说明】
 * 每种语言/字符集有独立的字体包，内含多种字重变体：
 * - @embedpdf/fonts-jp: 日语字体（7 种字重：Thin 到 Black）
 * - @embedpdf/fonts-kr: 韩语字体（7 种字重：Thin 到 Black）
 * - @embedpdf/fonts-sc: 简体中文字体（5 种字重：Light 到 Bold）
 * - @embedpdf/fonts-tc: 繁体中文字体（7 种字重：Thin 到 Black）
 * - @embedpdf/fonts-arabic: 阿拉伯语字体（Regular, Bold）
 * - @embedpdf/fonts-hebrew: 希伯来语字体（Regular, Bold）
 * - @embedpdf/fonts-latin: 拉丁/西里尔/希腊/越南语字体（9 种字重含斜体）
 */

import { FontCharset, type FontFile } from '../../models';
import type { FontFallbackConfig, FontVariant } from './font-fallback';

// 从各字体包导入字体定义文件列表（单一数据源，避免硬编码）
import { fonts as jpFonts } from '../../fonts/jp';
import { fonts as krFonts } from '../../fonts/kr';
import { fonts as scFonts } from '../../fonts/sc';
import { fonts as tcFonts } from '../../fonts/tc';
import { fonts as arabicFonts } from '../../fonts/arabic';
import { fonts as hebrewFonts } from '../../fonts/hebrew';
import { fonts as latinFonts } from '../../fonts/latin';

// ============================================================================
// CDN URL 构建器
// ============================================================================

/**
 * 根据包版本构建各语言字体包的 CDN 基础 URL
 *
 * 生成的 URL 格式为：
 * https://cdn.jsdelivr.net/npm/@embedpdf/fonts-{lang}@{version}/fonts
 *
 * jsDelivr 会自动解析 npm 包中的 /fonts 目录，提供静态文件服务。
 *
 * @param version - npm 包版本号，默认 'latest'（始终获取最新版）
 * @returns 各语言对应的 CDN 基础 URL 映射
 */
function buildCdnUrls(version: string = 'latest') {
  return {
    jp: `https://cdn.jsdelivr.net/npm/@embedpdf/fonts-jp@${version}/fonts`,
    kr: `https://cdn.jsdelivr.net/npm/@embedpdf/fonts-kr@${version}/fonts`,
    sc: `https://cdn.jsdelivr.net/npm/@embedpdf/fonts-sc@${version}/fonts`,
    tc: `https://cdn.jsdelivr.net/npm/@embedpdf/fonts-tc@${version}/fonts`,
    arabic: `https://cdn.jsdelivr.net/npm/@embedpdf/fonts-arabic@${version}/fonts`,
    hebrew: `https://cdn.jsdelivr.net/npm/@embedpdf/fonts-hebrew@${version}/fonts`,
    latin: `https://cdn.jsdelivr.net/npm/@embedpdf/fonts-latin@${version}/fonts`,
  };
}

/**
 * 将字体文件列表转换为 FontVariant 数组，并拼接 CDN 基础 URL
 *
 * 每个字体包导出的 fonts 数组包含该包内所有字体文件的元信息（文件名、字重、斜体），
 * 此函数将其转换为 FontFallbackManager 所需的 FontVariant 格式，同时拼接完整 URL。
 *
 * @param fonts - 字体文件元信息数组
 * @param baseUrl - CDN 基础 URL
 * @returns 可用于 FontFallbackConfig 的 FontVariant 数组
 */
function toFontVariants(fonts: FontFile[], baseUrl: string): FontVariant[] {
  return fonts.map((f) => ({
    url: `${baseUrl}/${f.file}`,
    weight: f.weight,
    italic: f.italic,
  }));
}

/**
 * 根据 CDN URL 构建完整的字体回退配置
 *
 * 将 PDF 字符集编号映射到对应语言字体包中的字体变体列表。
 * 某些字符集共用同一字体包（如西里尔、希腊和越南语共用 latin 包）。
 *
 * 字符集映射关系：
 * - SHIFTJIS (128) → 日语字体包
 * - HANGEUL (129) → 韩语字体包
 * - GB2312 (134) → 简体中文包
 * - CHINESEBIG5 (136) → 繁体中文包
 * - ARABIC (178) → 阿拉伯语包
 * - HEBREW (177) → 希伯来语包
 * - CYRILLIC (204) → 拉丁字体包（含西里尔字母支持）
 * - GREEK (161) → 拉丁字体包（含希腊字母支持）
 * - VIETNAMESE (163) → 拉丁字体包（含越南语支持）
 *
 * @param urls - 各语言的 CDN 基础 URL 映射
 * @returns 可直接传给 FontFallbackManager 的完整配置
 */
function buildCdnFontConfig(urls: ReturnType<typeof buildCdnUrls>): FontFallbackConfig {
  return {
    fonts: {
      [FontCharset.SHIFTJIS]: toFontVariants(jpFonts, urls.jp),
      [FontCharset.HANGEUL]: toFontVariants(krFonts, urls.kr),
      [FontCharset.GB2312]: toFontVariants(scFonts, urls.sc),
      [FontCharset.CHINESEBIG5]: toFontVariants(tcFonts, urls.tc),
      [FontCharset.ARABIC]: toFontVariants(arabicFonts, urls.arabic),
      [FontCharset.HEBREW]: toFontVariants(hebrewFonts, urls.hebrew),
      [FontCharset.CYRILLIC]: toFontVariants(latinFonts, urls.latin),
      [FontCharset.GREEK]: toFontVariants(latinFonts, urls.latin),
      [FontCharset.VIETNAMESE]: toFontVariants(latinFonts, urls.latin),
    },
  };
}

// ============================================================================
// 公共导出
// ============================================================================

/**
 * 默认 CDN 基础 URL（使用 @latest 版本）
 *
 * 指向 jsDelivr 上 @embedpdf/fonts-* 包的最新版本。
 * 生产环境中建议使用 createCdnFontConfig() 锁定具体版本以确保稳定性。
 */
export const FONT_CDN_URLS = buildCdnUrls('latest');

/**
 * 默认 CDN 字体配置（使用 @latest 版本）
 *
 * 这是浏览器端 Worker 引擎的默认字体回退配置。
 * 字体在 PDFium 需要时按需从 jsDelivr 加载，无需预下载。
 *
 * 使用 @latest 以始终获取最新版字体包，适合开发环境。
 * 生产环境建议通过 createCdnFontConfig() 指定具体版本。
 *
 * 包含的字体包：
 * - @embedpdf/fonts-jp: 日语（7 种字重：Thin 到 Black）
 * - @embedpdf/fonts-kr: 韩语（7 种字重：Thin 到 Black）
 * - @embedpdf/fonts-sc: 简体中文（5 种字重：Light 到 Bold）
 * - @embedpdf/fonts-tc: 繁体中文（7 种字重：Thin 到 Black）
 * - @embedpdf/fonts-arabic: 阿拉伯语（Regular, Bold）
 * - @embedpdf/fonts-hebrew: 希伯来语（Regular, Bold）
 * - @embedpdf/fonts-latin: 拉丁/西里尔/希腊/越南语（9 种字重含斜体）
 */
export const cdnFontConfig: FontFallbackConfig = buildCdnFontConfig(FONT_CDN_URLS);

/**
 * 创建指定版本的 CDN 字体配置
 *
 * 用于需要锁定字体包版本以保证稳定性的场景。
 *
 * @param version - npm 版本字符串（如 '1.0.0' 精确版本、'1' 主版本、'latest' 最新版）
 * @returns 使用指定版本 CDN URL 的 FontFallbackConfig
 *
 * @example 锁定精确版本：
 * ```typescript
 * const fontConfig = createCdnFontConfig('1.0.0');
 * ```
 *
 * @example 锁定主版本（推荐，自动获取补丁更新）：
 * ```typescript
 * const fontConfig = createCdnFontConfig('1');
 * ```
 */
export function createCdnFontConfig(version: string = 'latest'): FontFallbackConfig {
  return buildCdnFontConfig(buildCdnUrls(version));
}

// 重新导出类型，方便外部使用
export type { FontFallbackConfig };
