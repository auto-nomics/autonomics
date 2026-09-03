/**
 * useGuideBlockData - V2 导览块数据管理 Hook
 *
 * 管理导览节点和 PDF 块数据的加载、处理和计算。
 * 从 PaperReaderPage 中提取，封装了 V2 导览系统的核心数据逻辑。
 *
 * 功能：
 * - 加载导览树和 PDF 块数据
 * - 建立块→节点的空间映射关系
 * - 根据当前页码计算当前章节、引导和锚点
 */
import { useState, useEffect, useMemo } from 'react';

/**
 * 在导览树中查找指定 ID 的节点
 */
function findNodeById(nodes: any, nodeId: any): any {
  if (!nodes || !nodeId) return null;

  for (const node of nodes) {
    if (node.id === nodeId || node.nodeId === nodeId) {
      return node;
    }
    if (node.children && node.children.length > 0) {
      const found: any = findNodeById(node.children, nodeId);
      if (found) return found;
    }
  }
  return null;
}

export default function useGuideBlockData({ paperId, paper, currentPage }: any) {
  const [blocksData, setBlocksData] = useState<any[]>([]);
  const [guideNodes, setGuideNodes] = useState<any[]>([]);

  // 加载 V2 导览节点和 PDF 块数据
  useEffect(() => {
    const controller = new AbortController();

    async function loadGuideNodes() {
      if (!paper || paper.guide_version !== 'v2') {
        setBlocksData([]);
        return;
      }

      try {
        const { getGuideTree, getBlocks } = await import('../../../services/guideApi');

        // 并行加载导览树和 PDF 块数据
        const [guideData, blocksResponse] = await Promise.all([
          getGuideTree(paperId, controller.signal),
          getBlocks(paperId, null, controller.signal).catch(() => null),
        ]);

        if (controller.signal.aborted) return;

        // 扁平化导览树
        const nodeEntries: any[] = [];

        // 解析 page_range
        const parsePageRange = (range: any) => {
          if (typeof range === 'number') return range;
          if (!range) return 0;
          try {
            const parsed = typeof range === 'string' ? JSON.parse(range) : range;
            if (parsed && typeof parsed === 'object') {
              return parsed.start_page_idx ?? 0;
            }
            if (typeof parsed === 'number') return parsed;
          } catch {
            if (typeof range === 'string') {
              const firstPage = parseInt(range.split('-')[0], 10);
              return isNaN(firstPage) ? 0 : firstPage;
            }
          }
          return 0;
        };

        // 解析 primary_bbox
        const parseBbox = (bbox: any) => {
          if (Array.isArray(bbox)) return bbox;
          if (!bbox) return null;
          try {
            const parsed = typeof bbox === 'string' ? JSON.parse(bbox) : bbox;
            if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
              return parsed.bbox || null;
            }
            if (Array.isArray(parsed)) return parsed;
          } catch {
            return null;
          }
          return null;
        };

        // 递归遍历导览树
        const traverseNodes = (nodeList: any) => {
          nodeList.forEach((node: any) => {
            const bbox = parseBbox(node.primaryBbox);
            const pageIdx = parsePageRange(node.pageRange);

            if (bbox && bbox.length === 4) {
              nodeEntries.push({
                id: node.id,
                node_id: node.id,
                page_idx: pageIdx,
                bbox: [bbox[0], bbox[1], bbox[2], bbox[3]],
                title: node.title,
              });
            }

            if (node.children && node.children.length > 0) {
              traverseNodes(node.children);
            }
          });
        };

        if (guideData && guideData.nodes) {
          traverseNodes(guideData.nodes);
          setGuideNodes(guideData.nodes);
        }

        // 建立块→节点映射
        if (blocksResponse && blocksResponse.blocks && blocksResponse.blocks.length > 0) {
          const preciseBlocks = [];

          for (const block of blocksResponse.blocks) {
            const b = block as any;
            if (b.bboxX0 == null || b.bboxY0 == null ||
                b.bboxX1 == null || b.bboxY1 == null) {
              continue;
            }

            const blockX0 = b.bboxX0;
            const blockY0 = b.bboxY0;
            const blockX1 = b.bboxX1;
            const blockY1 = b.bboxY1;

            let matchedNodeId: string | null = null;
            for (const node of nodeEntries) {
              if (node.page_idx !== b.pageIdx) continue;
              const [nx0, ny0, nx1, ny1] = node.bbox;
              if (blockX0 >= nx0 && blockX1 <= nx1 && blockY0 >= ny0 && blockY1 <= ny1) {
                matchedNodeId = node.node_id;
                break;
              }
            }

            preciseBlocks.push({
              id: (b as any).id,
              page_idx: b.pageIdx,
              bbox: [blockX0, blockY0, blockX1, blockY1],
              node_id: matchedNodeId,
              blockType: b.blockType,
            });
          }

          setBlocksData(preciseBlocks);
        } else {
          setBlocksData(nodeEntries);
        }
      } catch (err) {
        if ((err as any).name !== 'AbortError') {
          console.warn('[useGuideBlockData] 加载导览节点失败:', err);
          setBlocksData([]);
        }
      }
    }

    loadGuideNodes();

    return () => {
      controller.abort();
    };
  }, [paperId, paper?.guide_version]);

  // 根据当前页码计算当前章节
  const currentSection = useMemo(() => {
    if (!blocksData || blocksData.length === 0 || !guideNodes) {
      return null;
    }

    const targetPageIdx = currentPage - 1;
    const matchedBlock = blocksData.find((block: any) => block.page_idx === targetPageIdx);

    if (!matchedBlock) {
      const prevPageBlocks = blocksData.filter((b: any) => b.page_idx < targetPageIdx);
      if (prevPageBlocks.length > 0) {
        const nearestBlock = prevPageBlocks[prevPageBlocks.length - 1];
        return findNodeById(guideNodes, nearestBlock.node_id || nearestBlock.id);
      }
      return null;
    }

    return findNodeById(guideNodes, (matchedBlock as any).node_id || (matchedBlock as any).id);
  }, [blocksData, currentPage, guideNodes]);

  const currentGuide = useMemo(() => {
    if (!currentSection?.readingGuide) return null;
    try {
      return typeof currentSection.readingGuide === 'string'
        ? JSON.parse(currentSection.readingGuide)
        : currentSection.readingGuide;
    } catch {
      return null;
    }
  }, [currentSection]);

  const currentAnchors = useMemo(() => {
    if (!currentSection?.attentionAnchors) return [];
    try {
      return typeof currentSection.attentionAnchors === 'string'
        ? JSON.parse(currentSection.attentionAnchors)
        : currentSection.attentionAnchors || [];
    } catch {
      return [];
    }
  }, [currentSection]);

  return {
    blocksData,
    guideNodes,
    currentSection,
    currentGuide,
    currentAnchors,
  };
}

export { findNodeById };
