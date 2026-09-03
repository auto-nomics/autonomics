/**
 * ReadingContextBarContainer 组件
 *
 * 容器组件，负责将 useScrollContext Hook 与 ReadingContextBar 组合。
 * 它从 PdfViewer 获取 PDF 容器引用，监听滚动事件，并传递当前章节数据给 ReadingContextBar。
 *
 * @module ReadingContextBarContainer
 */

import React, { useMemo } from 'react';
import ReadingContextBar from './ReadingContextBar';
import useScrollContext from './hooks/useScrollContext';
import useAnchors from './hooks/useAnchors';

/**
 * 将后端返回的节点数据映射为前端期望的格式
 *
 * 与 ParagraphGuideV2 的 mapNode 函数保持一致。
 *
 * @param {object} node - 后端返回的节点对象
 * @returns {object} 前端期望的节点对象
 */
const mapNode = (node: any) => {
  // 解析页码信息
  let pageIdx = null;
  try {
    if (node.pageRange) {
      const range = JSON.parse(node.pageRange);
      pageIdx = range?.start_page_idx ?? null;
    }
  } catch {
    // pageRange 格式异常时保持 null
  }

  // 解析边界框信息
  let bboxes: any[] = [];
  try {
    if (node.primaryBbox) {
      bboxes = [JSON.parse(node.primaryBbox)];
    }
  } catch {
    // primaryBbox 格式异常时保持空数组
  }

  const role = node.role || 'default';
  return {
    id: node.id,
    title: node.title,
    summary: node.summary || '',
    pageIdx,
    logicalRole: role,
    importance: node.importance || 0,
    bboxes,
    imageUrl: node.imageUrl || null,
    children: (node.children || []).map(mapNode),
    // V4 导览阅读体验：新增字段映射
    sectionRole: node.sectionRole || null,
    readingGuide: node.readingGuide || null,
    attentionAnchors: node.attentionAnchors || null,
    criticalHints: node.criticalHints || null,
    skipHint: node.skipHint || null,
    argumentContext: node.argumentContext || null,
    whyExists: node.whyExists || null,
  };
};

/**
 * ReadingContextBar 容器组件
 *
 * @param {object} props - 组件属性
 * @param {Array} props.guideNodes - 导览树节点数组（来自后端 API，原始格式）
 * @param {Array} props.blocksData - PDF 块数据（用于坐标匹配）
 * @param {React.RefObject} props.pdfViewerRef - PdfViewer 组件的 ref
 * @param {Function} props.onNavigate - 导航回调函数
 */
function ReadingContextBarContainer({
  guideNodes,
  blocksData,
  pdfViewerRef,
  onNavigate,
}: {
  guideNodes: any;
  blocksData: any;
  pdfViewerRef: any;
  onNavigate: any;
}) {
  // 从 PdfViewer ref 获取 pdfAreaRef
  const pdfAreaRef = pdfViewerRef?.current?.pdfAreaRef || null;

  // 使用 useScrollContext Hook 跟踪当前章节
  const {
    currentSection,
    currentGuide,
    currentAnchors,
  } = useScrollContext({
    pdfContainerRef: pdfAreaRef,
    blocksData,
    guideNodes,
    throttleMs: 200,
  });

  // 使用 useAnchors Hook 管理锚点
  const { handleAnchorClick } = useAnchors({
    blocksData,
    currentSection,
    onNavigate,
  });

  // 如果没有导览数据，不渲染
  if (!guideNodes || guideNodes.length === 0) {
    return null;
  }

  return (
    <ReadingContextBar
      currentSection={currentSection}
      currentGuide={currentGuide}
      anchors={currentAnchors}
      onAnchorClick={handleAnchorClick}
    />
  );
}

export default ReadingContextBarContainer;
