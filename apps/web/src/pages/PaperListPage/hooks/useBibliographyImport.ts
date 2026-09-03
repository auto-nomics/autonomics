/**
 * useBibliographyImport - 题录导入钩子
 *
 * 本文件封装了题录文件导入功能，包括：
 * - 处理题录文件上传（BibTeX / RIS / CSL-JSON；重复条目由 autonomics
 *   服务端按标识符 upsert 合并，前端没有去重解决步骤）
 * - 为元数据记录手动上传 PDF 文件
 * - 通过 DOI 自动下载开放获取 PDF（服务端无此管线，桩化）
 *
 * 工作流程：
 * 1. 用户选择题录文件
 * 2. 服务端解析、去重合并、可选归类，一次性返回结果
 * 3. 前端汇总 toast 并刷新论文列表
 */

import { useState, useCallback } from 'react'; // 导入 React 核心钩子
import { useTranslation } from 'react-i18next'; // 导入 i18n 翻译钩子
import { App } from 'antd'; // 导入 Ant Design App 组件
import type { ImportResult } from '@/types';
import {
  importBibliography,    // 导入题录文件（BibTeX/RIS/CSL-JSON）
  attachPdf,             // 为元数据记录手动上传 PDF
  downloadPdf,           // 通过 DOI 自动下载开放获取 PDF（桩，UI 入口待清扫）
} from '../../../services/bibliographyApi'; // 导入题录导入 API 函数

/**
 * 题录导入钩子
 *
 * @param {Function} loadPapers - 刷新论文列表的回调函数
 * @param {Function} startMetadataPolling - （保留参数兼容调用方）autonomics 无异步元数据获取，不再使用
 * @returns {Object} 返回状态和处理函数的对象
 */
export function useBibliographyImport(loadPapers: () => void, _startMetadataPolling: (ids: string[]) => void) {
  const { t } = useTranslation('paperList');
  const { message } = App.useApp();
  // ========== 状态定义 ==========

  // 导入中加载状态：控制导入过程中显示加载动画，防止重复提交
  const [importingBib, setImportingBib] = useState(false); // 题录导入中状态，默认 false

  // 正在下载 PDF 的论文 ID 集合：用于控制"自动下载PDF"按钮的加载状态
  const [downloadingPdfIds, setDownloadingPdfIds] = useState<Set<string>>(new Set()); // 正在下载的论文 ID 集合

  // 正在上传 PDF 的论文 ID 集合：用于控制"上传PDF"按钮的加载状态
  const [uploadingPdfIds, setUploadingPdfIds] = useState<Set<string>>(new Set()); // 正在上传的论文 ID 集合

  // ========== 处理函数 ==========

  /**
   * 处理题录文件导入
   *
   * 当用户在 CategorySidebar 右键菜单中点击"导入题录"并选择题录文件后触发。
   * 工作流程：
   * 1. 设置加载状态
   * 2. 调用后端导入 API 解析题录文件
   * 3. 如果有重复记录，弹出重复解决对话框
   * 4. 如果没有重复，直接显示成功通知并刷新列表
   *
   * @param {File} file - 用户选择的题录文件
   * @param {number|null} categoryId - 目标分类 ID
   */
  const handleImportBibliography = useCallback(async (file: File, categoryId: number | null) => {
    setImportingBib(true); // 设置导入中状态

    try {
      // 调用题录导入 API，发送文件到后端解析。autonomics 在服务端按标识符
      // upsert 合并重复条目，**没有前端去重解决步骤**——重复数只体现在 toast
      const result = await importBibliography(file, categoryId as any);

      // 汇总提示：导入 N / 合并 M 条重复 / 失败 K（有失败时降级为 warning）
      const parts: string[] = [t('message.importSuccess', { count: result.imported })];
      if (result.duplicates > 0) {
        parts.push(`合并重复 ${result.duplicates} 条`);
      }
      if (result.failed > 0) {
        parts.push(`失败 ${result.failed} 条`);
      }
      if (result.failed > 0) {
        message.warning(parts.join('，'));
      } else {
        message.success(parts.join('，'));
      }

      // 刷新论文列表
      loadPapers(); // 调用父组件传入的刷新函数
    } catch (err: any) {
      // 导入失败，显示错误提示
      message.error(t('message.importFailed', { error: err.message }));
    } finally {
      // 无论成功失败，都重置加载状态
      setImportingBib(false);
    }
  }, [loadPapers, message, t]); // 依赖项：刷新列表与 toast

  /**
   * 为元数据记录手动上传 PDF 文件
   *
   * 题录导入的论文记录没有 PDF，用户可以点击"上传PDF"按钮
   * 选择本地 PDF 文件进行关联。上传成功后会自动触发 PDF 解析。
   *
   * @param {number} paperId - 论文 ID
   * @param {File} file - PDF 文件
   * @param {Function} listenToParseStatus - SSE 监听函数，上传成功后开始监听解析状态
   */
  const handleAttachPdf = useCallback(async (paperId: string, file: File, listenToParseStatus?: (paperId: string) => void) => {
    // 添加到上传中集合，显示加载状态
    setUploadingPdfIds(prev => new Set(prev).add(String(paperId)));

    try {
      // 调用后端 PDF 关联 API
      const result = await attachPdf(paperId, file);
      message.success(result.message || t('message.pdfUploadSuccess'));
      // 开始监听解析状态
      listenToParseStatus?.(paperId); // 如果提供了监听函数，开始监听
      // 刷新论文列表以获取最新状态
      loadPapers(); // 刷新列表
    } catch (err: any) {
      message.error(t('message.pdfUploadFailed', { error: err.message }));
    } finally {
      // 从上传中集合移除
      setUploadingPdfIds(prev => {
        const next = new Set(prev);
        next.delete(String(paperId));
        return next;
      });
    }
  }, [loadPapers]); // 依赖项：loadPapers

  /**
   * 通过 DOI 自动下载开放获取 PDF
   *
   * 使用 Unpaywall API 查询论文的开放获取 PDF 链接并自动下载。
   * 如果论文没有 DOI 或没有开放获取版本，下载会失败。
   *
   * @param {number} paperId - 论文 ID
   * @param {Function} listenToParseStatus - SSE 监听函数，下载成功后开始监听解析状态
   */
  const handleDownloadPdf = useCallback(async (paperId: string, listenToParseStatus?: (paperId: string) => void) => {
    // 添加到下载中集合，显示加载状态
    setDownloadingPdfIds(prev => new Set(prev).add(String(paperId)));

    try {
      // 调用后端自动下载 PDF API
      const result = await downloadPdf(paperId);
      if (result.success) {
        // 下载成功
        message.success(result.message || t('message.pdfDownloadSuccess'));
        // 开始监听解析状态
        listenToParseStatus?.(paperId); // 如果提供了监听函数，开始监听
        // 刷新论文列表
        loadPapers(); // 刷新列表
      } else {
        // 下载失败（没有开放获取 PDF）
        message.warning(result.message || t('message.pdfNotFound'));
      }
    } catch (err: any) {
      message.error(t('message.pdfDownloadFailed', { error: err.message }));
    } finally {
      // 从下载中集合移除
      setDownloadingPdfIds(prev => {
        const next = new Set(prev);
        next.delete(String(paperId));
        return next;
      });
    }
  }, [loadPapers]); // 依赖项：loadPapers

  // 返回公共接口
  return {
    // 状态
    importingBib,           // 题录导入中状态
    downloadingPdfIds,      // 正在下载 PDF 的论文 ID 集合
    uploadingPdfIds,        // 正在上传 PDF 的论文 ID 集合

    // 题录导入相关
    handleImportBibliography,      // 处理题录文件导入

    // PDF 操作相关
    handleAttachPdf,         // 为元数据记录手动上传 PDF
    handleDownloadPdf,       // 通过 DOI 自动下载 PDF
  };
}

/**
 * 导出类型定义（用于 TypeScript 项目）
 * 本项目使用 JavaScript，保留类型注释供参考
 */
/**
 * @typedef {Object} DuplicateResolution
 * @property {string} id - 重复记录的唯一标识（同后端 DuplicateItem.id）
 * @property {'skip'|'replace'|'keep_both'} action - 处理方式
 * @property {string} [existing_item_id] - 已有论文 ID（replace 必填）
 * @property {Object} [record] - 原始题录记录（replace / keep_both 必填，由 import 响应原样回传）
 */
