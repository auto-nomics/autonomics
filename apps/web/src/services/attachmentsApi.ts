/**
 * JayRead 附件 API 模块
 *
 * 提供论文附件的增删查和主 PDF 切换接口封装。
 * 附件是论文的补充文件（PDF、Word、图片、数据文件等），
 * 用户可以在论文列表页展开论文行查看和管理附件。
 *
 * 接口对应关系：
 * - getAttachments(paperId)  → GET  /api/papers/{paperId}/attachments
 * - uploadAttachments(paperId, files) → POST /api/papers/{paperId}/attachments
 * - getAttachmentUrl(id)     → URL 字符串 /api/attachments/{id}
 * - deleteAttachment(id)     → DELETE /api/attachments/{id}
 * - setPrimaryAttachment(id) → PUT /api/attachments/{id}/set-primary
 */

import request, { invalidateCache } from './client';
import type {
  GetAttachmentsResponse,
  UploadAttachmentsResponse,
  DeleteAttachmentResponse,
  SetPrimaryAttachmentResponse,
} from '@/types';

/**
 * 获取论文的所有附件列表
 *
 * 当用户在论文列表页展开某篇论文行时调用此函数，
 * 获取该论文的所有附件元数据（不包含文件内容本身）。
 * 附件内容通过 getAttachmentUrl() 获取的 URL 单独下载。
 *
 * @param {string} paperId - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<GetAttachmentsResponse>} 附件列表和总数
 */
export async function getAttachments(paperId: string, signal?: AbortSignal): Promise<GetAttachmentsResponse> {
  return request<GetAttachmentsResponse>(`/papers/${paperId}/attachments`, { signal });
}

/**
 * 上传论文附件（支持多文件）
 *
 * 使用 FormData 格式上传一个或多个文件作为论文的附件。
 * 每个文件会被保存到 data/attachments/ 目录并在数据库中创建记录。
 *
 * @param {string} paperId - 论文 ID
 * @param {File[]} files - 要上传的文件数组（File 对象）
 * @returns {Promise<UploadAttachmentsResponse>} 新创建的附件列表
 */
export async function uploadAttachments(paperId: string, files: File[]): Promise<UploadAttachmentsResponse> {
  const formData = new FormData();
  files.forEach((file) => {
    formData.append('files', file);
  });
  const result = await request<UploadAttachmentsResponse>(`/papers/${paperId}/attachments`, {
    method: 'POST',
    body: formData,
    headers: {},
  });
  invalidateCache(`/papers/${paperId}/attachments`);
  return result;
}

/**
 * 获取附件文件的下载/预览 URL（同步函数）
 *
 * @param {string} attachmentId - 附件 ID
 * @returns {string} 附件文件的 URL 字符串，如 "/api/attachments/123"
 */
export function getAttachmentUrl(attachmentId: string): string {
  return `/api/attachments/${attachmentId}`;
}

/**
 * 删除单个附件
 *
 * 同时删除数据库记录和磁盘上的物理文件。
 * 删除后需要刷新附件列表以更新 UI。
 *
 * @param {string} attachmentId - 附件 ID
 * @returns {Promise<DeleteAttachmentResponse>} 操作结果消息
 */
export async function deleteAttachment(attachmentId: string): Promise<DeleteAttachmentResponse> {
  const result = await request<DeleteAttachmentResponse>(`/attachments/${attachmentId}`, {
    method: 'DELETE',
  });
  invalidateCache('/papers');
  return result;
}

/**
 * 将指定附件设为论文的主 PDF
 *
 * 将该附件的 is_primary 设为 1（主 PDF），同时将论文当前的主 PDF 降级为普通附件。
 * 操作完成后后端会自动触发 PDF 重新解析任务。
 *
 * @param {string} attachmentId - 要设为主 PDF 的附件 ID
 * @returns {Promise<SetPrimaryAttachmentResponse>} 操作结果消息
 */
export async function setPrimaryAttachment(attachmentId: string): Promise<SetPrimaryAttachmentResponse> {
  const result = await request<SetPrimaryAttachmentResponse>(`/attachments/${attachmentId}/set-primary`, {
    method: 'POST',
  });
  invalidateCache('/papers');
  return result;
}
