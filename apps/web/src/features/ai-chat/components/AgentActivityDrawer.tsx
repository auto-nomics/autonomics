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
 * P5c-6 起打开期间订阅 /agents/events SSE（fetch 流式解析，带 bearer）：
 * registered / unregistered / status 帧就地更新行，output 帧追加进展开行的
 * 实时输出缓冲（逐模型轮一帧，见 §11）；断线重连后 sync 帧重拉快照。台账
 * 无实时事件，仍靠手动刷新。所有请求失败显示错误态 + 重试按钮，不打断聊天
 * 主链路。
 */

import { useCallback, useEffect, useState } from 'react';
import { Button, Drawer, Empty, Spin, Tag, Tooltip } from 'antd';
import { ReloadOutlined } from '@ant-design/icons';
import {
  fetchAgentHistory,
  fetchAgents,
  fetchDelegations,
  subscribeAgentEvents,
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

/** 单个 agent 的实时输出缓冲封顶（字符）：长会话下抽屉开着不无限膨胀，
 * 只保留最近的输出（展开区是滚动阅读，不需要全文）。 */
const LIVE_OUTPUT_CAP = 8000;

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
  /** path → 实时输出缓冲（output 帧追加；仅展开行渲染） */
  const [liveOutput, setLiveOutput] = useState<Record<string, string>>({});

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

  // P5c-6：打开期间订阅 host 实时事件（关闭即退订，数据保留）。帧语义：
  // - registered：新行（或把已有行顶成 live）——快照失败时也能从空表起步
  // - unregistered：行移除（shutdown 后服务端视图也没有它；孤儿行靠刷新带回）
  // - status：就地更新状态 + last_event
  // - output：追加进实时输出缓冲（封顶，见 LIVE_OUTPUT_CAP）
  // - sync：断线重连成功——增量流已出现缺口，重拉快照补基线
  useEffect(() => {
    if (!open) return;
    return subscribeAgentEvents((frame) => {
      if (frame.type === 'sync') {
        void refresh();
        return;
      }
      if (frame.type === 'registered') {
        setAgents((prev) => {
          const rows = prev ?? [];
          const idx = rows.findIndex((r) => r.path === frame.path);
          if (idx === -1) {
            return [
              ...rows,
              { name: frame.name, path: frame.path, agent_id: null, status: frame.status, live: true },
            ];
          }
          const next = [...rows];
          next[idx] = { ...next[idx], name: frame.name, status: frame.status, live: true };
          return next;
        });
        return;
      }
      if (frame.type === 'unregistered') {
        setAgents((prev) => (prev ? prev.filter((r) => r.path !== frame.path) : prev));
        return;
      }
      if (frame.type === 'status') {
        setAgents((prev) =>
          prev
            ? prev.map((r) =>
                r.path === frame.path
                  ? { ...r, status: frame.status, last_event: frame.last_event }
                  : r,
              )
            : prev,
        );
        return;
      }
      setLiveOutput((prev) => {
        const merged = (prev[frame.path] ?? '') + frame.text;
        return { ...prev, [frame.path]: merged.slice(-LIVE_OUTPUT_CAP) };
      });
    });
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
                    {liveOutput[row.path] && (
                      <div style={{ marginTop: 6, fontSize: 12 }}>
                        <span style={{ fontWeight: 600, color: '#1677ff', marginRight: 6 }}>实时输出:</span>
                        <span style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                          {liveOutput[row.path]}
                        </span>
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
