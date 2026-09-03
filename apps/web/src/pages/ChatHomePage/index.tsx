/**
 * AI Chat 首页
 *
 * 独立的 AI 对话页面，不绑定任何论文，用于通用 AI 对话场景。
 * 功能特性：
 * - 全屏 ChatPanel：使用 STANDALONE_CHAT_ID 作为会话标识
 * - 快捷导航：提供跳转到论文列表的按钮
 * - 无上下文约束：对话不关联特定论文，适合通用问答
 *
 * 设计说明：
 * - 使用 React.memo 优化渲染性能
 * - 导航按钮使用 useCallback 缓存回调函数
 *
 * @module ChatHomePage
 */
import React, { useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import ChatPanel from '../../features/ai-chat/components/ChatPanel';
import { STANDALONE_CHAT_ID } from '../../features/ai-chat/utils/chatConstants';

export default function ChatHomePage() {
  const navigate = useNavigate();

  const handleNavigateToPapers = useCallback(() => {
    navigate('/papers');
  }, [navigate]);

  return (
    <div style={{
      height: '100%',
      display: 'flex',
      flexDirection: 'column',
      background: 'var(--bg-primary)',
      overflow: 'hidden',
    }}>
      <main style={{
        flex: 1,
        overflow: 'hidden',
        display: 'flex',
        justifyContent: 'center',
      }}>
        <div style={{
          width: '100%',
          maxWidth: 860,
          height: '100%',
        }}>
          <ChatPanel
            agentType="homepage"
            paperId={STANDALONE_CHAT_ID}
            collapsed={false}
            onToggleCollapse={() => {}}
            chatSearchKeyword=""
            onChatSearchClear={() => {}}
            onNavigateToPapers={handleNavigateToPapers}
          />
        </div>
      </main>
    </div>
  );
}
