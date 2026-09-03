/**
 * AI 聊天会话管理 Hook
 *
 * 负责管理对话会话的生命周期，包括：
 * - 加载对话列表（用于 /resume 命令展示历史对话）
 * - 恢复历史对话（切换到选中的对话会话）
 * - 删除对话会话
 *
 * 从 ChatPanel 组件中提取，使代码结构更清晰，职责更单一。
 *
 * @module ai-chat/hooks/useConversationManager
 */

import { useState, useCallback, useEffect } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // 导入 Ant Design App 组件用于获取 message 上下文
import { getConversations, activateConversation, deleteConversation, getMessages } from '../../../services/chatApi'; // 导入对话管理 API

/**
 * AI 聊天会话管理 Hook
 *
 * @param {object} params - Hook 参数
 * @param {number} params.paperId - 当前论文 ID，用于关联对话记录
 * @param {Function} params.setMessages - 设置消息列表的函数（用于恢复对话后更新消息）
 * @returns {object} 包含会话管理状态和操作函数的对象
 */
export function useConversationManager({
  paperId,
  setMessages,
}: {
  paperId?: string | number;
  setMessages: (msgs: any[]) => void;
}) {
  // 获取 Ant Design App 上下文中的 message 实例
  // 必须使用 App.useApp() 而不是静态导入 { message }，
  // 因为静态方法在 ConfigProvider 外部渲染，无法继承深色主题
  const { message } = App.useApp();

  // ========== 状态定义 ==========

  /**
   * 对话历史弹窗显示状态
   *
   * 控制对话历史弹窗的打开/关闭。
   * 用户输入 /resume 命令后，此状态设为 true，弹出对话历史弹窗。
   *
   * @type {boolean}
   */
  const [resumeModalOpen, setResumeModalOpen] = useState(false);

  /**
   * 对话会话列表
   *
   * 存储当前论文的所有历史对话会话。
   * 每个会话包含：
   * - id: 会话唯一标识
   * - title: 会话标题（通常为首条消息的摘要）
   * - is_active: 是否为当前活跃会话
   * - message_count: 会话中的消息数量
   * - created_at: 会话创建时间
   * - updated_at: 会话最后更新时间
   *
   * 用于 /resume 命令展示历史对话列表。
   *
   * @type {Array<{id: number, title: string, is_active: boolean, message_count: number, created_at: string, updated_at: string}>}
   */
  const [conversations, setConversations] = useState<any[]>([]);

  // ========== 操作函数 ==========

  /**
   * 加载对话列表
   *
   * 从后端获取当前论文的所有对话会话列表，
   * 用于在 /resume 弹窗中展示。
   * 按更新时间倒序排列（最近活跃的排在最前）
   *
   * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
   */
  const loadConversations = useCallback(async (signal?: AbortSignal) => {
    try {
      // 调用后端 API 获取对话列表
      const data = await getConversations(paperId as any, signal);
      // 更新状态，触发弹窗重新渲染
      const list = data.conversations || [];
      setConversations(list);
      return list;
    } catch (err: any) {
      // 忽略 AbortError，只记录其他错误
      if (err.name !== 'AbortError') {
        // 加载失败时显示错误提示
        message.error('加载对话历史失败: ' + err.message);
      }
    }
  }, [paperId]); // 依赖论文 ID

  /**
   * 恢复指定的历史对话
   *
   * 切换到用户在 /resume 弹窗中选择的历史对话：
   * 1. 调用后端 API 将目标对话设为活跃
   * 2. 重新加载活跃对话的消息内容
   * 3. 关闭弹窗
   *
   * @param {number} conversationId - 要恢复的对话会话 ID
   */
  const handleResumeConversation = useCallback(async (conversationId: string | number) => {
    try {
      // 调用后端 API 切换到指定的历史对话
      await activateConversation(paperId as any, conversationId as any);
      // 重新加载活跃对话的消息内容（后端已将目标对话设为活跃）
      const msgData = await getMessages(paperId as any);
      // 更新前端消息列表
      setMessages(msgData.messages || []);
      // 关闭对话历史弹窗
      setResumeModalOpen(false);
      // 显示成功提示
      message.success('已切换到历史对话');
    } catch (err: any) {
      // 切换失败时显示错误提示
      message.error('切换对话失败: ' + err.message);
    }
  }, [paperId, setMessages]); // 依赖论文 ID 和设置消息函数

  /**
   * 删除指定的对话会话
   *
   * 从 /resume 弹窗中删除用户指定的历史对话：
   * 1. 调用后端 API 删除对话及其所有消息
   * 2. 重新加载对话列表（更新弹窗显示）
   * 3. 如果删除的是活跃对话，后端会自动激活另一个对话，
   *    前端需要重新加载消息以反映变化
   *
   * @param {number} conversationId - 要删除的对话会话 ID
   */
  const handleDeleteConversation = useCallback(async (conversationId: string | number) => {
    try {
      // 调用后端 API 删除指定对话
      await deleteConversation(paperId as any, conversationId as any);
      // 重新加载对话列表（更新弹窗中的列表数据）
      await loadConversations(undefined);
      // 重新加载当前活跃对话的消息（因为可能删除的是活跃对话）
      const msgData = await getMessages(paperId as any);
      // 更新前端消息列表
      setMessages(msgData.messages || []);
    } catch (err: any) {
      // 删除失败时显示错误提示
      message.error('删除对话失败: ' + err.message);
    }
  }, [paperId, loadConversations, setMessages]); // 依赖论文 ID、加载对话列表函数、设置消息函数

  // ========== 副作用 ==========

  /**
   * 当弹窗打开时，自动从后端加载对话列表数据
   *
   * 使用 useEffect 监听 resumeModalOpen 状态变化，
   * 当弹窗打开时，自动加载最新的对话列表数据，
   * 确保用户看到的是最新的对话列表。
   */
  useEffect(() => {
    if (resumeModalOpen) {
      // 创建 AbortController 用于取消请求
      const controller = new AbortController();
      // 弹窗打开时，加载最新数据
      loadConversations(controller.signal);

      // 清理函数：弹窗关闭或组件卸载时取消请求
      return () => {
        controller.abort();
      };
    }
  }, [resumeModalOpen, loadConversations]); // 依赖弹窗打开状态和加载函数

  // ========== 返回值 ==========

  // 返回会话管理相关的状态和操作函数
  return {
    // 状态
    resumeModalOpen,      // 对话历史弹窗显示状态
    conversations,        // 对话会话列表

    // 状态更新函数
    setResumeModalOpen,   // 设置对话历史弹窗显示状态

    // 操作函数
    loadConversations,        // 加载对话列表
    handleResumeConversation, // 恢复指定的历史对话
    handleDeleteConversation, // 删除指定的对话会话
  };
}
