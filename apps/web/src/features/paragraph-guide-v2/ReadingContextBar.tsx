/**
 * JayRead 滚动上下文条组件
 *
 * Layer 2 伴读模式的核心组件，固定在 PDF 视图顶部（或底部）。
 *
 * 核心功能：
 * 1. 随 PDF 滚动自动更新当前章节信息
 * 2. 收起状态：1-2 行，显示章节名 + 论证角色 + 最多 3 个锚点标签
 * 3. 展开状态：完整阅读引导面板
 * 4. 非侵入性：收起时只占 1-2 行高度，不遮挡 PDF
 *
 * 参考文档：GUIDE_READING_EXPERIENCE_REDESIGN.md 3.3.2-3.3.3 节
 *
 * @module ReadingContextBar
 */

import React, { useState, useCallback } from 'react'; // 导入 React 核心库
import PropTypes from 'prop-types'; // 导入 PropTypes 类型检查库
import { Tooltip } from 'antd'; // 导入 Ant Design 组件：Tooltip 提示框
import {
  InfoCircleOutlined, // 信息图标（阅读策略）
  QuestionCircleOutlined, // 问题图标（批判性思考）
  ForwardOutlined, // 跳过图标
  ThunderboltOutlined, // 锚点图标
  DownOutlined, // 展开图标
  UpOutlined, // 收起图标
} from '@ant-design/icons'; // 导入 Ant Design 图标
import styles from './ReadingContextBar.module.css'; // 导入样式模块

/**
 * 滚动上下文条组件
 *
 * @param {object} props - 组件属性
 * @param {object} props.currentSection - 当前章节节点数据
 * @param {object} props.currentGuide - 当前章节的阅读引导（已解析）
 * @param {Array} props.anchors - 当前章节的注意力锚点（已解析）
 * @param {Function} [props.onAnchorClick] - 锚点点击回调
 * @param {boolean} [props.compact] - 是否使用紧凑模式（默认 false）
 */
function ReadingContextBar({
  currentSection,
  currentGuide,
  anchors,
  onAnchorClick,
  compact = false,
}: {
  currentSection: any;
  currentGuide: any;
  anchors: any;
  onAnchorClick: any;
  compact?: boolean;
}) {
  /**
   * 展开/收起状态
   *
   * 默认收起，用户点击后展开显示完整引导信息。
   */
  const [expanded, setExpanded] = useState(false);

  /**
   * 处理锚点点击事件
   *
   * 注意：所有 hooks 必须在 early return 之前调用，
   * 否则 React 会报 "Rendered fewer hooks than expected" 错误。
   */
  const handleAnchorClick = useCallback((anchor: any) => {
    if (onAnchorClick) {
      onAnchorClick(anchor);
    }
  }, [onAnchorClick]);

  /**
   * 如果没有当前章节数据，不渲染组件
   */
  if (!currentSection) {
    return null;
  }

  /**
   * 解析 whyExists 字段
   */
  const whyExists = currentSection.whyExists || null;

  /**
   * 解析批判性思考提示
   *
   * 从 currentGuide 或 currentSection.criticalHints 中提取。
   */
  const criticalHints = currentGuide?.critical_hints || [];
  let parsedCriticalHints = [];
  try {
    if (currentSection.criticalHints) {
      parsedCriticalHints = typeof currentSection.criticalHints === 'string'
        ? JSON.parse(currentSection.criticalHints)
        : currentSection.criticalHints || [];
    }
  } catch {
    parsedCriticalHints = [];
  }
  const hints = [...criticalHints, ...parsedCriticalHints];

  /**
   * 收起状态：显示章节名 + 论证角色 + 锚点标签
   */
  if (!expanded) {
    return (
      <div
        className={`${styles.collapsedContainer} ${compact ? styles.collapsedContainerCompact : ''}`}
        onClick={() => setExpanded(true)}
      >
        {/**
         * 章节名称
         */}
        <span className={styles.sectionTitle}>
          {currentSection.title}
        </span>

        {/**
         * 论证位置（argument_context）
         */}
        {currentSection.argumentContext && (
          <span className={styles.argumentContextText}>
            · {currentSection.argumentContext}
          </span>
        )}

        {/**
         * 锚点标签（最多 3 个）
         */}
        {anchors && anchors.length > 0 && (
          <div className={styles.anchorTagsContainer}>
            {anchors.slice(0, 3).map((anchor: any, index: any) => (
              <Tooltip
                key={index}
                title={
                  <div style={{ fontSize: 11 }}>
                    <div style={{ fontWeight: 500, marginBottom: 2 }}>
                      {anchor.guidance || '查看详情'}
                    </div>
                    {anchor.connects_to && (
                      <div style={{ opacity: 0.8 }}>
                        → {anchor.connects_to}
                      </div>
                    )}
                    {anchor.reference_count > 0 && (
                      <div style={{ opacity: 0.6, marginTop: 2 }}>
                        被引用 {anchor.reference_count} 次
                      </div>
                    )}
                  </div>
                }
              >
                <span
                  className={styles.anchorTag}
                  onClick={(e) => {
                    e.stopPropagation();
                    handleAnchorClick(anchor);
                  }}
                >
                  <ThunderboltOutlined className={styles.anchorTagIcon} />
                  {anchor.location_hint}
                  {anchor.reference_count > 0 && (
                    <span className={styles.anchorTagCount}>
                      {anchor.reference_count}
                    </span>
                  )}
                </span>
              </Tooltip>
            ))}
            {anchors.length > 3 && (
              <span className={styles.moreAnchors}>
                +{anchors.length - 3}
              </span>
            )}
          </div>
        )}

        {/**
         * 展开提示
         */}
        <span className={styles.expandHint}>
          展开详情 <DownOutlined className={styles.expandHintIcon} />
        </span>
      </div>
    );
  }

  /**
   * 展开状态：显示完整阅读引导
   */
  return (
    <div className={styles.expandedContainer}>
      {/**
       * 头部：章节名 + 收起按钮
       */}
      <div className={styles.expandedHeader}>
        <div className={styles.expandedHeaderLeft}>
          <span className={styles.expandedSectionTitle}>
            {currentSection.title}
          </span>
          {currentSection.argumentContext && (
            <span className={styles.argumentContextWithIcon}>
              📍 {currentSection.argumentContext}
            </span>
          )}
        </div>
        <button
          onClick={() => setExpanded(false)}
          className={styles.collapseButton}
        >
          收起 <UpOutlined className={styles.collapseButtonIcon} />
        </button>
      </div>

      {/**
       * 内容区域：阅读策略 + 锚点详情 + 批判性思考
       */}
      <div className={styles.expandedContent}>
        {/**
         * 章节角色描述
         */}
        {currentSection.sectionRole && (
          <div className={styles.sectionRole}>
            <span className={styles.sectionRoleLabel}>
              章节：
            </span>
            <span className={styles.sectionRoleText}>
              {currentSection.sectionRole}
            </span>
          </div>
        )}

        {/**
         * 章节存在原因
         */}
        {whyExists && (
          <div className={styles.whyExists}>
            <div className={styles.whyExistsHeader}>
              <span className={styles.whyExistsIcon}>💡</span>
              <span className={styles.whyExistsLabel}>为什么有这一节</span>
            </div>
            <div className={styles.whyExistsContent}>
              {whyExists}
            </div>
          </div>
        )}

        {/**
         * 阅读策略建议
         */}
        {currentGuide?.reading_strategy && (
          <div className={styles.readingStrategy}>
            <div className={styles.readingStrategyHeader}>
              <InfoCircleOutlined className={styles.readingStrategyIcon} />
              <span className={styles.readingStrategyLabel}>阅读策略</span>
            </div>
            <div className={styles.readingStrategyContent}>
              {currentGuide.reading_strategy}
            </div>
          </div>
        )}

        {/**
         * 注意力锚点详情
         */}
        {anchors && anchors.length > 0 && (
          <div className={styles.anchorsDetail}>
            <div className={styles.anchorsDetailLabel}>
              注意力锚点
            </div>
            <div className={styles.anchorsGrid}>
              {anchors.map((anchor: any, index: any) => (
                <div
                  key={index}
                  onClick={() => handleAnchorClick(anchor)}
                  className={styles.anchorCard}
                >
                  <div className={styles.anchorCardHeader}>
                    <ThunderboltOutlined className={styles.anchorCardIcon} />
                    {anchor.location_hint}
                    {anchor.reference_count > 0 && (
                      <span className={styles.anchorCardCount}>
                        {anchor.reference_count} 次引用
                      </span>
                    )}
                  </div>
                  <div className={styles.anchorCardGuidance}>
                    {anchor.guidance}
                  </div>
                  {anchor.connects_to && (
                    <div className={styles.anchorCardConnects}>
                      → {anchor.connects_to}
                    </div>
                  )}
                </div>
              ))}
            </div>
          </div>
        )}

        {/**
         * 批判性思考提示
         */}
        {hints.length > 0 && (
          <div className={styles.criticalHints}>
            <div className={styles.criticalHintsLabel}>
              批判性思考
            </div>
            {hints.map((hint, index) => (
              <div key={index} className={styles.criticalHintItem}>
                <QuestionCircleOutlined className={styles.criticalHintIcon} />
                <span className={styles.flex1}>{hint}</span>
              </div>
            ))}
          </div>
        )}

        {/**
         * 可跳过提示
         */}
        {currentSection.skipHint && (
          <div className={styles.skipHint}>
            <ForwardOutlined className={styles.skipHintIcon} />
            <div>
              <span className={styles.skipHintLabel}>可跳过：</span>
              <span>{currentSection.skipHint}</span>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// ========== PropTypes 类型检查 ==========

/**
 * 组件属性类型检查
 */
ReadingContextBar.propTypes = {
  /**
   * 当前章节节点数据
   */
  currentSection: PropTypes.shape({
    id: PropTypes.string,
    title: PropTypes.string,
    pageIdx: PropTypes.number,
    sectionRole: PropTypes.string,
    argumentContext: PropTypes.string,
    skipHint: PropTypes.string,
    criticalHints: PropTypes.string,
    attentionAnchors: PropTypes.string,
    bboxes: PropTypes.array,
    whyExists: PropTypes.string,
  }),
  /**
   * 当前章节的阅读引导（已解析）
   */
  currentGuide: PropTypes.shape({
    reading_strategy: PropTypes.string,
    critical_hints: PropTypes.array,
  }),
  /**
   * 当前章节的注意力锚点（已解析）
   */
  anchors: PropTypes.array,
  /**
   * 锚点点击回调
   */
  onAnchorClick: PropTypes.func,
  /**
   * 是否使用紧凑模式
   */
  compact: PropTypes.bool,
};

/**
 * 默认导出：ReadingContextBar 组件
 */
export default ReadingContextBar;
