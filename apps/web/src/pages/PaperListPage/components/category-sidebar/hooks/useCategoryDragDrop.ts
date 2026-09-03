/**
 * useCategoryDragDrop Hook
 *
 * 处理分类侧边栏的拖放功能
 * 管理两个拖放系统：
 * 1. 内部树节点重排序（由 rc-tree 处理）
 * 2. 外部文件/论文拖入分类（此处处理）
 *
 * 使用原生 DOM 捕获监听器拦截拖放事件，在 rc-tree 能够
 * stopPropagation() 之前进行拦截，实现可靠的外部拖放检测
 */
import { useRef, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { assignPapers } from '../../../../../services/categoriesApi';

interface UseCategoryDragDropProps {
  /** 将 PDF 文件上传到分类的回调 */
  onUploadToCategory?: (file: File, categoryId: string | null) => Promise<void>;
  /** 将参考文献文件导入到分类的回调 */
  onImportBibliography?: (file: File, categoryId: string | null) => Promise<void>;
  /** 拖放操作完成后刷新分类数据的回调 */
  loadCategories: () => Promise<void>;
  /** 通知父组件论文列表更新的回调 */
  onPapersUpdate?: () => void;
  /** Ant Design message 实例，用于通知提示 */
  message: { success: (msg: string) => void; warning: (msg: string) => void; error: (msg: string) => void };
}

/**
 * 分类拖放功能的自定义 Hook
 *
 * @param props - 拖放行为的配置属性
 * @returns wrapperRef - 附加到树包装器元素的 ref
 */
export function useCategoryDragDrop({
  onUploadToCategory,
  onImportBibliography,
  loadCategories,
  onPapersUpdate,
  message,
}: UseCategoryDragDropProps) {
  const treeWrapperRef = useRef<HTMLDivElement>(null);
  const { t } = useTranslation('paperList');

  useEffect(() => {
    const wrapper = treeWrapperRef.current;
    if (!wrapper) return;

    let highlightedNode: HTMLElement | null = null;
    const HIGHLIGHT_CLASS = 'category-drop-highlight';

    // 清除高亮样式
    const clearHighlight = () => {
      if (highlightedNode) {
        highlightedNode.classList.remove(HIGHLIGHT_CLASS);
        highlightedNode = null;
      }
    };

    // 从事件中获取分类键名
    const getCategoryKeyFromEvent = (e: DragEvent): string | null => {
      const target = e.target as HTMLElement;
      const treeNode = target?.closest('.rc-tree-treenode, .ant-tree-treenode');
      if (!treeNode) return null;
      const keyEl = treeNode.querySelector('[data-category-key]');
      return (keyEl as HTMLElement)?.dataset?.categoryKey ?? null;
    };

    // 判断是否为外部拖放（文件或论文）
    const isExternalDrag = (e: DragEvent): boolean => {
      const types = e.dataTransfer?.types;
      if (!types) return false;
      const arr = Array.isArray(types) ? types : Array.from(types);
      return arr.includes('Files') || arr.includes('application/jayread-paper');
    };

    const onDragOver = (e: Event) => {
      if (!isExternalDrag(e as DragEvent)) return;
      const treeNode = (e.target as Element).closest('.rc-tree-treenode, .ant-tree-treenode');
      if (treeNode && treeNode !== highlightedNode) {
        clearHighlight();
        treeNode.classList.add(HIGHLIGHT_CLASS);
        highlightedNode = treeNode as HTMLElement;
      }
    };

    const onDragLeave = (e: Event) => {
      if (!wrapper.contains((e as DragEvent).relatedTarget as Node)) {
        clearHighlight();
      }
    };

    const onDrop = async (e: Event) => {
      const dragEvent = e as DragEvent;
      if (!isExternalDrag(dragEvent)) return;
      dragEvent.stopPropagation();
      dragEvent.preventDefault();

      const key = getCategoryKeyFromEvent(dragEvent);
      clearHighlight();
      if (key === null) return;

      const isVirtual = key === 'all' || key === 'uncategorized';
      const droppedFiles = dragEvent.dataTransfer?.files;
      // 处理文件拖放
      if (droppedFiles && droppedFiles.length > 0) {
        const categoryId = isVirtual ? null : key;
        for (const file of droppedFiles as unknown as File[]) {
          if (!file.name.toLowerCase().endsWith('.pdf')) {
            message.warning(t('message.skipNonPdf', { filename: file.name }));
            continue;
          }
          try {
            await onUploadToCategory?.(file, categoryId);
            message.success(t('message.uploadSuccess', { filename: file.name }));
          } catch (err) {
            message.error(t('message.uploadFileFailed', { filename: file.name, error: (err as Error).message }));
          }
        }
        await loadCategories();
        return;
      }

      // 处理论文拖放
      const paperIdStr = dragEvent.dataTransfer?.getData('application/jayread-paper');
      if (!paperIdStr) return;
      const paperId = paperIdStr;
      const categoryId = key;
      try {
        await assignPapers([paperId], [categoryId]);
        message.success(t('message.addedToCategory'));
        await loadCategories();
        if (onPapersUpdate) onPapersUpdate();
      } catch (err) {
        message.error(t('message.categoryFailed', { error: (err as Error).message }));
      }
    };

    // 使用捕获阶段监听，确保在 rc-tree 处理之前拦截事件
    wrapper.addEventListener('dragover', onDragOver, { capture: true });
    wrapper.addEventListener('dragleave', onDragLeave, { capture: true });
    wrapper.addEventListener('drop', onDrop, { capture: true });

    return () => {
      wrapper.removeEventListener('dragover', onDragOver, { capture: true });
      wrapper.removeEventListener('dragleave', onDragLeave, { capture: true });
      wrapper.removeEventListener('drop', onDrop, { capture: true });
      clearHighlight();
    };
  }, [onUploadToCategory, onImportBibliography, loadCategories, onPapersUpdate, message]);

  return { treeWrapperRef };
}
