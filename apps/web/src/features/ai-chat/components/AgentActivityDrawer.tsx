/**
 * AgentActivityDrawer — P5a 代理活动抽屉（docs/design/web-agent-delegation.md §6）
 * ============================================================
 *
 * 只读观测面：AI 面板的「活动」入口（ChatPanel，仅 runtime 模式渲染）打开，
 * 展示 host 全景——
 * - Agents：活跃 agent（TUI 侧 + 常驻 web 三剑客 + 派生 child）+ 上次运行
 *   遗留的持久化图行（live:false）；行点击展开该 agent 的近期转写
 *   （/agents/history，{role,text} 同 thread messages 形状）
 * - Delegations：delegation 台账（五态、任务、目标、更新时间）
 *
 * 无 live 推送（D3 一期先无 live，刷新拉取）；所有请求失败显示错误态 +
 * 重试按钮，不打断聊天主链路。
 */

import { useCallback, useEffect, useState } from 'react';
import { Button, Drawer, Empty, Spin, Tag, Tooltip } from 'antd';
import { ReloadOutlined } from '@ant-design/icons';
import {
  fetchAgentHistory,
  fetchAgents,
  fetchDelegations,
  type AgentHistory,
  type AgentRow,
  type DelegationRow,
} from '../api/agentActivityApi';

/** 状态 → Tag 颜色（未知状态落 default） */
const STATUS_COLORS: Record<string, string> = {
  idle: 'default',
  running: 'processing',
  awaiting_tool: 'warning',
  completed: 'success',
  failed: 'error',
  pending: 'default',
  interrupted: 'warning',
  unknown: 'default',
};

function statusTag(status: string): React.ReactNode {
  return <Tag color={STATUS_COLORS[status] ?? 'default'}>{status}</Tag>;
}

/** 相对时间（台账 updated_at 是 unix 毫秒） */
function relativeTime(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return '刚刚';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  return `${Math.floor(diff / 86_400_000)} 天前`;
}

interface AgentRowWithHistory {
  row: AgentRow;
  history: AgentHistory | null;
  loading: boolean;
}

interface AgentActivityDrawerProps {
  open: boolean;
  onClose: () => void;
}

export default function AgentActivityDrawer({ open, onClose }: AgentActivityDrawerProps) {
  const [agents, setAgents] = useState<AgentRow[] | null>(null);
  const [delegations, setDelegations] = useState<DelegationRow[] | null>(null);
  const [loading, setLoading] = useState(false);
  /** path → 展开的转写（null = 加载失败或空） */
  const [expanded, setExpanded] = useState<Record<string, AgentRowWithHistory>>({});

  const refresh = useCallback(async () => {
    setLoading(true);
    // 两个视图独立失败：一个 null 不拖垮另一个
    const [nextAgents, nextDelegations] = await Promise.all([fetchAgents(), fetchDelegations()]);
    setAgents(nextAgents);
    setDelegations(nextDelegations);
    setLoading(false);
  }, []);

  // 打开时拉取（手动刷新兜底）；关闭不清数据，重开先显示旧数据再刷新
  useEffect(() => {
    if (open) void refresh();
  }, [open, refresh]);

  const toggleExpand = useCallback(
    async (row: AgentRow) => {
      const isOpen = Boolean(expanded[row.path]);
      if (isOpen) {
        const next = { ...expanded };
        delete next[row.path];
        setExpanded(next);
        return;
      }
      // 乐观展开 + 转写加载
      setExpanded((prev) => ({ ...prev, [row.path]: { row, history: null, loading: true } }));
      const history = await fetchAgentHistory(row.path, 20);
      setExpanded((prev) => ({ ...prev, [row.path]: { row, history, loading: false } }));
    },
    [expanded],
  );

  const agentsError = agents === null;
  const delegationsError = delegations === null;

  return (
    <Drawer
      title="代理活动"
      placement="right"
      width={480}
      open={open}
      onClose={onClose}
      extra={
        <Tooltip title="刷新">
          <Button type="text" icon={<ReloadOutlined spin={loading} />} onClick={() => void refresh()} />
        </Tooltip>
      }
      styles={{ body: { padding: '12px 16px' } }}
    >
      {/* ===== Agents ===== */}
      <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--text-primary)', margin: '4px 0 8px' }}>
        Agents（{agents?.length ?? '—'}）
      </div>
      {agentsError ? (
        <div style={{ color: 'var(--text-secondary)', fontSize: 12, padding: '8px 0' }}>
          加载失败（后端不可用或非 runtime 模式）
        </div>
      ) : loading && !agents ? (
        <Spin size="small" />
      ) : agents && agents.length > 0 ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {agents.map((row) => {
            const expandState = expanded[row.path];
            return (
              <div
                key={row.path}
                style={{
                  border: '1px solid var(--border-color, #d9d9d9)',
                  borderRadius: 6,
                  padding: '6px 10px',
                  background: 'var(--bg-secondary)',
                }}
              >
                <div
                  role="button"
                  tabIndex={0}
                  onClick={() => void toggleExpand(row)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') void toggleExpand(row);
                  }}
                  style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer' }}
                >
                  <span style={{ fontWeight: 600, fontSize: 13 }}>{row.name}</span>
                  {statusTag(row.status)}
                  {!row.live && <Tag>历史</Tag>}
                  <span
                    style={{
                      flex: 1,
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                      fontSize: 11,
                      color: 'var(--text-secondary)',
                    }}
                    title={row.path}
                  >
                    {row.path}
                  </span>
                  <span style={{ fontSize: 11, color: 'var(--text-secondary)' }}>
                    {expandState ? '▾' : '▸'}
                  </span>
                </div>
                {row.last_event && (
                  <div style={{ fontSize: 11, color: 'var(--text-secondary)', marginTop: 2 }}>
                    {row.last_event}
                  </div>
                )}
                {expandState && (
                  <div style={{ borderTop: '1px solid var(--border-color, #f0f0f0)', marginTop: 6, paddingTop: 6 }}>
                    {expandState.loading ? (
                      <Spin size="small" />
                    ) : !expandState.history || expandState.history.messages.length === 0 ? (
                      <div style={{ fontSize: 12, color: 'var(--text-secondary)' }}>（无转写记录）</div>
                    ) : (
                      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                        {expandState.history.messages.map((m, i) => (
                          <div key={i} style={{ fontSize: 12 }}>
                            <span
                              style={{
                                fontWeight: 600,
                                color: m.role === 'user' ? 'var(--text-primary)' : '#1677ff',
                                marginRight: 6,
                              }}
                            >
                              {m.role === 'user' ? '用户' : '助手'}:
                            </span>
                            <span style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>{m.text}</span>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无 agent" style={{ margin: '8px 0' }} />
      )}

      {/* ===== Delegations ===== */}
      <div
        style={{
          fontSize: 13,
          fontWeight: 600,
          color: 'var(--text-primary)',
          margin: '16px 0 8px',
        }}
      >
        Delegations（{delegations?.length ?? '—'}）
      </div>
      {delegationsError ? (
        <div style={{ color: 'var(--text-secondary)', fontSize: 12, padding: '8px 0' }}>加载失败</div>
      ) : delegations && delegations.length > 0 ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {delegations.map((d) => (
            <div
              key={d.delegation_id}
              style={{
                border: '1px solid var(--border-color, #d9d9d9)',
                borderRadius: 6,
                padding: '6px 10px',
                background: 'var(--bg-secondary)',
              }}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                {statusTag(d.status)}
                <span style={{ fontSize: 12, flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={d.task}>
                  {d.task}
                </span>
                <span style={{ fontSize: 11, color: 'var(--text-secondary)', flexShrink: 0 }}>
                  {relativeTime(d.updated_at)}
                </span>
              </div>
              <div style={{ fontSize: 11, color: 'var(--text-secondary)', marginTop: 2 }}>
                {(d.caller_path ?? '（宿主）')} → {d.target_path}
              </div>
              {d.response && (
                <div
                  style={{
                    fontSize: 11,
                    color: 'var(--text-secondary)',
                    marginTop: 2,
                    display: '-webkit-box',
                    WebkitLineClamp: 2,
                    WebkitBoxOrient: 'vertical',
                    overflow: 'hidden',
                  }}
                  title={d.response}
                >
                  {d.response}
                </div>
              )}
            </div>
          ))}
        </div>
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无委派记录" style={{ margin: '8px 0' }} />
      )}
    </Drawer>
  );
}
