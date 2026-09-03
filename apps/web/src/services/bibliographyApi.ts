/**
 * 题录导入 API 客户端（autonomics 导入管线）
 *
 * - `importBibliography` → POST /articles/import/batch：服务端解析
 *   BibTeX / RIS / CSL-JSON（format=auto 按内容嗅探）、按标识符 upsert
 *   合并重复条目，响应携带 `{imported, duplicates, failed[], duplicate_details[], articles}`。
 * - `attachPdf` → POST /articles/{enc}/fulltext：multipart 上传原始文件，
 *   服务端按 SHA-256 内容寻址落盘并抽取纯文本。
 * - `resolveDuplicates` / `downloadPdf`：autonomics 无对应能力（服务端合并
 *   取代了前端去重；无 Unpaywall 下载管线），保留 jayread 签名的显式 reject，
 *   UI 入口在死代码清扫阶段移除。
 */

import request from './client';
import { fromSafeId, encodeIdSegment, encodeSafeId, type BibArticle } from './mapping';
import type {
  ImportResult,
  DuplicateResolution,
  ResolveDuplicatesResult,
  AttachPdfResponse,
  DownloadPdfResponse,
} from '@/types';

/** import/batch 响应（服务端原始形状；failed/duplicate_details 为数组） */
interface ImportBatchResponse {
  imported: number;
  duplicates: number;
  failed: Array<{ key: string; reason: string }>;
  duplicate_details: Array<{ title: string | null; identifiers: unknown; existing_id: string }>;
  articles: BibArticle[];
}

/** 扩展名 → 后端 format；无法判断返回 'auto'（服务端按内容嗅探） */
function formatForFile(file: File): string {
  const name = file.name.toLowerCase();
  if (name.endsWith('.bib')) return 'bibtex';
  if (name.endsWith('.ris')) return 'ris';
  if (name.endsWith('.json')) return 'csl_json';
  return 'auto'; // .txt 等交给服务端嗅探
}

/**
 * 导入题录文件
 *
 * 读文件为文本后 POST /articles/import/batch。重复条目由服务端按标识符
 * upsert 合并（重复数体现在 `duplicates`），**前端没有去重解决步骤**，
 * 因此返回的 `duplicate_details` 恒为空数组；`imported_paper_ids` 是实际
 * 入库（含合并落位）文章的 safeId 列表。
 *
 * @param {File} file - 用户上传的题录文件对象
 * @param {string|null} [categoryId=null] - 可选的分类 ID（导入后自动归入）
 * @param {string|null} [format=null] - 可选的格式提示（bibtex|ris|csl_json|auto）
 * @returns {Promise<ImportResult>} 导入结果对象
 */
export async function importBibliography(
  file: File,
  categoryId: string | null = null,
  format: string | null = null,
): Promise<ImportResult> {
  const content = await file.text();
  const body: Record<string, unknown> = {
    format: format || formatForFile(file),
    content,
  };
  if (categoryId !== null && categoryId !== undefined && categoryId !== '') {
    body.category_id = String(categoryId);
  }

  const res = await request<ImportBatchResponse>('/articles/import/batch', {
    method: 'POST',
    body,
  });

  return {
    imported: res.imported,
    duplicates: res.duplicates,
    failed: res.failed.length,
    // 服务端已合并重复条目，无需前端解决 → 恒为空
    duplicate_details: [],
    imported_paper_ids: (res.articles ?? []).map((article) => encodeSafeId(article.id)),
  };
}

/**
 * 处理重复文献的去重选择
 *
 * ⚠️ 桩：autonomics 在服务端 upsert 合并，无需前端解决重复。此函数将随
 * resolveDuplicates 流程在死代码清扫阶段一并删除。
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
 * POST /articles/{enc}/fulltext（multipart `file`）：服务端按内容寻址保存
 * 原始文件并抽取纯文本，返回 `{"fulltext": {...}}`。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {File} file - 用户上传的附件文件对象
 * @returns {Promise<AttachPdfResponse>} 操作结果
 */
export async function attachPdf(paperId: string, file: File): Promise<AttachPdfResponse> {
  const realId = fromSafeId(paperId);
  const form = new FormData();
  form.append('file', file, file.name);

  // 服务端同步抽取文本（含 OCR 回退），放宽超时
  await request('/articles/' + encodeIdSegment(realId) + '/fulltext', {
    method: 'POST',
    body: form,
    timeout: 300000,
  });

  return { message: 'PDF 已关联', paper_id: paperId };
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
