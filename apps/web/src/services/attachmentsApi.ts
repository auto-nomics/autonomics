/**
 * 附件 API 模块（autonomics）
 *
 * autonomics 没有附件表 —— 每篇文章至多一份 fulltext（PDF）。读侧由
 * mapping.synthesizeAttachments 从 fulltext 元信息合成单个主附件；写侧
 * （上传 / 删除 / 切主附件）属于 Phase 3 导入管线，本阶段桩化。
 *
 * 接口对应关系（保持 jayread 签名）：
 * - getAttachments(paperId)  → GET  /api/v1/bib/articles/{enc}（合成）
 * - uploadAttachments(...)   → 桩
 * - getAttachmentUrl(id)     → URL 字符串（合成附件 id 内含 fulltext 路径信息）
 * - deleteAttachment(id)     → 桩
 * - setPrimaryAttachment(id) → 桩
 */
import request from './client';
import { fromSafeId, encodeIdSegment, synthesizeAttachments, type BibArticleDetail } from './mapping';
import type {
  GetAttachmentsResponse,
  UploadAttachmentsResponse,
  DeleteAttachmentResponse,
  SetPrimaryAttachmentResponse,
} from '@/types';

// ============================================================
// 合成附件 id 的编解码
// ============================================================

/**
 * 合成附件 id 的固定前缀。`ft-<safeId>`，safeId 只含 [A-Za-z0-9_-]，
 * 所以整个 id 可安全放进 URL query（PaperReaderPage 用 ?attachmentId= 传递）。
 */
export const SYNTHETIC_ATTACHMENT_PREFIX = 'ft-';

/**
 * 由合成附件 id 还原论文 safeId。
 *
 * jayread 的 getAttachmentUrl(attachmentId) 只拿得到附件 id，没有论文 id；
 * 合成 id 里把 safeId 编码进去，让这个函数仍能定位到正确的 fulltext。
 */
export function paperSafeIdOfAttachment(attachmentId: string): string | null {
  if (!attachmentId.startsWith(SYNTHETIC_ATTACHMENT_PREFIX)) return null;
  return attachmentId.slice(SYNTHETIC_ATTACHMENT_PREFIX.length);
}

// ============================================================
// 读侧（合成）
// ============================================================

/**
 * 获取论文的所有附件列表
 *
 * 从 `GET /articles/{enc}` 信封顶层的 `fulltext` 合成（Article 本体没有任何
 * 全文字段）。有 PDF → 一个 `is_primary: true` 的附件；无 PDF → 空列表
 * （列表行不展开附件区）。
 *
 * 正文切片只要 1 个字符 —— 这里只读 file_path / file_size 等元数据。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<GetAttachmentsResponse>} 附件列表和总数
 */
export async function getAttachments(paperId: string, signal?: AbortSignal): Promise<GetAttachmentsResponse> {
  const realId = fromSafeId(paperId);
  const detail = await request<BibArticleDetail>(
    `/articles/${encodeIdSegment(realId)}?offset=0&limit=1`,
    { signal },
  );
  const attachments = synthesizeAttachments(detail.article, detail.fulltext);
  return { attachments, total: attachments.length };
}

// ============================================================
// 写侧（桩）
// ============================================================

/**
 * 上传论文附件（支持多文件）
 *
 * ⚠️ 桩：autonomics 的 fulltext 上传端点属 Phase 3 导入管线。
 * 保持签名不变，调用方（useAttachmentManager）会 toast 失败提示。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 3/5)
 */
export async function uploadAttachments(_paperId: string, _files: File[]): Promise<UploadAttachmentsResponse> {
  void _paperId; void _files;
  throw new Error('上传附件暂未开放（autonomics 附件上传尚未实现，见 plan Phase 3）');
}

/**
 * 获取附件文件的下载/预览 URL（同步函数）
 *
 * 合成附件的 id 是 `ft-<safeId>`，解码出论文 safeId 后指向 `/fulltext/raw`。
 * 拿不到论文 id 时回落到一个必然 404 的路径（不抛异常 —— jayread 的签名
 * 是同步纯函数，调用方直接把它塞进 href/window.open）。
 *
 * @param {string} attachmentId - 附件 ID
 * @returns {string} 附件文件的 URL 字符串
 */
export function getAttachmentUrl(attachmentId: string): string {
  const paperSafeId = paperSafeIdOfAttachment(attachmentId);
  if (!paperSafeId) return `/api/v1/bib/attachments/${encodeURIComponent(attachmentId)}`;
  return `/api/v1/bib/articles/${encodeIdSegment(fromSafeId(paperSafeId))}/fulltext/raw`;
}

/**
 * 删除单个附件
 *
 * ⚠️ 桩：autonomics 的 fulltext 是文章的一部分，删除走文章编辑，没有
 * 独立的附件删除端点。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 3/5)
 */
export async function deleteAttachment(_attachmentId: string): Promise<DeleteAttachmentResponse> {
  void _attachmentId;
  throw new Error('删除附件暂未开放（autonomics 附件无独立删除端点）');
}

/**
 * 将指定附件设为论文的主 PDF
 *
 * ⚠️ 桩：每篇文章至多一份 fulltext，恒为主附件，「切换主 PDF」无意义。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function setPrimaryAttachment(_attachmentId: string): Promise<SetPrimaryAttachmentResponse> {
  void _attachmentId;
  throw new Error('切换主附件暂不可用（autonomics 每篇文献至多一份 PDF）');
}
