/**
 * JayRead 题录导入 API 客户端
 *
 * 提供与后端 /api/bibliography 端点通信的函数：
 * - importBibliography: 导入题录文件（BibTeX/RIS/NBIB/EndNote/XML/GB-T 7714）
 * - resolveDuplicates: 处理重复文献的去重选择
 * - attachPdf: 为元数据记录手动上传 PDF
 * - downloadPdf: 通过 DOI 自动下载开放获取 PDF
 */

import request, { invalidateCache } from './client'
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
 * 将用户上传的题录文件发送到后端进行解析和导入。
 * 后端会自动检测文件格式、解析文献记录、检测重复，并创建论文元数据记录。
 *
 * @param {File} file - 用户上传的题录文件对象（.bib/.ris/.nbib/.enw/.xml/.txt）
 * @param {string|null} [categoryId=null] - 可选的分类 ID，导入的论文将归入该分类
 * @param {string|null} [format=null] - 可选的格式提示，手动指定解析格式
 * @returns {Promise<ImportResult>} 导入结果对象
 */
export async function importBibliography(file: File, categoryId: string | null = null, format: string | null = null): Promise<ImportResult> {
  const formData = new FormData()
  formData.append('file', file)

  if (categoryId !== null) {
    formData.append('category_id', categoryId)
  }

  if (format) {
    formData.append('format', format)
  }

  const result = await request<ImportResult>('/bibliography/import', {
    method: 'POST',
    body: formData,
    headers: {},
  })
  invalidateCache('/papers')
  return result
}

/**
 * 处理重复文献的去重选择
 *
 * 当导入题录文件检测到重复时，前端将用户的去重选择发送到此函数。
 * 每个重复记录有三种处理方式：跳过（skip）、覆盖（replace）、保留两条（keep_both）。
 *
 * @param {Array<DuplicateResolution>} resolutions - 去重解决方案列表
 * @param {string|null} [categoryId=null] - 可选的分类 ID，新创建的记录将归入该分类
 * @returns {Promise<ResolveDuplicatesResult>} 处理结果
 */
export async function resolveDuplicates(resolutions: DuplicateResolution[], categoryId: string | null = null): Promise<ResolveDuplicatesResult> {
  const result = await request<ResolveDuplicatesResult>('/bibliography/resolve-duplicates', {
    method: 'POST',
    body: {
      resolutions,
      category_id: categoryId,
    },
  })
  invalidateCache('/papers')
  return result
}

/**
 * 为元数据记录手动上传 PDF 文件
 *
 * 当论文记录是通过题录导入创建的（没有 PDF），用户可以后续上传 PDF 文件。
 * 上传成功后，后端会自动触发 PDF 解析和元数据获取的后台任务。
 *
 * @param {string} paperId - 论文 ID
 * @param {File} file - 用户上传的附件文件对象
 * @returns {Promise<AttachPdfResponse>} 操作结果
 */
export async function attachPdf(paperId: string, file: File): Promise<AttachPdfResponse> {
  const formData = new FormData()
  formData.append('file', file)

  const result = await request<AttachPdfResponse>(`/bibliography/${paperId}/attach-pdf`, {
    method: 'POST',
    body: formData,
    headers: {},
  })
  invalidateCache('/papers')
  return result
}

/**
 * 通过 DOI 自动下载开放获取 PDF
 *
 * 使用 Unpaywall API 查询论文的开放获取 PDF 链接并自动下载。
 * 约覆盖 50% 的 DOI 文献（取决于论文是否开放获取）。
 *
 * @param {string} paperId - 论文 ID（该论文必须有 DOI）
 * @returns {Promise<DownloadPdfResponse>} 下载结果
 */
export async function downloadPdf(paperId: string): Promise<DownloadPdfResponse> {
  const result = await request<DownloadPdfResponse>(`/bibliography/${paperId}/download-pdf`, {
    method: 'POST',
  })
  invalidateCache('/papers')
  return result
}
