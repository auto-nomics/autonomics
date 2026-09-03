/**
 * 题录导入 API 客户端
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 3)
 *
 * autonomics 的批量导入端点（`POST /articles/import/batch`，BibTeX/RIS/CSL-JSON，
 * hayagriva + RIS 解析）和 PDF 上传建档（`POST /articles/upload`）属于 Phase 3
 * 导入管线，本阶段尚未实现。全部函数显式 reject，调用方走既有 catch → toast，
 * UI 不崩；签名保持与 jayread 一致，Phase 3 落地时只换函数体。
 *
 * 注：`resolveDuplicates` 对应 jayread 的「导入去重解决」流程，autonomics 在
 * 服务端直接 upsert 合并，前端该流程整体删除（plan §3.2），所以这个函数
 * Phase 3 也不会复活，将在 Phase 5 随 UI 一起清扫。
 */

import type {
  ImportResult,
  DuplicateResolution,
  ResolveDuplicatesResult,
  AttachPdfResponse,
  DownloadPdfResponse,
} from '@/types';

/**
 * 导入题录文件
 *
 * ⚠️ 桩：autonomics 批量导入端点属 Phase 3。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 3)
 *
 * @param {File} file - 用户上传的题录文件对象
 * @param {string|null} [categoryId=null] - 可选的分类 ID
 * @param {string|null} [format=null] - 可选的格式提示
 * @returns {Promise<ImportResult>} 导入结果对象
 */
export async function importBibliography(
  _file: File,
  _categoryId: string | null = null,
  _format: string | null = null,
): Promise<ImportResult> {
  void _categoryId; void _format;
  throw new Error('题录批量导入暂未开放（autonomics 导入管线尚未实现，见 plan Phase 3）');
}

/**
 * 处理重复文献的去重选择
 *
 * ⚠️ 桩：autonomics 在服务端 upsert 合并，无需前端解决重复。此函数将随
 * resolveDuplicates 流程在 Phase 5 一并删除。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 3/5)
 */
export async function resolveDuplicates(
  _resolutions: DuplicateResolution[],
  _categoryId: string | null = null,
): Promise<ResolveDuplicatesResult> {
  void _categoryId;
  throw new Error('去重解决流程已由服务端合并替代，不再需要');
}

/**
 * 为元数据记录手动上传 PDF 文件
 *
 * ⚠️ 桩：autonomics 的 fulltext 上传端点属 Phase 3。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 3)
 *
 * @param {string} paperId - 论文 ID
 * @param {File} file - 用户上传的附件文件对象
 * @returns {Promise<AttachPdfResponse>} 操作结果
 */
export async function attachPdf(_paperId: string, _file: File): Promise<AttachPdfResponse> {
  void _file;
  throw new Error('上传 PDF 暂未开放（autonomics 导入管线尚未实现，见 plan Phase 3）');
}

/**
 * 通过 DOI 自动下载开放获取 PDF
 *
 * ⚠️ 桩：autonomics 无 Unpaywall 下载管线。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} paperId - 论文 ID（该论文必须有 DOI）
 * @returns {Promise<DownloadPdfResponse>} 下载结果
 */
export async function downloadPdf(_paperId: string): Promise<DownloadPdfResponse> {
  throw new Error('自动下载 PDF 暂不可用（autonomics 无开放获取下载管线）');
}
