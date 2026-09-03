/**
 * JayRead 附件工具模块
 *
 * 提供附件系统所需的通用工具函数和常量：
 * - 文件类型图标映射（根据扩展名获取对应的 Ant Design 图标组件名）
 * - 文件类型颜色映射（根据扩展名获取对应的主题色）
 * - 文件分类函数（将扩展名归类为 PDF/Word/Excel 等大类）
 * - 文件大小格式化（将字节数转换为人类可读的 KB/MB/GB 格式）
 * - 文件上传 accept 属性字符串（用于限制文件选择对话框的文件类型）
 */

// ========== 文件分类到图标名称的映射 ==========
// 每种文件大类对应一个 Ant Design 图标组件名称
// 前端根据图标名称动态渲染对应的 <Icon /> 组件
// 图标名称对应 @ant-design/icons 中的组件
export const FILE_CATEGORY_ICON_MAP = {
  pdf: 'FilePdfOutlined',        // PDF 文件图标（红色调）
  word: 'FileWordOutlined',      // Word 文档图标（蓝色调）
  excel: 'FileExcelOutlined',    // Excel 表格图标（绿色调）
  ppt: 'FilePptOutlined',        // PowerPoint 演示文稿图标（橙色调）
  image: 'FileImageOutlined',    // 图片文件图标（紫色调）
  audio: 'SoundOutlined',        // 音频文件图标
  video: 'PlayCircleOutlined',   // 视频文件图标
  archive: 'FileZipOutlined',    // 压缩包文件图标
  code: 'CodeOutlined',          // 代码文件图标
  ebook: 'BookOutlined',         // 电子书图标
  data: 'DatabaseOutlined',      // 数据文件图标
  text: 'FileTextOutlined',      // 纯文本文件图标
  web: 'GlobalOutlined',         // 网页文件图标
  default: 'FileOutlined',       // 默认文件图标（未知类型）
};

// ========== 文件分类到主题颜色的映射 ==========
// 颜色用于图标着色和标签背景，与 Zotero 的文件类型颜色方案类似
// 每种文件大类对应一个 CSS 颜色值
export const FILE_CATEGORY_COLOR_MAP = {
  pdf: '#f5222d',       // 红色 - PDF 文件
  word: '#1890ff',      // 蓝色 - Word 文档
  excel: '#52c41a',     // 绿色 - Excel 表格
  ppt: '#fa8c16',       // 橙色 - PowerPoint 演示文稿
  image: '#722ed1',     // 紫色 - 图片文件
  audio: '#13c2c2',     // 青色 - 音频文件
  video: '#eb2f96',     // 品红 - 视频文件
  archive: '#8c8c8c',   // 灰色 - 压缩包
  code: '#faad14',      // 金色 - 代码文件
  ebook: '#2f54eb',     // 靛蓝 - 电子书
  data: '#a0d911',      // 黄绿 - 数据文件
  text: '#595959',      // 深灰 - 纯文本
  web: '#1890ff',       // 蓝色 - 网页文件
  default: '#8c8c8c',   // 灰色 - 默认/未知类型
};

// ========== 文件扩展名到分类的映射表 ==========
// 将具体的文件扩展名映射到文件大类
// 例如：'.docx' → 'word', '.xlsx' → 'excel'
const EXTENSION_TO_CATEGORY = {
  // ---- PDF 格式 ----
  pdf: 'pdf',
  // ---- Word 文档格式 ----
  doc: 'word',           // 旧版 Word 二进制格式
  docx: 'word',          // 新版 Word XML 格式
  // ---- Excel 表格格式 ----
  xls: 'excel',          // 旧版 Excel 二进制格式
  xlsx: 'excel',         // 新版 Excel XML 格式
  csv: 'data',           // CSV 归为数据文件
  // ---- PowerPoint 演示文稿格式 ----
  ppt: 'ppt',            // 旧版 PowerPoint 二进制格式
  pptx: 'ppt',           // 新版 PowerPoint XML 格式
  // ---- OpenDocument 格式（LibreOffice/OpenOffice）----
  odt: 'word',           // OpenDocument 文本，归类为 Word
  ods: 'excel',          // OpenDocument 表格，归类为 Excel
  odp: 'ppt',            // OpenDocument 演示文稿，归类为 PPT
  // ---- 纯文本和富文本 ----
  txt: 'text',           // 纯文本文件
  rtf: 'text',           // 富文本格式
  md: 'text',            // Markdown 文本
  // ---- 图片格式 ----
  jpg: 'image',          // JPEG 图片
  jpeg: 'image',         // JPEG 图片（另一种扩展名）
  png: 'image',          // PNG 图片
  gif: 'image',          // GIF 图片
  tiff: 'image',         // TIFF 图片（高质量无损）
  tif: 'image',          // TIFF 图片（另一种扩展名）
  bmp: 'image',          // BMP 位图
  svg: 'image',          // SVG 矢量图
  webp: 'image',         // WebP 图片
  // ---- 电子书格式 ----
  epub: 'ebook',         // EPUB 电子书
  // ---- 网页格式 ----
  html: 'web',           // HTML 网页
  htm: 'web',            // HTML 网页（短扩展名）
  mhtml: 'web',          // MHTML 网页归档
  // ---- 音频格式 ----
  mp3: 'audio',          // MP3 音频
  wav: 'audio',          // WAV 音频（无损）
  m4a: 'audio',          // M4A 音频（AAC 编码）
  ogg: 'audio',          // OGG 音频
  oga: 'audio',          // OGG 音频（短扩展名）
  flac: 'audio',         // FLAC 无损音频
  // ---- 视频格式 ----
  mp4: 'video',          // MP4 视频
  avi: 'video',          // AVI 视频
  mov: 'video',          // QuickTime 视频
  mkv: 'video',          // MKV 视频容器
  webm: 'video',         // WebM 视频
  // ---- 压缩格式 ----
  zip: 'archive',        // ZIP 压缩包
  rar: 'archive',        // RAR 压缩包
  '7z': 'archive',       // 7-Zip 压缩包
  tar: 'archive',        // TAR 归档文件
  gz: 'archive',         // Gzip 压缩文件
  // ---- 代码文件格式 ----
  py: 'code',            // Python 源代码
  r: 'code',             // R 语言源代码
  js: 'code',            // JavaScript 源代码
  ts: 'code',            // TypeScript 源代码
  m: 'code',             // MATLAB 源代码
  ipynb: 'code',         // Jupyter Notebook
  // ---- 数据文件格式 ----
  json: 'data',          // JSON 数据文件
  xml: 'data',           // XML 数据文件
};

/**
 * 根据文件扩展名获取文件分类
 *
 * 将具体的文件扩展名（如 'docx'、'xlsx'）映射到文件大类（如 'word'、'excel'）。
 * 用于前端选择对应的图标和颜色。
 *
 * @param {string} ext - 文件扩展名（小写，不含点号），如 'pdf'、'docx'
 * @returns {string} 文件分类字符串，如 'pdf'、'word'、'default'
 */
export function getFileCategory(ext: string) {
  // 将扩展名转为小写，确保大小写不敏感
  const lowerExt = (ext || '').toLowerCase();
  // 从映射表中查找分类，找不到则返回 'default'
  return (EXTENSION_TO_CATEGORY as Record<string, string>)[lowerExt] || 'default';
}

/**
 * 根据文件分类获取 Ant Design 图标名称
 *
 * @param {string} category - 文件分类（来自 getFileCategory 函数）
 * @returns {string} 图标组件名称，如 'FilePdfOutlined'
 */
export function getFileIconName(category: string) {
  return (FILE_CATEGORY_ICON_MAP as Record<string, string>)[category] || FILE_CATEGORY_ICON_MAP.default;
}

/**
 * 根据文件分类获取主题颜色
 *
 * @param {string} category - 文件分类（来自 getFileCategory 函数）
 * @returns {string} CSS 颜色值，如 '#f5222d'
 */
export function getFileColor(category: string) {
  return (FILE_CATEGORY_COLOR_MAP as Record<string, string>)[category] || FILE_CATEGORY_COLOR_MAP.default;
}

/**
 * 格式化文件大小为人类可读的字符串
 *
 * 将字节数转换为最适合的单位（B/KB/MB/GB/TB），
 * 保留适当的小数位数。
 *
 * 例如：
 * - 500 → "500 B"
 * - 1024 → "1.0 KB"
 * - 1536000 → "1.5 MB"
 * - 1073741824 → "1.0 GB"
 *
 * @param {number} bytes - 文件大小（字节数）
 * @returns {string} 格式化后的文件大小字符串
 */
export function formatFileSize(bytes: number) {
  // 防御性检查：无效值返回空字符串
  if (!bytes || bytes <= 0) return '0 B';

  // 定义单位列表和对应的进制（1024）
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  // 计算应该使用的单位索引
  // 使用对数运算：log1024(bytes) 得到单位层级
  const unitIndex = Math.floor(Math.log(bytes) / Math.log(1024));
  // 防止索引超出单位列表范围（如超大文件）
  const safeIndex = Math.min(unitIndex, units.length - 1);
  // 计算转换后的数值：字节 → 对应单位
  const size = bytes / Math.pow(1024, safeIndex);
  // 格式化输出：小于 1KB 显示整数，否则保留一位小数
  // 例如：500 B（整数），1.5 MB（一位小数）
  return safeIndex === 0
    ? `${size} ${units[safeIndex]}`          // 字节单位直接显示整数
    : `${size.toFixed(1)} ${units[safeIndex]}`; // 其他单位保留一位小数
}

/**
 * 文件上传 accept 属性字符串
 *
 * 用于 <input type="file" accept="..."> 的 accept 属性，
 * 限制文件选择对话框只显示支持的文件类型。
 * 格式为逗号分隔的 .扩展名 列表。
 *
 * 例如：".pdf,.doc,.docx,.ppt,.pptx,.xls,.xlsx,..."
 */
export const ATTACHMENT_ACCEPT_STRING = [
  // ---- 文档格式 ----
  '.pdf',                          // PDF 文件
  '.doc', '.docx',                 // Word 文档
  '.ppt', '.pptx',                 // PowerPoint 演示文稿
  '.xls', '.xlsx',                 // Excel 表格
  '.odt', '.ods', '.odp',          // OpenDocument 格式
  '.txt', '.rtf',                  // 纯文本和富文本
  '.epub',                         // 电子书
  // ---- 图片格式 ----
  '.jpg', '.jpeg',                 // JPEG 图片
  '.png',                          // PNG 图片
  '.gif',                          // GIF 图片
  '.tiff', '.tif',                 // TIFF 图片
  '.bmp',                          // BMP 位图
  '.svg',                          // SVG 矢量图
  '.webp',                         // WebP 图片
  // ---- 网页格式 ----
  '.html', '.htm',                 // HTML 网页
  '.mhtml',                        // MHTML 网页归档
  // ---- 音频格式 ----
  '.mp3', '.wav', '.m4a',          // 常见音频格式
  '.ogg', '.flac',                 // OGG 和 FLAC 音频
  // ---- 视频格式 ----
  '.mp4', '.avi', '.mov',          // 常见视频格式
  '.mkv', '.webm',                 // MKV 和 WebM 视频
  // ---- 数据格式 ----
  '.csv', '.json', '.xml',         // 数据文件
  // ---- 代码格式 ----
  '.py', '.r', '.js', '.ts',       // 编程语言源代码
  '.ipynb',                        // Jupyter Notebook
  // ---- 压缩格式 ----
  '.zip', '.rar', '.7z',           // 常见压缩格式
  '.tar', '.gz',                   // Unix 归档和压缩
].join(','); // 用逗号拼接为 accept 属性字符串
