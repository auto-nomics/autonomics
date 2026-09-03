/**
 * ToolCallCard 组件
 *
 * AI 聊天中的工具调用卡片
 * 在助手消息中渲染单个工具调用的状态和信息
 * 功能：
 * - 显示工具名称和执行状态（执行中/完成/错误）
 * - 可展开/收起查看工具参数和结果
 * - 不同状态显示不同图标和颜色
 */

import { useState } from 'react';
import { LoadingOutlined, CheckCircleOutlined, CloseCircleOutlined, WarningOutlined } from '@ant-design/icons';
import type { ReactNode } from 'react';

// 工具调用数据结构
interface ToolCall {
  id?: string;           // 工具调用唯一标识
  name: string;          // 工具名称
  status: 'running' | 'completed' | 'error' | 'abandoned';  // 执行状态
  input?: unknown;       // 工具输入参数
  result?: unknown;      // 工具执行结果
}

// 状态配置：图标、颜色、标签
const statusConfig: Record<ToolCall['status'], { icon: ReactNode; color: string; label: string }> = {
  running: { icon: <LoadingOutlined spin />, color: 'var(--color-primary)', label: '执行中' },
  completed: { icon: <CheckCircleOutlined />, color: 'var(--status-success)', label: '完成' },
  error: { icon: <CloseCircleOutlined />, color: 'var(--text-error)', label: '错误' },
  abandoned: { icon: <WarningOutlined />, color: 'var(--status-warning, #faad14)', label: '已中止' },
};

export default function ToolCallCard({ toolCall }: { toolCall: ToolCall }) {
  const [expanded, setExpanded] = useState(false);  // 展开状态
  const { name, status, input, result } = toolCall;
  const cfg = statusConfig[status] || statusConfig.running;

  return (
    <div
      style={{
        margin: '6px 0',
        border: `1px solid var(--border-color, #e8e8e8)`,
        borderRadius: 6,
        overflow: 'hidden',
        fontSize: 12,
      }}
    >
      {/* 卡片头部：始终显示，点击可展开/收起 */}
      <div
        onClick={() => setExpanded(!expanded)}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          padding: '4px 8px',
          background: 'var(--bg-secondary, #fafafa)',
          cursor: 'pointer',
          userSelect: 'none',
        }}
      >
        {/* 状态图标 */}
        <span style={{ color: cfg.color }}>{cfg.icon}</span>
        {/* 工具名称 */}
        <code style={{ fontWeight: 500, fontSize: 12 }}>{name}</code>
        {/* 状态标签 */}
        <span style={{ color: cfg.color, fontSize: 11 }}>{cfg.label}</span>
        {/* 展开/收起指示器 */}
        <span style={{ marginLeft: 'auto', color: 'var(--text-tertiary, #8c8c8c)', fontSize: 10 }}>
          {expanded ? '▼' : '▶'}
        </span>
      </div>

      {/* 展开内容区域：仅在展开时显示 */}
      {expanded && (
        <div style={{ padding: '6px 8px', borderTop: `1px solid var(--border-color, #e8e8e8)` }}>
          {/* 输入参数 */}
          {!!input && (
            <div style={{ marginBottom: 4 }}>
              <div style={{ color: 'var(--text-tertiary, #8c8c8c)', marginBottom: 2 }}>参数:</div>
              <pre
                style={{
                  margin: 0,
                  padding: '4px 6px',
                  background: 'var(--bg-tertiary, #f0f0f0)',
                  borderRadius: 4,
                  fontSize: 11,
                  maxHeight: 120,
                  overflow: 'auto',
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-all',
                }}
              >
                {typeof input === 'string' ? String(input) : String(JSON.stringify(input, null, 2))}
              </pre>
            </div>
          )}
          {/* 执行结果 */}
          {!!result && (
            <div>
              <div style={{ color: 'var(--text-tertiary, #8c8c8c)', marginBottom: 2 }}>结果:</div>
              <pre
                style={{
                  margin: 0,
                  padding: '4px 6px',
                  background: status === 'error'
                    ? 'rgba(255,77,79,0.06)'  // 错误状态使用淡红色背景
                    : status === 'abandoned'
                      ? 'rgba(250,173,20,0.08)'  // 已中止使用淡黄色背景
                      : 'var(--bg-tertiary, #f0f0f0)',
                  borderRadius: 4,
                  fontSize: 11,
                  maxHeight: 200,
                  overflow: 'auto',
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-all',
                  borderLeft: status === 'error'
                    ? '2px solid var(--text-error)'
                    : status === 'abandoned'
                      ? `2px solid var(--status-warning, #faad14)`
                      : 'none',
                }}
              >
                {typeof result === 'string' ? String(result) : String(JSON.stringify(result, null, 2))}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
