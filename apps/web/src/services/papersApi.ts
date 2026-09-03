/**
 * JayRead 论文相关 API 客户端
 *
 * 封装论文 CRUD、PDF 获取、Markdown 解析、重解析、实时状态推送、元数据获取等接口。
 */
import request, { invalidateCache } from './client';
import type {
  UploadPaperResponse,
  GetPapersParams,
  GetPapersResponse,
  GetPaperResponse,
  GetMarkdownResponse,
  ReparsePaperResponse,
  DeletePaperResponse,
  FetchMetadataResponse,
  MetadataPendingCheck,
  GetPaperTypesResponse,
  TranslatePaperResponse,
  TranslationStatus,
  DownloadPdfResponse,
  Paper,
} from '@/types';

/**
 * 上传 PDF 文件创建论文记录
 *
 * @param {File} file - PDF 文件对象
 * @param {string|null} [categoryId=null] - 可选的分类 ID，上传后论文将归入该分类
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<UploadPaperResponse>} 上传结果，包含论文 ID
 */
export async function uploadPaper(file: File, categoryId: string | null = null, signal?: AbortSignal): Promise<UploadPaperResponse> {
  const formData = new FormData();
  formData.append('file', file);
  if (categoryId !== null) {
    formData.append('category_id', categoryId);
  }
  const result = await request<UploadPaperResponse>('/papers/upload', {
    method: 'POST',
    body: formData,
    headers: {},
    signal,
    timeout: 120000, // 上传可能需要较长时间
  });
  invalidateCache('/papers');
  return result;
}

/**
 * 获取论文列表
 *
 * @param {Object} [params={}] - 查询参数
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetPapersResponse>} 论文列表
 */
export async function getPapers(params: GetPapersParams = {}, signal?: AbortSignal): Promise<GetPapersResponse> {
  const query = new URLSearchParams(params as Record<string, string>).toString();
  return request<GetPapersResponse>(`/papers${query ? '?' + query : ''}`, { signal });
}

/**
 * 获取论文详情
 *
 * @param {string} id - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetPaperResponse>} 论文详细信息
 */
export async function getPaper(id: string, signal?: AbortSignal): Promise<GetPaperResponse> {
  return request<GetPaperResponse>(`/papers/${id}`, { signal });
}

/**
 * PATCH 风格更新论文
 *
 * 只传需要改的字段，后端 `UpdateItem` 全是 `Option`。
 * 用于切换悬浮翻译开关等局部更新。
 *
 * @param {string} id - 论文 ID
 * @param {Partial<Pick<Paper, 'hoverTranslationEnabled'>>} patch - 待更新字段
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<Paper>} 更新后的论文对象
 */
export async function updatePaper(
  id: string,
  patch: Partial<Pick<Paper, 'hoverTranslationEnabled'>>,
  signal?: AbortSignal,
): Promise<Paper> {
  const result = await request<Paper>(`/papers/${id}`, {
    method: 'PUT',
    body: patch,
    signal,
  });
  invalidateCache(`/papers/${id}`);
  invalidateCache('/papers');
  return result;
}

/**
 * 获取论文 PDF 的下载/预览 URL
 *
 * @param {string} id - 论文 ID
 * @returns {string} PDF 文件的 API URL
 */
export function getPdfUrl(id: string): string {
  return `/api/papers/${id}/pdf`;
}

/**
 * 获取论文解析后的 Markdown 内容
 *
 * @param {string} id - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetMarkdownResponse>} Markdown 内容
 */
export async function getMarkdown(id: string, signal?: AbortSignal): Promise<GetMarkdownResponse> {
  return request<GetMarkdownResponse>(`/papers/${id}/markdown`, { signal });
}

/**
 * 重新解析论文
 *
 * 当论文 PDF 更新或解析结果有问题时，可调用此接口重新解析。
 * 会触发后端重新运行 PDF 解析器（如 MinerU）提取内容。
 *
 * @param {string} id - 论文 ID
 * @param {string} [engine='mineru'] - 解析引擎，默认为 MinerU
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<ReparsePaperResponse>} 重新解析结果
 */
export async function reparsePaper(id: string, engine: string = 'mineru', signal?: AbortSignal): Promise<ReparsePaperResponse> {
  const result = await request<ReparsePaperResponse>(`/papers/${id}/reparse`, {
    method: 'POST',
    body: { engine },
    signal,
  });
  invalidateCache(`/papers/${id}`);
  return result;
}

/**
 * 删除论文
 *
 * @param {string} id - 论文 ID
 * @returns {Promise<DeletePaperResponse>} 删除确认消息
 */
export async function deletePaper(id: string): Promise<DeletePaperResponse> {
  const result = await request<DeletePaperResponse>(`/papers/${id}`, { method: 'DELETE' });
  invalidateCache('/papers');
  return result;
}

/**
 * 获取论文 SSE 实时状态推送端点 URL
 *
 * @param {string} id - 论文 ID
 * @returns {string} SSE 端点的完整 URL，用于 new EventSource()
 */
export function getStreamUrl(id: string): string {
  return `/api/papers/${id}/stream`;
}

/**
 * 手动触发期刊元数据获取
 *
 * 从期刊官网或 DOI 服务获取论文元数据（作者、期刊名、影响因子等）。
 *
 * @param {string} id - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<FetchMetadataResponse>} 元数据获取结果
 */
export async function fetchMetadata(id: string, signal?: AbortSignal): Promise<FetchMetadataResponse> {
  return request<FetchMetadataResponse>(`/metadata/${id}/fetch`, {
    method: 'POST',
    signal,
  });
}

/**
 * 检查是否有论文正在等待元数据获取
 *
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<MetadataPendingCheck>} 待处理状态
 */
export async function checkMetadataPending(signal?: AbortSignal): Promise<MetadataPendingCheck> {
  return request<MetadataPendingCheck>('/metadata/pending', { signal });
}

/**
 * 获取系统支持的文献类型列表
 *
 * @returns {Promise<GetPaperTypesResponse>} 支持的文献类型数组
 */
export const getPaperTypes = async (): Promise<GetPaperTypesResponse> => {
  return request<GetPaperTypesResponse>('/papers/types');
};

/**
 * 翻译论文标题和摘要
 *
 * @param {string} id - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<TranslatePaperResponse>} 翻译结果
 */
export async function translatePaper(id: string, signal?: AbortSignal): Promise<TranslatePaperResponse> {
  const result = await request<TranslatePaperResponse>(`/papers/${id}/translate`, {
    method: 'POST',
    signal,
  });
  invalidateCache(`/papers/${id}`);
  return result;
}

/**
 * 查询论文翻译状态
 *
 * @param {string} id - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<TranslationStatus>} 翻译状态
 */
export async function getTranslationStatus(id: string, signal?: AbortSignal): Promise<TranslationStatus> {
  return request<TranslationStatus>(`/papers/${id}/translate`, {
    method: 'GET',
    signal,
  });
}

/**
 * 通过 DOI 自动下载开放获取 PDF
 *
 * 使用 Unpaywall API 查询论文的开放获取 PDF 链接并下载。
 *
 * @param {string} paperId - 论文 ID（必须有 DOI）
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<DownloadPdfResponse>} 下载结果
 */
export async function downloadPdf(paperId: string, signal?: AbortSignal): Promise<DownloadPdfResponse> {
  const result = await request<DownloadPdfResponse>(`/bibliography/${paperId}/download-pdf`, {
    method: 'POST',
    signal,
    timeout: 60000,
  });
  invalidateCache('/papers');
  return result;
}
