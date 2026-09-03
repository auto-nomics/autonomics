import { useState, useEffect } from '../../../framework';
import { useCapability, usePlugin } from '../../../core/shared';
import { ScrollPlugin, ScrollScope } from '../..';

export const useScrollPlugin = () => usePlugin<ScrollPlugin>(ScrollPlugin.id);
export const useScrollCapability = () => useCapability<ScrollPlugin>(ScrollPlugin.id);

// Define the return type explicitly to maintain type safety
interface UseScrollReturn {
  provides: ScrollScope | null;
  state: {
    currentPage: number;
    totalPages: number;
  };
}

export const useScroll = (documentId: string): UseScrollReturn => {
  const { provides } = useScrollCapability();
  const [currentPage, setCurrentPage] = useState(1);
  const [totalPages, setTotalPages] = useState(1);

  useEffect(() => {
    if (!provides || !documentId) return;

    const scope = provides.forDocument(documentId);
    setCurrentPage(scope.getCurrentPage());
    setTotalPages(scope.getTotalPages());

    const unsubPageChange = provides.onPageChange((event) => {
      if (event.documentId === documentId) {
        setCurrentPage(event.pageNumber);
        setTotalPages(event.totalPages);
      }
    });

    // 文档首次布局完成时也会携带正确的 totalPages，
    // 确保在用户尚未滚动时就能拿到真实页数。
    const unsubLayoutReady = provides.onLayoutReady((event) => {
      if (event.documentId === documentId) {
        setTotalPages(event.totalPages);
      }
    });

    return () => {
      unsubPageChange();
      unsubLayoutReady();
    };
  }, [provides, documentId]);

  return {
    // New format (preferred)
    provides: provides?.forDocument(documentId) ?? null,
    state: {
      currentPage,
      totalPages,
    },
  };
};
