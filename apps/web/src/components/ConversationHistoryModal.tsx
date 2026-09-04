/**
 * ConversationHistoryModal.jsx
 * Autonomics 对话历史弹窗组件
 *
 * 用于 /resume 斜杠命令，展示当前论文的所有历史对话列表。
 * 使用 NiceModal 实现命令式调用。
 */

import React, { useState, useEffect, useRef, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { Modal, List, Empty, Flex } from 'antd';
import { DeleteOutlined, MessageOutlined, ClockCircleOutlined } from '@ant-design/icons';
import NiceModal, { useModal } from '@ebay/nice-modal-react';

interface Conversation {
  id: string;
  title: string;
  is_active: boolean;
  message_count: number;
  created_at: number;
  updated_at: number;
}

interface ConversationHistoryModalProps {
  conversations: Conversation[];
  onResume: (id: string) => void;
  onDelete: (id: string) => void;
}

const ConversationHistoryModal = NiceModal.create(({
  conversations,
  onResume,
  onDelete,
}: ConversationHistoryModalProps) => {
  const { t } = useTranslation('common');
  const modal = useModal();
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; convId: string } | null>(null);
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (modal.visible) {
      setSelectedIndex(-1);
    }
  }, [modal.visible]);

  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if (!modal.visible || conversations.length === 0) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSelectedIndex((prev) => (prev < conversations.length - 1 ? prev + 1 : 0));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSelectedIndex((prev) => (prev > 0 ? prev - 1 : conversations.length - 1));
    } else if (e.key === 'Enter' && selectedIndex >= 0) {
      e.preventDefault();
      const conv = conversations[selectedIndex];
      if (conv && !conv.is_active) {
        onResume(conv.id);
      } else if (conv && conv.is_active) {
        modal.hide();
      }
    }
  }, [modal.visible, conversations, selectedIndex, onResume]);

  useEffect(() => {
    if (selectedIndex < 0 || !listRef.current) return;
    const items = listRef.current.querySelectorAll('.ant-list-item');
    if (items[selectedIndex]) {
      items[selectedIndex].scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex]);

  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  const formatDate = (ts: number) => {
    const d = new Date(ts * 1000);
    const dateStr = d.toLocaleDateString();
    const timeStr = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    return dateStr + ' ' + timeStr;
  };

  return (
    <Modal
      title={t('conversationHistory.title')}
      open={modal.visible}
      onCancel={() => modal.hide()}
      closable={false}
      footer={null}
      destroyOnHidden
      centered
      width={520}
      getContainer={false}
      wrapClassName="ai-chat-modal"
    >
      {conversations.length === 0 ? (
        <Empty description={t('conversationHistory.noHistory')} />
      ) : (
        <div style={{ maxHeight: '60vh', overflowY: 'auto' }} ref={listRef}>
          <List
            dataSource={conversations}
            renderItem={(conv, index) => (
              <List.Item
                key={conv.id}
                style={{
                  cursor: conv.is_active ? 'default' : 'pointer',
                  backgroundColor: selectedIndex === index
                    ? 'var(--bg-row-hover, #eaeaea)'
                    : 'transparent',
                  borderRadius: 6,
                  padding: '8px 12px',
                  marginBottom: 4,
                  transition: 'background-color 0.2s',
                }}
                onMouseEnter={() => setSelectedIndex(index)}
                onMouseLeave={() => setSelectedIndex(-1)}
                onClick={() => {
                  if (!conv.is_active) {
                    onResume(conv.id);
                  }
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  setContextMenu({ x: e.clientX, y: e.clientY, convId: conv.id });
                }}
              >
                <List.Item.Meta
                  title={
                    <Flex align="center" gap={6}>
                      <MessageOutlined style={{ fontSize: 12, color: 'var(--text-tertiary)' }} />
                      <span style={{ fontWeight: conv.is_active ? 600 : 400 }}>
                        {conv.title || t('conversationHistory.untitled')}
                      </span>
                      {conv.is_active && (
                        <span style={{
                          fontSize: 11,
                          color: 'var(--color-primary)',
                          border: '1px solid var(--color-primary)',
                          borderRadius: 4,
                          padding: '0 4px',
                        }}>
                          当前
                        </span>
                      )}
                    </Flex>
                  }
                  description={
                    <Flex gap={12} style={{ fontSize: 12, color: 'var(--text-tertiary)' }}>
                      <span><ClockCircleOutlined /> {formatDate(conv.created_at)}</span>
                      <span>{t('conversationHistory.messageCount', { count: conv.message_count })}</span>
                    </Flex>
                  }
                />
              </List.Item>
            )}
          />
        </div>
      )}
      {contextMenu && (
        <>
          <div
            style={{ position: 'fixed', inset: 0, zIndex: 1049 }}
            onClick={() => setContextMenu(null)}
          />
          <div
            style={{
              position: 'fixed',
              left: contextMenu.x,
              top: contextMenu.y,
              zIndex: 1050,
              background: 'var(--bg-primary)',
              border: '1px solid var(--border-color, rgba(0, 0, 0, 0.08))',
              borderRadius: 6,
              boxShadow: '0 2px 8px rgba(0, 0, 0, 0.15)',
              padding: '4px 0',
              minWidth: 140,
              cursor: 'pointer',
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <div
              style={{
                padding: '6px 12px',
                color: 'var(--danger-text)',
                fontSize: 13,
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                transition: 'background-color 0.2s',
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.backgroundColor = 'var(--bg-row-hover, rgba(0, 0, 0, 0.04))';
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.backgroundColor = 'transparent';
              }}
              onClick={() => {
                onDelete(contextMenu.convId);
                setContextMenu(null);
              }}
            >
              <DeleteOutlined /> {t('conversationHistory.deleteConversation')}
            </div>
          </div>
        </>
      )}
    </Modal>
  );
});

export default ConversationHistoryModal;
