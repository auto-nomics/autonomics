/**
 * useAnchors Hook
 *
 * 管理注意力锚点状态，提供锚点与 PDF 页面的位置映射
 *
 * 核心功能：
 * 1. 解析章节节点的注意力锚点数据（JSON 字符串 → 数组）
 * 2. 提供锚点位置映射（location_hint → 页码 + 坐标）
 * 3. 提供锚点点击处理（跳转到 PDF 对应位置）
 *
 * @module useAnchors
 */

import { useMemo, useCallback } from 'react'; // 导入 React 核心钩子

/**
 * 注意力锚点 Hook
 *
 * @param {Object} params - 参数对象
 * @param {Array} params.blocksData - PDF 块数据（用于位置映射）
 * @param {Object} params.currentSection - 当前章节节点数据
 * @param {Function} params.onNavigate - 导航回调函数（可选）
 * @returns {Object} { anchors, anchorPositions, handleAnchorClick }
 */
export default function useAnchors({
  blocksData,
  currentSection,
  onNavigate,
}: {
  blocksData: any;
  currentSection: any;
  onNavigate: any;
}) {
  /**
   * 解析并格式化注意力锚点数据
   *
   * 将 JSON 字符串解析为数组，并添加默认值。
   */
  const anchors = useMemo(() => {
    if (!currentSection?.attentionAnchors) {
      return [];
    }

    let parsed = [];
    try {
      parsed = typeof currentSection.attentionAnchors === 'string'
        ? JSON.parse(currentSection.attentionAnchors)
        : currentSection.attentionAnchors || [];
    } catch {
      parsed = [];
    }

    // 确保返回数组，并为每个锚点添加默认值
    return Array.isArray(parsed) ? parsed.map((anchor, index) => ({
      anchor_type: anchor.anchor_type || 'paragraph',
      location_hint: anchor.location_hint || `锚点 ${index + 1}`,
      guidance: anchor.guidance || '',
      connects_to: anchor.connects_to || '',
      reference_count: anchor.reference_count || 0,
    })) : [];
  }, [currentSection]);

  /**
   * 锚点位置映射
   *
   * 将 location_hint（如 "Eq. 4"、"Table 3"）映射到 PDF 页面位置。
   *
   * 注意：这是一个简化的实现，精确匹配需要更复杂的逻辑：
   * - 需要解析 PDF 文本层的引用内容
   * - 需要维护引用位置索引
   *
   * 当前实现返回空映射，后续可以优化。
   */
  const anchorPositions = useMemo(() => {
    const positions = {};

    // TODO: 实现更精确的位置映射
    // 当前简化实现：根据 blocksData 中的标题/标签进行匹配
    if (blocksData && blocksData.length > 0) {
      anchors.forEach((anchor) => {
        const hint = anchor.location_hint.toLowerCase();

        // 尝试匹配公式引用（Eq. 4, Equation 4, (4)）
        const equationMatch = hint.match(/(?:eq\.?|equation|\()?\s*(\d+)/);
        if (equationMatch) {
          // 在 blocksData 中查找包含对应公式编号的块
          const matchingBlock = blocksData.find((block: any) => {
            const blockText = (block.title || '').toLowerCase();
            return blockText.includes(`eq${equationMatch[1]}`) ||
                   blockText.includes(`equation${equationMatch[1]}`) ||
                   blockText.includes(`(${equationMatch[1]}`);
          });
          if (matchingBlock) {
            (positions as any)[anchor.location_hint] = {
              pageIdx: matchingBlock.page_idx,
              bbox: matchingBlock.bbox,
            };
          }
        }

        // 尝试匹配表格引用（Table 3, Tab. 3）
        const tableMatch = hint.match(/(?:table|tab\.?)\s*(\d+)/);
        if (tableMatch) {
          const matchingBlock = blocksData.find((block: any) => {
            const blockText = (block.title || '').toLowerCase();
            return blockText.includes(`table${tableMatch[1]}`) ||
                   blockText.includes(`tab${tableMatch[1]}`);
          });
          if (matchingBlock) {
            (positions as any)[anchor.location_hint] = {
              pageIdx: matchingBlock.page_idx,
              bbox: matchingBlock.bbox,
            };
          }
        }

        // 尝试匹配图片引用（Fig. 3, Figure 3）
        const figureMatch = hint.match(/(?:fig\.?|figure)\s*(\d+)/);
        if (figureMatch) {
          const matchingBlock = blocksData.find((block: any) => {
            const blockText = (block.title || '').toLowerCase();
            return blockText.includes(`fig${figureMatch[1]}`) ||
                   blockText.includes(`figure${figureMatch[1]}`);
          });
          if (matchingBlock) {
            (positions as any)[anchor.location_hint] = {
              pageIdx: matchingBlock.page_idx,
              bbox: matchingBlock.bbox,
            };
          }
        }
      });
    }

    return positions;
  }, [anchors, blocksData]);

  /**
   * 处理锚点点击事件
   *
   * 跳转到 PDF 中对应的锚点位置。
   *
   * @param {Object} anchor - 被点击的锚点数据
   */
  const handleAnchorClick = useCallback((anchor: any) => {
    const position = (anchorPositions as any)[anchor.location_hint];
    if (position && onNavigate) {
      onNavigate({
        pageIdx: position.pageIdx,
        bboxes: position.bbox ? [position.bbox] : [],
        animationMs: 2000,
      });
    } else if (onNavigate && currentSection) {
      // 如果没有精确位置，跳转到当前章节起始位置
      onNavigate({
        pageIdx: currentSection.pageIdx,
        bboxes: currentSection.bboxes || [],
        animationMs: 2000,
      });
    }
  }, [anchorPositions, onNavigate, currentSection]);

  return {
    anchors,           // 解析后的锚点数组
    anchorPositions,   // 锚点位置映射 { location_hint: { pageIdx, bbox } }
    handleAnchorClick, // 锚点点击处理函数
  };
}
