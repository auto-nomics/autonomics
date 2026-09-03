/**
 * JayRead 锚点视觉提示层组件
 *
 * Layer 2 伴读模式的核心组件，在 PDF 页面上为注意力锚点渲染微弱的视觉提示标记。
 *
 * 核心功能：
 * 1. 根据 attention_anchors 数据在 PDF 页面上渲染视觉提示标记
 * 2. 不同锚点类型使用不同的标记样式（公式竖线、段落三角、图表圆点、claim 高亮条）
 * 3. 标记极微弱，不干扰阅读，但能在余光中提供"这里值得停一下"的信号
 * 4. 点击标记展开详细引导（guidance + connects_to）
 * 5. 支持深色模式
 *
 * 坐标系统说明：
 * - 使用 MinerU 归一化坐标（0-1000），与 CoordinateHighlightOverlay 一致
 * - bbox: [x0, y0, x1, y1]，需要转换为 PDF point 再到屏幕像素
 *
 * @module AnchorOverlay
 */

import React, { useMemo, useState, useCallback, useEffect } from 'react';
import PropTypes from 'prop-types';
import { Tooltip } from 'antd';
import type { PageInfo } from '../pdf-viewer/types';
export type { PageInfo };

/**
 * MinerU 归一化坐标范围（与 CoordinateHighlightOverlay 保持一致）
 */
const COORD_RANGE = 1000;

/**
 * 不同锚点类型的默认样式配置
 */
const ANCHOR_STYLES = {
  equation: {
    // 公式锚点：极细的半透明淡色竖线
    type: 'line',
    color: 'rgba(59, 130, 246, 0.25)', // 淡蓝色
    lineWidth: 1,
    darkColor: 'rgba(96, 165, 250, 0.3)',
  },
  paragraph: {
    // 段落锚点：极小的三角标记（▲）
    type: 'triangle',
    color: 'rgba(107, 114, 128, 0.2)', // 淡灰色
    size: 6,
    darkColor: 'rgba(156, 163, 175, 0.25)',
  },
  figure: {
    // 图锚点：小圆点标记
    type: 'circle',
    color: 'rgba(34, 197, 94, 0.25)', // 淡绿色
    radius: 3,
    darkColor: 'rgba(74, 222, 128, 0.3)',
  },
  table: {
    // 表格锚点：小圆点标记（与 figure 相同）
    type: 'circle',
    color: 'rgba(249, 115, 22, 0.25)', // 淡橙色
    radius: 3,
    darkColor: 'rgba(251, 146, 60, 0.3)',
  },
  claim: {
    // claim 锚点：微弱的背景高亮条
    type: 'highlight',
    color: 'rgba(168, 85, 247, 0.08)', // 极淡的紫色
    darkColor: 'rgba(192, 132, 252, 0.12)',
  },
};

/**
 * 锚点视觉提示层组件
 *
 * @param {object} props - 组件属性
 * @param {Array} props.anchors - 注意力锚点数组
 * @param {object} props.pageInfo - PDFium PageInfo 对象（页面尺寸、视图框、旋转角度）
 * @param {number} props.pageIdx - 当前页码索引（0-based）
 * @param {number} props.scale - 当前缩放比例
 * @param {Function} [props.onAnchorClick] - 锚点点击回调
 */
interface AnchorData {
  anchor_type?: string;
  location_hint?: string;
  guidance?: string;
  connects_to?: string;
  reference_count?: number;
  block_id?: string;
  page_idx?: number;
  pageIdx?: number;
  bbox?: number[];
}

interface MarkerData {
  anchor: AnchorData;
  anchorId: string;
  anchorType: string;
  x: number;
  y: number;
  width: number;
  height: number;
  color: string;
  styleConfig: typeof ANCHOR_STYLES.equation;
  isPageLevel?: boolean;
}

function AnchorOverlay({
  anchors,
  pageInfo,
  pageIdx,
  scale,
  onAnchorClick,
}: {
  anchors: AnchorData[];
  pageInfo: PageInfo;
  pageIdx: number;
  scale: number;
  onAnchorClick?: (anchor: AnchorData) => void;
}) {
  /**
   * 展开的锚点详情状态
   */
  const [expandedAnchorId, setExpandedAnchorId] = useState<string | null>(null);

  /**
   * 跟踪深色模式状态
   */
  const [isDark, setIsDark] = useState(() => {
    return document.documentElement.classList.contains('dark') ||
           document.body.getAttribute('data-theme') === 'dark';
  });

  /**
   * 监听主题变化
   */
  useEffect(() => {
    const observer = new MutationObserver(() => {
      const dark = document.documentElement.classList.contains('dark') ||
                  document.body.getAttribute('data-theme') === 'dark';
      setIsDark(dark);
    });

    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['class'],
    });
    observer.observe(document.body, {
      attributes: true,
      attributeFilter: ['data-theme'],
    });

    return () => observer.disconnect();
  }, []);

  /**
   * 解析页面对象
   */
  const resolvedPage = useMemo(() => ({
    width: pageInfo.width,
    height: pageInfo.height,
    view: pageInfo.view,
    rotate: pageInfo.rotate,
  }), [pageInfo]);

  /**
   * 处理锚点点击
   */
  const handleAnchorClick = useCallback((anchor: AnchorData, event: React.MouseEvent) => {
    event.stopPropagation();
    const anchorId = `${anchor.pageIdx}-${anchor.block_id}-${anchor.location_hint}`;
    setExpandedAnchorId(prev => prev === anchorId ? null : anchorId);
    if (onAnchorClick) {
      onAnchorClick(anchor);
    }
  }, [onAnchorClick]);

  /**
   * 计算需要渲染的锚点标记列表
   */
  const anchorMarkers = useMemo(() => {
    if (!anchors || !resolvedPage || !scale || scale <= 0) {
      return [];
    }

    const pageWidth = resolvedPage.width || 612;
    const pageHeight = resolvedPage.height || 792;
    const pageRotate = resolvedPage.rotate || 0;
    const pageView = resolvedPage.view || [0, 0, pageWidth, pageHeight];

    const [cropX0, cropY0, cropX1, cropY1] = pageView;
    const cropOffsetX = cropX0;
    const cropOffsetY = cropY0;

    const dark = isDark;

    /**
     * 归一化坐标转 PDF point
     */
    const normalizedToPdf = (normalized: number, pdfSize: number): number => {
      return (normalized / COORD_RANGE) * pdfSize;
    };

    /**
     * 处理页面旋转
     */
    const applyRotation = (x: number, y: number): [number, number] => {
      switch (pageRotate) {
        case 90:
          return [pageHeight - y, x];
        case 180:
          return [pageWidth - x, pageHeight - y];
        case 270:
          return [y, pageWidth - x];
        default:
          return [x, y];
      }
    };

    /**
     * 处理单个锚点，转换为渲染坐标
     */
    const processAnchor = (anchor: AnchorData, index: number): MarkerData | null => {
      // 只处理当前页的锚点
      const anchorPageIdx = anchor.page_idx !== undefined ? anchor.page_idx :
                           (anchor.pageIdx !== undefined ? anchor.pageIdx : 0);
      if (anchorPageIdx !== pageIdx) {
        return null;
      }

      // 确定锚点类型
      const anchorType = anchor.anchor_type || 'paragraph';
      const styleConfig = (ANCHOR_STYLES as any)[anchorType] || ANCHOR_STYLES.paragraph;

      // 计算颜色
      const color = dark ? styleConfig.darkColor : styleConfig.color;

      // 如果有 bbox，使用 bbox 定位
      if (anchor.bbox && Array.isArray(anchor.bbox) && anchor.bbox.length === 4) {
        const [x0, y0, x1, y1] = anchor.bbox;

        // 归一化坐标 → PDF point
        const pdfX0 = normalizedToPdf(x0, pageWidth);
        const pdfY0 = normalizedToPdf(y0, pageHeight);
        const pdfX1 = normalizedToPdf(x1, pageWidth);
        const pdfY1 = normalizedToPdf(y1, pageHeight);

        // 应用旋转
        const [rotatedX0, rotatedY0] = applyRotation(pdfX0, pdfY0);
        const [rotatedX1, rotatedY1] = applyRotation(pdfX1, pdfY1);

        // 重新排序确保左上角和右下角
        const normalizedRotatedX0 = Math.min(rotatedX0, rotatedX1);
        const normalizedRotatedY0 = Math.min(rotatedY0, rotatedY1);
        const normalizedRotatedX1 = Math.max(rotatedX0, rotatedX1);
        const normalizedRotatedY1 = Math.max(rotatedY0, rotatedY1);

        // 减去 CropBox 偏移
        const relativeX0 = normalizedRotatedX0 - cropOffsetX;
        const relativeY0 = normalizedRotatedY0 - cropOffsetY;
        const relativeX1 = normalizedRotatedX1 - cropOffsetX;
        const relativeY1 = normalizedRotatedY1 - cropOffsetY;

        // PDF point → 屏幕像素
        const screenX = relativeX0 * scale;
        const screenY = relativeY0 * scale;
        const screenWidth = (relativeX1 - relativeX0) * scale;
        const screenHeight = (relativeY1 - relativeY0) * scale;

        // 生成唯一 ID
        const anchorId = `${anchorPageIdx}-${anchor.block_id || index}-${anchor.location_hint}`;

        return {
          anchor,
          anchorId,
          anchorType,
          x: screenX,
          y: screenY,
          width: screenWidth,
          height: screenHeight,
          color,
          styleConfig,
        };
      }

      // 如果没有 bbox，使用页面级定位（在页面顶部显示标记）
      const anchorId = `${anchorPageIdx}-pagelevel-${index}`;
      return {
        anchor,
        anchorId,
        anchorType,
        x: 10,
        y: 10 + index * 12, // 垂直排布，避免重叠
        width: 0,
        height: 0,
        color,
        styleConfig,
        isPageLevel: true,
      };
    };

    return anchors
      .map((anchor, index) => processAnchor(anchor, index))
      .filter(Boolean);
  }, [anchors, resolvedPage, scale, pageIdx, isDark]);

  /**
   * 如果没有锚点标记，不渲染
   */
  if (anchorMarkers.length === 0) {
    return null;
  }

  /**
   * 渲染锚点标记
   */
  const renderMarker = (marker: any) => {
    const { anchor, anchorId, anchorType, x, y, width, height, color, styleConfig, isPageLevel } = marker;
    const isExpanded = expandedAnchorId === anchorId;

    switch (styleConfig.type) {
      case 'line':
        // 公式锚点：竖线（在公式右侧）
        return (
          <>
            <line
              key={`${anchorId}-line`}
              x1={x + width + 2}
              y1={y}
              x2={x + width + 2}
              y2={y + height}
              stroke={color}
              strokeWidth={styleConfig.lineWidth}
              strokeLinecap="round"
              style={{ cursor: 'pointer' }}
              onClick={(e) => handleAnchorClick(anchor, e)}
            />
            <rect
              key={`${anchorId}-hitbox`}
              x={x + width}
              y={y}
              width={4}
              height={height}
              fill="transparent"
              style={{ cursor: 'pointer' }}
              onClick={(e) => handleAnchorClick(anchor, e)}
            />
          </>
        );

      case 'triangle':
        // 段落锚点：小三角（在段落左侧）
        const triangleSize = styleConfig.size;
        const triangleX = x - triangleSize - 2;
        const triangleY = y + height / 2;

        return (
          <>
            <polygon
              key={`${anchorId}-triangle`}
              points={`${triangleX},${triangleY - triangleSize/2} ${triangleX + triangleSize},${triangleY} ${triangleX},${triangleY + triangleSize/2}`}
              fill={color}
              style={{ cursor: 'pointer' }}
              onClick={(e) => handleAnchorClick(anchor, e)}
            />
            <rect
              key={`${anchorId}-hitbox`}
              x={triangleX - 2}
              y={triangleY - triangleSize}
              width={triangleSize + 4}
              height={triangleSize * 2}
              fill="transparent"
              style={{ cursor: 'pointer' }}
              onClick={(e) => handleAnchorClick(anchor, e)}
            />
          </>
        );

      case 'circle':
        // 图/表锚点：小圆点（在标题/图表旁）
        const radius = styleConfig.radius;
        const circleX = isPageLevel ? x + radius : x + width + radius + 2;
        const circleY = isPageLevel ? y + radius : y + radius;

        return (
          <>
            <circle
              key={`${anchorId}-circle`}
              cx={circleX}
              cy={circleY}
              r={radius}
              fill={color}
              style={{ cursor: 'pointer' }}
              onClick={(e) => handleAnchorClick(anchor, e)}
            />
            <rect
              key={`${anchorId}-hitbox`}
              x={circleX - radius - 2}
              y={circleY - radius - 2}
              width={radius * 2 + 4}
              height={radius * 2 + 4}
              fill="transparent"
              style={{ cursor: 'pointer' }}
              onClick={(e) => handleAnchorClick(anchor, e)}
            />
          </>
        );

      case 'highlight':
        // claim 锚点：背景高亮条
        return (
          <rect
            x={x}
            y={y}
            width={width}
            height={height}
            fill={color}
            style={{ cursor: 'pointer' }}
            onClick={(e) => handleAnchorClick(anchor, e)}
          />
        );

      default:
        return null;
    }
  };

  /**
   * 渲染锚点详情浮层
   */
  const renderDetailPopup = (marker: any) => {
    const { anchor, x, y, width, height } = marker;
    const isExpanded = expandedAnchorId === marker.anchorId;

    if (!isExpanded) return null;

    // 计算 popup 位置（在标记右侧）
    const popupX = x + width + 8;
    const popupY = y;

    // 检查是否超出右边界
    const wouldOverflowRight = popupX + 200 > (resolvedPage.width * scale);

    return (
      <foreignObject
        x={wouldOverflowRight ? Math.max(0, x - 208) : popupX}
        y={popupY}
        width={200}
        height={300}
      >
        <div
          style={{
            background: 'var(--bg-secondary)',
            border: '1px solid var(--border-color)',
            borderRadius: 6,
            padding: '8px 10px',
            fontSize: 11,
            color: 'var(--text-primary)',
            boxShadow: '0 2px 8px rgba(0,0,0,0.1)',
            maxWidth: 200,
            pointerEvents: 'auto',
          }}
          onClick={(e) => e.stopPropagation()}
        >
          {/* 锚点类型和位置提示 */}
          <div style={{
            fontWeight: 500,
            color: 'var(--color-primary)',
            marginBottom: 4,
            fontSize: 10,
            textTransform: 'uppercase',
            letterSpacing: 0.5,
          }}>
            {anchor.anchor_type || 'paragraph'} · {anchor.location_hint}
          </div>

          {/* 引导文本 */}
          {anchor.guidance && (
            <div style={{
              color: 'var(--text-secondary)',
              marginBottom: 6,
              lineHeight: 1.4,
            }}>
              {anchor.guidance}
            </div>
          )}

          {/* 连接关系 */}
          {anchor.connects_to && (
            <div style={{
              color: 'var(--text-tertiary)',
              fontSize: 10,
              marginBottom: 4,
            }}>
              → {anchor.connects_to}
            </div>
          )}

          {/* 引用计数 */}
          {anchor.reference_count > 0 && (
            <div style={{
              display: 'inline-block',
              marginTop: 4,
              padding: '1px 6px',
              background: 'var(--color-primary-bg)',
              color: 'var(--color-primary)',
              borderRadius: 3,
              fontSize: 9,
            }}>
              被引用 {anchor.reference_count} 次
            </div>
          )}
        </div>
      </foreignObject>
    );
  };

  return (
    <svg
      className="anchor-overlay"
      style={{
        position: 'absolute',
        top: 0,
        left: 0,
        width: '100%',
        height: '100%',
        pointerEvents: 'none',
        zIndex: 5, // 在 CoordinateHighlightOverlay 下方，不影响导航高亮
      }}
    >
      {anchorMarkers.map((marker: any) => (
        <g key={marker.anchorId}>
          {renderMarker(marker)}
          {renderDetailPopup(marker)}
        </g>
      ))}
    </svg>
  );
}

// ========== PropTypes 类型检查 ==========

AnchorOverlay.propTypes = {
  /**
   * 注意力锚点数组
   */
  anchors: PropTypes.arrayOf(PropTypes.shape({
    anchor_type: PropTypes.oneOf(['equation', 'paragraph', 'figure', 'table', 'claim']),
    location_hint: PropTypes.string,
    guidance: PropTypes.string,
    connects_to: PropTypes.string,
    reference_count: PropTypes.number,
    block_id: PropTypes.string,
    page_idx: PropTypes.number,
    pageIdx: PropTypes.number,
    bbox: PropTypes.arrayOf(PropTypes.number),
  })),

  /**
   * PDFium PageInfo 对象
   */
  pageInfo: PropTypes.shape({
    width: PropTypes.number.isRequired,
    height: PropTypes.number.isRequired,
    view: PropTypes.arrayOf(PropTypes.number).isRequired,
    rotate: PropTypes.oneOf([0, 90, 180, 270]).isRequired,
  }).isRequired,

  /**
   * 当前页码索引（0-based）
   */
  pageIdx: PropTypes.number.isRequired,
  /**
   * 当前缩放比例
   */
  scale: PropTypes.number.isRequired,
  /**
   * 锚点点击回调
   */
  onAnchorClick: PropTypes.func,
};

/**
 * 默认导出：AnchorOverlay 组件
 */
export default AnchorOverlay;
