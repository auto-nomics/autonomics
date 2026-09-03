/**
 * useScrollContext Hook
 *
 * 根据 PDF 滚动位置匹配当前章节，用于 ReadingContextBar 显示
 *
 * 核心功能：
 * 1. 监听 PDF 容器滚动事件（200ms 节流）
 * 2. 获取当前视口中心点的页码和 y 坐标
 * 3. 在 guide_nodes 中找到覆盖当前视口的章节节点
 * 4. 返回 { currentSection, currentGuide, currentAnchors }
 *
 * @module useScrollContext
 */

import { useState, useEffect, useCallback, useRef } from 'react'; // 导入 React 核心钩子

/**
 * 节流函数
 *
 * 限制函数执行频率，避免滚动事件频繁触发导致的性能问题。
 *
 * @param {Function} func - 需要节流的函数
 * @param {number} delay - 节流延迟时间（毫秒）
 * @returns {Function} 节流后的函数
 */
function throttle(func: any, delay: any) {
  let lastCall = 0; // 上次执行时间戳
  return function (this: any, ...args: any[]) {
    const now = Date.now(); // 当前时间戳
    if (now - lastCall >= delay) {
      lastCall = now; // 更新上次执行时间
      return func.apply(this, args); // 执行函数
    }
  };
}

/**
 * 根据滚动位置查找匹配的导览节点
 *
 * @param {Array} blocksData - PDF 块数据（包含导览节点信息）
 * @param {Object} scrollPosition - 当前滚动位置 { pageIdx, y }
 * @returns {Object|null} 匹配的节点数据，如果没找到返回 null
 */
function findNodeAtPosition(blocksData: any, scrollPosition: any) {
  if (!blocksData || blocksData.length === 0 || !scrollPosition) {
    return null;
  }

  const { pageIdx, y } = scrollPosition;

  // 按页码筛选候选节点
  const candidates = blocksData.filter((block: any) => block.page_idx === pageIdx);

  if (candidates.length === 0) {
    // 如果当前页没有节点，尝试找最近的上一页的节点
    const prevPageNodes = blocksData.filter((block: any) => block.page_idx < pageIdx);
    if (prevPageNodes.length > 0) {
      return prevPageNodes[prevPageNodes.length - 1]; // 返回最近的上一页节点
    }
    return null;
  }

  // 在当前页的候选节点中，找到覆盖当前 y 坐标的节点
  // y 是归一化坐标（0-1000），与 bbox 使用相同坐标系
  for (const node of candidates) {
    const [x0, y0, x1, y1] = node.bbox || [];
    if (y >= y0 && y <= y1) {
      return node;
    }
  }

  // 如果没有精确匹配，返回当前页的第一个节点
  return candidates[0];
}

/**
 * 滚动上下文 Hook
 *
 * @param {Object} params - 参数对象
 * @param {HTMLElement} params.pdfContainerRef - PDF 容器的 DOM 引用
 * @param {Array} params.blocksData - PDF 块数据（包含导览节点信息）
 * @param {Array} params.guideNodes - 完整的导览节点树（用于获取完整节点数据）
 * @param {number} [params.throttleMs=200] - 节流延迟时间（毫秒）
 * @returns {Object} { currentSection, currentGuide, currentAnchors, scrollToSection }
 */
export default function useScrollContext({
  pdfContainerRef,
  blocksData,
  guideNodes,
  throttleMs = 200,
}: {
  pdfContainerRef: any;
  blocksData: any;
  guideNodes: any;
  throttleMs?: number;
}) {
  /**
   * 当前章节状态
   */
  const [currentSection, setCurrentSection] = useState<any>(null); // 当前匹配的节点

  /**
   * 上次滚动位置缓存
   */
  const lastScrollPositionRef = useRef<any>(null); // 避免重复计算

  /**
   * 获取 PDF 容器当前滚动位置
   *
   * 返回当前视口中心点对应的页码和 y 坐标。
   *
   * @returns {Object|null} { pageIdx, y } 或 null
   */
  const getScrollPosition = useCallback(() => {
    if (!pdfContainerRef) {
      return null;
    }

    const container = pdfContainerRef;
    if (!container) {
      return null;
    }

    // 获取容器滚动信息
    const scrollTop = container.scrollTop;
    const scrollHeight = container.scrollHeight;
    const clientHeight = container.clientHeight;

    // 计算视口中心点
    const viewportCenter = scrollTop + clientHeight / 2;

    // 简化处理：假设 PDF 每页高度相同
    // TODO: 更精确的实现需要从 PdfViewer 获取每页的实际高度
    const approxPageHeight = scrollHeight / (blocksData?.length > 0
      ? Math.max(...blocksData.map((b: any) => b.page_idx)) + 1
      : 1);

    const pageIdx = Math.floor(viewportCenter / approxPageHeight);
    const y = (viewportCenter % approxPageHeight) / approxPageHeight * 1000; // 归一化为 0-1000

    return { pageIdx, y };
  }, [pdfContainerRef, blocksData]);

  /**
   * 滚动事件处理函数
   */
  const handleScroll = useCallback(() => {
    const scrollPosition = getScrollPosition();
    if (!scrollPosition) {
      return;
    }

    // 避免重复计算
    if (lastScrollPositionRef.current &&
        (lastScrollPositionRef.current as any).pageIdx === scrollPosition.pageIdx &&
        Math.abs((lastScrollPositionRef.current as any).y - scrollPosition.y) < 50) {
      return;
    }

    lastScrollPositionRef.current = scrollPosition;

    // 查找匹配的节点
    const matchedNode = findNodeAtPosition(blocksData, scrollPosition);

    // 找到完整节点数据
    let fullNodeData = null;
    if (matchedNode && guideNodes) {
      // 递归查找完整节点
      const findFullNode = (nodes: any[], nodeId: any): any => {
        for (const node of nodes) {
          if (node.id === nodeId || node.nodeId === nodeId) {
            return node;
          }
          if (node.children && node.children.length > 0) {
            const found: any = findFullNode(node.children, nodeId);
            if (found) return found;
          }
        }
        return null;
      };

      fullNodeData = findFullNode(guideNodes, matchedNode.id || matchedNode.node_id);
    }

    if (fullNodeData && fullNodeData.id !== currentSection?.id) {
      setCurrentSection(fullNodeData);
    }
  }, [blocksData, guideNodes, currentSection, getScrollPosition]);

  /**
   * 滚动到指定章节
   *
   * @param {string} nodeId - 目标节点 ID
   */
  const scrollToSection = useCallback((nodeId: any) => {
    if (!pdfContainerRef || !blocksData) {
      return;
    }

    // 查找目标节点
    const targetNode = blocksData.find((block: any) =>
      block.id === nodeId || block.node_id === nodeId
    );

    if (!targetNode) {
      console.warn('[useScrollContext] 未找到目标节点:', nodeId);
      return;
    }

    const { page_idx, bbox } = targetNode;
    if (!bbox) {
      return;
    }

    // 计算滚动位置
    const approxPageHeight = pdfContainerRef.scrollHeight /
      (Math.max(...blocksData.map((b: any) => b.page_idx)) + 1);

    const [x0, y0] = bbox;
    const targetScrollTop = page_idx * approxPageHeight + (y0 / 1000) * approxPageHeight;

    // 滚动到目标位置
    pdfContainerRef.scrollTo({
      top: targetScrollTop - pdfContainerRef.clientHeight / 2, // 居中显示
      behavior: 'smooth',
    });
  }, [pdfContainerRef, blocksData]);

  /**
   * 设置滚动监听
   */
  useEffect(() => {
    if (!pdfContainerRef) {
      return;
    }

    const container = pdfContainerRef;
    const throttledHandleScroll = throttle(handleScroll, throttleMs);

    container.addEventListener('scroll', throttledHandleScroll);
    container.addEventListener('resize', throttledHandleScroll);

    // 初始化时执行一次
    throttledHandleScroll();

    return () => {
      container.removeEventListener('scroll', throttledHandleScroll);
      container.removeEventListener('resize', throttledHandleScroll);
    };
  }, [pdfContainerRef, handleScroll, throttleMs]);

  /**
   * 解析当前节点的阅读引导数据
   */
  const currentGuide = currentSection ? (() => {
    let parsedGuide = null;
    try {
      if (currentSection.readingGuide) {
        parsedGuide = typeof currentSection.readingGuide === 'string'
          ? JSON.parse(currentSection.readingGuide)
          : currentSection.readingGuide;
      }
    } catch {
      parsedGuide = null;
    }
    return parsedGuide;
  })() : null;

  /**
   * 解析当前节点的注意力锚点数据
   */
  const currentAnchors = currentSection ? (() => {
    let parsedAnchors = [];
    try {
      if (currentSection.attentionAnchors) {
        parsedAnchors = typeof currentSection.attentionAnchors === 'string'
          ? JSON.parse(currentSection.attentionAnchors)
          : currentSection.attentionAnchors || [];
      }
    } catch {
      parsedAnchors = [];
    }
    return parsedAnchors;
  })() : [];

  return {
    currentSection,  // 当前章节节点数据
    currentGuide,     // 当前章节的阅读引导（已解析）
    currentAnchors,   // 当前章节的注意力锚点（已解析）
    scrollToSection,  // 滚动到指定章节的方法
  };
}
