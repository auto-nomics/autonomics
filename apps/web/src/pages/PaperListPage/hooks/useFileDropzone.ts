/**
 * useFileDropzone - 全局文件拖拽导入钩子
 *
 * 监听 document 级别的拖拽事件，当用户从文件管理器或 Chrome 下载气泡
 * 将文件拖入浏览器窗口时，显示拖拽覆盖层并处理文件导入。
 *
 * 功能：
 * - 检测外部文件拖入（区分内部表格行拖拽）
 * - 根据文件扩展名自动路由到题录导入或 PDF 上传
 * - 支持多文件同时拖入
 * - 拖拽过程中阻止浏览器默认行为（防止导航到文件）
 * - 看门狗定时器：dragover 停止 500ms 后自动隐藏覆盖层
 *   （解决拖拽中断时 dragleave 不触发导致覆盖层卡住的问题）
 * - ESC 键取消拖拽
 *
 * 为什么使用计数器而非布尔值？
 * - dragenter/dragleave 事件在进入/离开子元素时都会触发
 * - 计数器跟踪嵌套的进入/离开次数，确保覆盖层正确显示/隐藏
 *
 * 为什么需要看门狗定时器？
 * - 当用户将文件拖出浏览器窗口后松手，浏览器可能不触发 dragleave
 * - 此时计数器无法归零，覆盖层永远不消失
 * - dragover 在拖拽期间持续触发（约每 350ms），停止后即说明拖拽已结束
 *
 * @param {Object} options
 * @param {Function} options.onImportBibliography - 题录导入回调 (file, categoryId) => Promise
 * @param {Function} options.onUploadToCategory - PDF 上传回调 (file, categoryId) => Promise
 * @param {Function} [options.onRefreshCategories] - 导入后刷新分类计数的回调
 * @returns {{ isDragging: boolean }} 拖拽状态
 */

import { useState, useCallback, useEffect, useRef } from 'react';
import { App } from 'antd';

// 题录文件扩展名列表
const BIB_EXTENSIONS = ['.bib', '.ris', '.nbib', '.enw', '.xml', '.txt'];

// 看门狗超时时间（毫秒）：dragover 停止超过此时间即判定拖拽已结束
const WATCHDOG_TIMEOUT = 500;

/**
 * 获取文件扩展名（小写）
 * @param {string} filename
 * @returns {string} 扩展名，如 '.nbib'；无扩展名返回空字符串
 */
function getFileExtension(filename: string) {
  const dotIndex = filename.lastIndexOf('.');
  return dotIndex !== -1 ? filename.slice(dotIndex).toLowerCase() : '';
}

/**
 * 判断拖拽事件是否携带外部文件
 * 内部表格行拖拽使用 'application/jayread-paper' 类型，不含 'Files'
 * @param {DragEvent} e
 * @returns {boolean}
 */
function isFileDrag(e: any) {
  return e.dataTransfer?.types?.includes('Files');
}

export function useFileDropzone({ onImportBibliography, onUploadToCategory, onRefreshCategories }: { onImportBibliography?: any; onUploadToCategory?: any; onRefreshCategories?: any }) {
  const { message } = App.useApp();
  const [isDragging, setIsDragging] = useState(false);

  // 嵌套 dragenter/dragleave 计数器
  const dragCounterRef = useRef(0);
  // 看门狗定时器 ID
  const watchdogRef = useRef<any>(null);

  /** 强制重置拖拽状态（看门狗 / ESC 触发时调用） */
  const forceReset = useCallback(() => {
    dragCounterRef.current = 0;
    setIsDragging(false);
    if (watchdogRef.current) {
      clearTimeout(watchdogRef.current);
      watchdogRef.current = null;
    }
  }, []);

  /** 重置看门狗定时器（每次 dragover 时调用） */
  const resetWatchdog = useCallback(() => {
    if (watchdogRef.current) clearTimeout(watchdogRef.current);
    watchdogRef.current = setTimeout(forceReset, WATCHDOG_TIMEOUT);
  }, [forceReset]);

  const handleDragEnter = useCallback((e: any) => {
    e.preventDefault();
    if (!isFileDrag(e)) return;

    dragCounterRef.current += 1;
    if (dragCounterRef.current === 1) {
      setIsDragging(true);
    }
    resetWatchdog();
  }, [resetWatchdog]);

  const handleDragLeave = useCallback((e: any) => {
    e.preventDefault();
    if (!isFileDrag(e)) return;

    dragCounterRef.current = Math.max(0, dragCounterRef.current - 1);
    if (dragCounterRef.current === 0) {
      if (watchdogRef.current) {
        clearTimeout(watchdogRef.current);
        watchdogRef.current = null;
      }
      setIsDragging(false);
    }
  }, []);

  const handleDragOver = useCallback((e: any) => {
    e.preventDefault();
    if (isFileDrag(e)) {
      e.dataTransfer.dropEffect = 'copy';
    }
    // 拖拽期间 dragover 持续触发，每次重置看门狗
    resetWatchdog();
  }, [resetWatchdog]);

  const handleDrop = useCallback(async (e: any) => {
    e.preventDefault();
    e.stopPropagation();

    // 重置状态
    forceReset();

    const files = Array.from(e.dataTransfer?.files || []);
    if (files.length === 0) return;

    let importedCount = 0;

    for (const file of files as File[]) {
      const ext = getFileExtension(file.name);

      if (ext === '.pdf') {
        try {
          await onUploadToCategory?.(file, null);
          importedCount += 1;
        } catch (err) {
          // 错误已在 hook 内部处理
        }
      } else if (BIB_EXTENSIONS.includes(ext)) {
        try {
          await onImportBibliography?.(file, null);
          importedCount += 1;
        } catch (err) {
          // 错误已在 hook 内部处理
        }
      } else {
        message.warning(`不支持的文件格式: ${file.name}`);
      }
    }

    // 导入完成后刷新分类计数
    if (importedCount > 0) {
      onRefreshCategories?.();
    }
  }, [onImportBibliography, onUploadToCategory, onRefreshCategories, forceReset]);

  // 注册拖拽事件监听器
  useEffect(() => {
    document.addEventListener('dragenter', handleDragEnter);
    document.addEventListener('dragleave', handleDragLeave);
    document.addEventListener('dragover', handleDragOver);
    document.addEventListener('drop', handleDrop);

    return () => {
      document.removeEventListener('dragenter', handleDragEnter);
      document.removeEventListener('dragleave', handleDragLeave);
      document.removeEventListener('dragover', handleDragOver);
      document.removeEventListener('drop', handleDrop);
    };
  }, [handleDragEnter, handleDragLeave, handleDragOver, handleDrop]);

  // ESC 键取消拖拽
  useEffect(() => {
    const handleKeyDown = (e: any) => {
      if (e.key === 'Escape' && dragCounterRef.current > 0) {
        forceReset();
      }
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [forceReset]);

  // 组件卸载时清理看门狗定时器
  useEffect(() => {
    return () => {
      if (watchdogRef.current) clearTimeout(watchdogRef.current);
    };
  }, []);

  return { isDragging };
}
