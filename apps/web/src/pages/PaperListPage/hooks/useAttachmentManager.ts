/**
 * useAttachmentManager - 附件管理 Hook
 *
 * 管理论文附件的加载、缓存、上传、删除、设为主 PDF、打开等操作。
 * 同时处理展开行状态和拖拽文件上传功能。
 *
 * 从 PaperListPage 中提取，封装所有附件相关的状态和回调。
 */
import { useState, useCallback, useRef, useEffect } from 'react';
import { App } from 'antd';
import { useTranslation } from 'react-i18next';
import type { Attachment } from '@/types';
import {
  getAttachments,
  uploadAttachments,
  deleteAttachment,
  setPrimaryAttachment,
} from '../../../services/attachmentsApi';

/** useAttachmentManager 参数 */
interface UseAttachmentManagerParams {
  navigate: (path: string) => void;
  tableWrapperRef: React.RefObject<HTMLElement | null>;
  onSetPrimary?: () => Promise<void>;
}

export function useAttachmentManager({ navigate, tableWrapperRef, onSetPrimary }: UseAttachmentManagerParams) {
  const { message } = App.useApp();
  const { t } = useTranslation('paperList');
  const [expandedRowKeys, setExpandedRowKeys] = useState<string[]>([]);
  const [attachmentCache, setAttachmentCache] = useState<Record<string, Attachment[]>>({});
  const [attachmentLoading, setAttachmentLoading] = useState<Record<string, boolean>>({});
  const attachmentCacheRef = useRef<Record<string, Attachment[]>>({});

  // 加载指定论文的附件列表
  const loadAttachments = useCallback(async (paperId: string) => {
    setAttachmentLoading(prev => ({ ...prev, [paperId]: true }));
    try {
      const data = await getAttachments(paperId);
      const attachments = data.attachments || [];
      setAttachmentCache(prev => {
        const next = { ...prev, [paperId]: attachments };
        attachmentCacheRef.current = next;
        return next;
      });
      // 加载完后若该论文没有附件，主动从 expandedRowKeys 移除。
      // antd Table 只要 key 在 expandedRowKeys 里就会渲染展开行 <tr><td>，
      // <td> 自带 padding 会撑出空白条——光让 AttachmentList 返回 null 治不了本。
      if (attachments.length === 0) {
        setExpandedRowKeys(prev => prev.filter(id => id !== paperId));
      }
    } catch (err) {
      console.error(`Failed to load attachments for paper ${paperId}:`, err);
      setAttachmentCache(prev => {
        const next = { ...prev, [paperId]: [] };
        attachmentCacheRef.current = next;
        return next;
      });
      setExpandedRowKeys(prev => prev.filter(id => id !== paperId));
    } finally {
      setAttachmentLoading(prev => ({ ...prev, [paperId]: false }));
    }
  }, []);

  // 处理附件上传
  const handleUploadAttachments = useCallback(async (paperId: string | number, files: File[]) => {
    const pid = String(paperId);
    try {
      await uploadAttachments(pid, files);
      await loadAttachments(pid);
      message.success(t('message.attachmentUploaded', { count: files.length }));
    } catch (err) {
      message.error(t('message.uploadAttachmentFailed', { error: (err as any).message }));
    }
  }, [loadAttachments]);

  // 处理附件删除
  const handleDeleteAttachment = useCallback(async (paperId: string, attachmentId: string) => {
    try {
      await deleteAttachment(attachmentId);
      await loadAttachments(paperId);
      message.success(t('message.attachmentDeleted'));
    } catch (err) {
      message.error(t('message.deleteAttachmentFailed', { error: (err as any).message }));
    }
  }, [loadAttachments]);

  // 设置主 PDF
  const handleSetPrimary = useCallback(async (paperId: string, attachmentId: string) => {
    try {
      await setPrimaryAttachment(attachmentId);
      await loadAttachments(paperId);
      // 通知父组件刷新论文列表（更新 parse_status 等状态）
      if (onSetPrimary) await onSetPrimary();
      message.success(t('message.switchedPdf'));
    } catch (err) {
      message.error(t('message.switchPrimaryFailed', { error: (err as any).message }));
    }
  }, [loadAttachments, onSetPrimary]);

  // 打开附件
  const handleOpenAttachment = useCallback((paperId: string, attachmentId: string) => {
    navigate(`/paper/${paperId}?attachmentId=${attachmentId}`);
  }, [navigate]);

  // 处理行展开/折叠
  const handleExpand = useCallback(async (expanded: boolean, paper: { id: string }) => {
    if (expanded) {
      const cached = attachmentCacheRef.current[paper.id];
      // 已知该论文无附件：弹提示后直接 return
      if (cached && cached.length === 0) {
        message.info(t('message.noAttachments'));
        return;
      }

      // 未缓存：先 fetch，根据结果再决定要不要展开。
      // 这样在 API 返回前 id 不会进 expandedRowKeys，antd Table 不会渲染展开行 <tr>，
      // 避免出现"先空 Spin/空 td、再消失"的闪烁。
      if (!cached) {
        await loadAttachments(paper.id);
        if ((attachmentCacheRef.current[paper.id] || []).length === 0) {
          message.info(t('message.noAttachments'));
          return;
        }
      }
      setExpandedRowKeys(prev => prev.includes(paper.id) ? prev : [...prev, paper.id]);
    } else {
      setExpandedRowKeys(prev => prev.filter(id => id !== paper.id));
    }
  }, [loadAttachments, message, t]);

  // 拖拽外部文件到论文行上传附件
  useEffect(() => {
    const wrapper = tableWrapperRef.current;
    if (!wrapper) return;

    let highlightedRow: HTMLElement | null = null;
    const HIGHLIGHT_CLASS = 'paper-row-drop-highlight';

    const clearHighlight = () => {
      if (highlightedRow) {
        highlightedRow.classList.remove(HIGHLIGHT_CLASS);
        highlightedRow = null;
      }
    };

    const isExternalFileDrag = (e: any) => {
      const types = e.dataTransfer?.types;
      if (!types) return false;
      const arr = Array.isArray(types) ? types : Array.from(types);
      return arr.includes('Files');
    };

    const getPaperIdFromRow = (e: any) => {
      const tr = (e.target as HTMLElement).closest('tr[data-row-key]');
      if (!tr) return null;
      const key = tr.getAttribute('data-row-key');
      return key ? parseInt(key, 10) : null;
    };

    const onDragOver = (e: any) => {
      if (!isExternalFileDrag(e)) return;
      e.preventDefault();
      const tr = (e.target as HTMLElement).closest('tr[data-row-key]');
      if (tr && tr !== highlightedRow) {
        clearHighlight();
        tr.classList.add(HIGHLIGHT_CLASS);
        highlightedRow = tr as HTMLElement;
      }
    };

    const onDragLeave = (e: any) => {
      if (!wrapper.contains((e as any).relatedTarget)) {
        clearHighlight();
      }
    };

    const onDrop = async (e: any) => {
      if (!isExternalFileDrag(e)) return;
      e.preventDefault();
      e.stopPropagation();

      const paperId = getPaperIdFromRow(e);
      clearHighlight();
      if (!paperId) return;

      const files = Array.from(e.dataTransfer.files || []);
      if (files.length === 0) return;

      const pid = String(paperId);
      setExpandedRowKeys(prev => prev.includes(pid) ? prev : [...prev, pid]);
      if (!attachmentCacheRef.current[pid]) {
        loadAttachments(pid);
      }
      await handleUploadAttachments(paperId, files as File[]);
    };

    wrapper.addEventListener('dragover', onDragOver, { capture: true });
    wrapper.addEventListener('dragleave', onDragLeave, { capture: true });
    wrapper.addEventListener('drop', onDrop, { capture: true });

    return () => {
      wrapper.removeEventListener('dragover', onDragOver, { capture: true });
      wrapper.removeEventListener('dragleave', onDragLeave, { capture: true });
      wrapper.removeEventListener('drop', onDrop, { capture: true });
      clearHighlight();
    };
  }, [handleUploadAttachments, loadAttachments]);

  return {
    expandedRowKeys,
    setExpandedRowKeys,
    attachmentCache,
    attachmentCacheRef,
    attachmentLoading,
    loadAttachments,
    handleUploadAttachments,
    handleDeleteAttachment,
    handleSetPrimary,
    handleOpenAttachment,
    handleExpand,
  };
}
