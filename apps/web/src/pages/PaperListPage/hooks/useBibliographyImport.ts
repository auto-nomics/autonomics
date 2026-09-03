/**
 * useBibliographyImport - 题录导入和去重钩子
 *
 * 本文件封装了题录文件导入功能，包括：
 * - 处理题录文件上传（BibTeX/RIS/NBIB 等格式）
 * - 检测并处理重复文献
 * - 为元数据记录手动上传 PDF 文件
 * - 通过 DOI 自动下载开放获取 PDF
 *
 * 支持的题录格式：
 * - BibTeX (.bib): LaTeX 常用文献格式
 * - RIS (.ris): EndNote/Reference Manager 导出格式
 * - NBIB (.nbib): PubMed/医学数据库导出格式
 * - CSV (.csv): 简单逗号分隔值格式
 *
 * 工作流程：
 * 1. 用户选择题录文件
 * 2. 后端解析文件并提取文献元数据
 * 3. 后端检测重复文献（基于 DOI 或标题相似度）
 * 4. 如有重复，弹出对话框让用户选择处理方式
 * 5. 处理完成后刷新论文列表
 */

import { useState, useCallback } from 'react'; // 导入 React 核心钩子
import { useTranslation } from 'react-i18next'; // 导入 i18n 翻译钩子
import { App } from 'antd'; // 导入 Ant Design App 组件
import type { DuplicateDetail, DuplicateResolution, ImportResult } from '@/types';
import {
  importBibliography,    // 导入题录文件（BibTeX/RIS/NBIB 等）
  resolveDuplicates,     // 处理重复文献的去重选择
  attachPdf,             // 为元数据记录手动上传 PDF
  downloadPdf,           // 通过 DOI 自动下载开放获取 PDF
} from '../../../services/bibliographyApi'; // 导入题录导入 API 函数
import { getPaperCategories } from '../../../services/categoriesApi'; // 导入分类 API
import NiceModal from '@ebay/nice-modal-react';

/**
 * 题录导入钩子
 *
 * @param {Function} loadPapers - 刷新论文列表的回调函数
 * @param {Function} startMetadataPolling - 启动元数据获取轮询的回调函数
 * @returns {Object} 返回状态和处理函数的对象
 */
export function useBibliographyImport(loadPapers: () => void, startMetadataPolling: (ids: string[]) => void) {
  const { t } = useTranslation('paperList');
  const { message } = App.useApp();
  // ========== 状态定义 ==========

  // 导入中加载状态：控制导入过程中显示加载动画，防止重复提交
  const [importingBib, setImportingBib] = useState(false); // 题录导入中状态，默认 false

  // 重复文献详情列表：存储后端返回的重复记录信息
  const [duplicateData, setDuplicateData] = useState<DuplicateDetail[]>([]); // 重复记录数据

  // 当前导入操作的待定分类 ID：用于在解决重复后将新记录归入用户指定的分类
  const [pendingCategoryId, setPendingCategoryId] = useState<string | number | null>(null); // 待定分类 ID

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
    setPendingCategoryId(categoryId); // 记录目标分类 ID

    try {
      // 调用题录导入 API，发送文件到后端进行解析
      const result = await importBibliography(file, categoryId as any);

      if (result.duplicates > 0) {
        // 如果检测到重复文献，弹出重复解决对话框
        setDuplicateData(result.duplicate_details); // 存储重复详情
        // 通过 NiceModal 命令式调用弹窗，await 用户选择结果
        const resolutions = await NiceModal.show<DuplicateResolution[] | null>('duplicate-resolution', {
          duplicates: result.duplicate_details,
        });
        if (resolutions) {
          await handleResolveDuplicates(resolutions);
        } else {
          handleCancelDuplicates();
        }
        // 先将成功导入的论文添加到列表中（重复的等用户选择后再处理）
        if (result.imported_paper_ids.length > 0) {
          message.success(t('message.importWithDuplicates', { imported: result.imported, duplicates: result.duplicates }));
        }
        // 刷新论文列表
        loadPapers(); // 调用父组件传入的刷新函数
        // 为已导入的论文启动元数据获取轮询
        // 后台正在异步获取 IF、被引、分区等信息，轮询检测完成后自动刷新列表
        if (result.imported_paper_ids?.length > 0) {
          startMetadataPolling(result.imported_paper_ids);
        }
      } else {
        // 没有重复，直接导入完成
        message.success(t('message.importSuccess', { count: result.imported }));
        loadPapers(); // 刷新论文列表
        // 为导入的论文启动元数据获取轮询
        if (result.imported_paper_ids?.length > 0) {
          startMetadataPolling(result.imported_paper_ids);
        }
      }
    } catch (err: any) {
      // 导入失败，显示错误提示
      message.error(t('message.importFailed', { error: err.message }));
    } finally {
      // 无论成功失败，都重置加载状态
      setImportingBib(false);
    }
  }, [loadPapers, startMetadataPolling]); // 依赖项：loadPapers 用于刷新列表，startMetadataPolling 用于轮询元数据状态

  /**
   * 处理重复文献的去重选择确认
   *
   * 用户在重复解决对话框中为每条重复记录选择操作后，点击确认按钮触发。
   * 将用户的选择发送到后端进行处理。
   *
   * 支持的处理方式：
   * - skip: 跳过该记录（不导入）
   * - replace: 用新记录的非空字段覆盖现有记录
   * - keep_both: 把新记录作为独立论文入库
   *
   * @param {Array} resolutions - 去重解决方案列表
   *   每个元素结构：{ id: string, action, existing_item_id?, record? }
   */
  const handleResolveDuplicates = useCallback(async (resolutions: DuplicateResolution[]) => {
    try {
      // 调用后端去重解决 API
      const result = await resolveDuplicates(resolutions, pendingCategoryId as any);

      // 显示处理结果通知
      message.success(
        t('message.duplicateResolved', { created: result.created, skipped: result.skipped, updated: result.updated })
      );

      // 清空重复数据
      setDuplicateData([]); // 清空重复数据

      // 刷新论文列表
      loadPapers(); // 调用父组件传入的刷新函数
      // 为去重后新创建的论文启动元数据获取轮询
      // result.paper_ids 包含 keep_both 操作创建的新论文 ID
      if (result.paper_ids?.length > 0) {
        startMetadataPolling(result.paper_ids);
      }
    } catch (err: any) {
      message.error(t('message.duplicateResolveFailed', { error: err.message }));
    }
  }, [pendingCategoryId, loadPapers, startMetadataPolling]); // 依赖项：pendingCategoryId、loadPapers 和 startMetadataPolling

  /**
   * 取消重复文献解决
   * 关闭对话框并清空数据
   */
  const handleCancelDuplicates = useCallback(() => {
    setDuplicateData([]); // 清空重复数据
    setPendingCategoryId(null); // 清空待定分类
  }, []);

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
    duplicateData,          // 重复记录详情
    pendingCategoryId,      // 待定分类 ID
    downloadingPdfIds,      // 正在下载 PDF 的论文 ID 集合
    uploadingPdfIds,        // 正在上传 PDF 的论文 ID 集合

    // 题录导入相关
    handleImportBibliography,      // 处理题录文件导入
    handleResolveDuplicates,       // 处理重复文献去重选择
    handleCancelDuplicates,        // 取消重复文献解决

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
