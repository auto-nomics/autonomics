/**
 * JayRead 核心主张 + 核心概览组件
 *
 * 预读阶段组件，在导览面板最顶部以紧凑卡片形式呈现：
 * - 核心主张（core_claim）：一句话，带 🎯 图标，醒目样式
 * - 核心概览（core_overview）：200-300 字，次要文本颜色，限制最大行数
 *
 * 设计原则：
 * - 读完即走，不诱导读者停留
 * - 不作为独立面板，而是导览面板顶部的紧凑卡片
 * - 仅提供凭标题和摘要无法获得的判断依据
 *
 * 参考文档：GUIDE_READING_EXPERIENCE_REDESIGN.md 3.1.2 节
 *
 * @module CoreOverview
 */

import React, { useState } from 'react'; // 导入 React 核心库
import PropTypes from 'prop-types'; // 导入 PropTypes 类型检查库
import { Tooltip } from 'antd'; // 导入 Ant Design 组件：Tooltip 提示框
import {
  InfoCircleOutlined, // 信息图标（提示）
} from '@ant-design/icons'; // 导入 Ant Design 图标

/**
 * 核心主张 + 核心概览组件
 *
 * @param {object} props - 组件属性
 * @param {string} props.coreClaim - 核心主张（一句话）
 * @param {string} props.coreOverview - 核心概览（200-300 字）
 * @param {boolean} [props.compact] - 是否使用紧凑模式（默认 false）
 */
function CoreOverview({
  coreClaim,
  coreOverview,
  compact = false,
}: {
  coreClaim: any;
  coreOverview: any;
  compact?: boolean;
}) {
  /**
   * 概览展开状态
   *
   * 默认折叠，点击后展开显示完整概览。
   */
  const [expanded, setExpanded] = useState(false);
  const [claimExpanded, setClaimExpanded] = useState(false);

  /**
   * 如果没有核心主张和概览，不渲染组件
   */
  if (!coreClaim && !coreOverview) {
    return null;
  }

  /**
   * 计算概览文本是否需要折叠
   *
   * 超过 150 字符时默认折叠
   */
  const shouldCollapse = coreOverview && coreOverview.length > 150 && !expanded;

  /**
   * 获取显示的概览文本
   */
  const displayOverview = shouldCollapse
    ? coreOverview.slice(0, 150) + '...'
    : coreOverview;

  return (
    <div
      style={{
        position: 'relative',
        margin: '0 8px 12px 8px',
        padding: compact ? '10px 12px' : '12px 14px',
        paddingRight: 28, // 为提示图标留出空间
        background: 'var(--bg-secondary)',
        border: '1px solid var(--border-color)',
        borderRadius: 6,
        boxShadow: 'var(--shadow-sm)',
      }}
    >
      {/**
       * 核心主张（一句话）
       *
       * 使用 🎯 图标前缀，醒目样式，一行显示
       */}
      {coreClaim && (
        <div
          onClick={() => setClaimExpanded(!claimExpanded)}
          style={{
            display: 'flex',
            alignItems: 'flex-start',
            gap: 6,
            marginBottom: coreOverview ? 8 : 0,
            cursor: 'pointer',
            borderRadius: 4,
            margin: '-2px -4px',
            padding: '2px 4px',
            transition: 'background 0.15s',
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.background = 'var(--overlay-hover)';
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = 'transparent';
          }}
        >
          <span
            style={{
              fontSize: compact ? 13 : 14,
              flexShrink: 0,
              lineHeight: '1.4',
            }}
          >
            🎯
          </span>
          <span
            style={{
              fontSize: compact ? 12 : 13,
              fontWeight: 500,
              color: 'var(--text-primary)',
              lineHeight: '1.4',
              ...(!claimExpanded && {
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                display: '-webkit-box',
                WebkitLineClamp: 2,
                WebkitBoxOrient: 'vertical',
              }),
            }}
          >
            {coreClaim}
          </span>
        </div>
      )}

      {/**
       * 核心概览（200-300 字）
       *
       * 次要文本颜色，限制最大行数，可展开
       */}
      {coreOverview && (
        <div
          style={{
            display: 'flex',
            alignItems: 'flex-start',
            gap: 6,
          }}
        >
          <span
            style={{
              fontSize: compact ? 12 : 13,
              flexShrink: 0,
              marginTop: 1,
              lineHeight: '1.4',
            }}
          >
            💡
          </span>
          <div
            style={{
              flex: 1,
              fontSize: compact ? 11 : 12,
              color: 'var(--text-secondary)',
              lineHeight: '1.5',
            }}
          >
            <span style={{ fontWeight: 500, color: 'var(--text-tertiary)', marginRight: 4 }}>
              核心概览
            </span>
            <span>{displayOverview}</span>

            {/**
             * 展开/收起按钮
             *
             * 当概览超过 150 字符时显示
             */}
            {coreOverview.length > 150 && (
              <button
                onClick={() => setExpanded(!expanded)}
                style={{
                  background: 'none',
                  border: 'none',
                  color: 'var(--color-primary)',
                  fontSize: 11,
                  cursor: 'pointer',
                  padding: 0,
                  marginLeft: 4,
                }}
                onMouseEnter={(e) => {
                  e.currentTarget.style.textDecoration = 'underline';
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.textDecoration = 'none';
                }}
              >
                {expanded ? '收起' : '展开'}
              </button>
            )}
          </div>
        </div>
      )}

      {/**
       * 提示图标
       *
       * 鼠标悬停时显示说明
       */}
      <Tooltip title="核心主张和概览帮助你快速判断论文的价值和贡献份量">
        <InfoCircleOutlined style={{
          position: 'absolute',
          top: 8,
          right: 8,
          fontSize: 12,
          color: 'var(--text-tertiary)',
          cursor: 'help',
        }} />
      </Tooltip>
    </div>
  );
}

// ========== PropTypes 类型检查 ==========

/**
 * 组件属性类型检查
 */
CoreOverview.propTypes = {
  /**
   * 核心主张（一句话）
   */
  coreClaim: PropTypes.string,

  /**
   * 核心概览（200-300 字）
   */
  coreOverview: PropTypes.string,

  /**
   * 是否使用紧凑模式
   */
  compact: PropTypes.bool,
};

/**
 * 默认导出：CoreOverview 组件，使用 React.memo 优化渲染性能
 */
export default React.memo(CoreOverview);
