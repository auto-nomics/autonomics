/**
 * JayRead 聊天相关 API 客户端
 *
 * 封装与论文 AI 对话相关的数据持久化接口：
 * - 获取聊天记录：打开论文时加载历史对话
 * - 保存聊天记录：每次对话后保存完整消息列表
 * - 清空聊天记录：用户主动清空对话历史
 *
 * 为什么只负责存储而不处理 AI 对话？
 * - AI 对话需要流式响应（SSE），与普通 RESTful API 处理方式不同
 * - 将存储和流式对话分离，职责更清晰
 * - 聊天记录存储作为独立的持久化层，可被多种对话实现复用
 *
 * 为什么采用全量覆盖而非增量保存？
 * - 简化后端逻辑，无需处理并发写入
 * - 聊天记录通常不大，全量传输开销可接受
 * - 便于实现"编辑历史消息"等高级功能
 */
import request, { invalidateCache } from './client'; // 导入通用 API 客户端封装函数
import type {
  Message,
  GetMessagesResponse,
  SaveMessagesResponse,
  ClearMessagesResponse,
  GetConversationsResponse,
  CreateConversationResponse,
  ActivateConversationResponse,
  DeleteConversationResponse,
  ConversationSummary,
} from '@/types';

/**
 * 获取指定论文的聊天历史记录
 *
 * 使用场景：
 * - 用户打开论文阅读器时，自动加载历史对话
 * - 刷新页面后恢复之前的对话状态
 *
 * @param {string} paperId - 论文 ID，用于关联聊天记录与具体论文
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<GetMessagesResponse>} 历史消息数组
 */
export async function getMessages(paperId: string, signal?: AbortSignal): Promise<GetMessagesResponse> { // 导出获取聊天记录函数
  return request<GetMessagesResponse>(`/papers/${paperId}/chat`, { signal }); // GET 请求获取指定论文的聊天记录
}

/**
 * 保存聊天记录（全量覆盖模式）
 *
 * 为什么是 POST 而不是 PUT？
 * - 从语义上讲，POST 用于"提交数据供服务器处理"
 * - 这里是"保存消息列表"这一动作，而非"替换资源"
 * - RESTful 规范中 POST 更适合这种"命令式"操作
 *
 * 为什么全量保存而不是只保存新消息？
 * - 支持用户编辑/删除历史消息的场景
 * - 避免增量同步带来的版本冲突问题
 * - 简化客户端逻辑，无需维护本地同步状态
 *
 * @param {string} paperId - 论文 ID
 * @param {Array<Message>} messages - 完整的消息数组
 * @returns {Promise<SaveMessagesResponse>} 保存确认消息
 */
export async function saveMessages(paperId: string, messages: Message[], signal?: AbortSignal): Promise<SaveMessagesResponse> { // 导出保存聊天记录函数
  return request<SaveMessagesResponse>(`/papers/${paperId}/chat`, { // POST 请求保存聊天记录
    method: 'POST', // HTTP 方法为 POST
    body: { messages },  // 将完整消息列表作为请求体发送
    signal,
  });
}

/**
 * 清空指定论文的聊天记录
 *
 * 使用场景：
 * - 用户点击"清空对话"按钮
 * - 开始新的对话话题时清空历史
 *
 * 为什么使用 DELETE 而不是 POST + empty array？
 * - DELETE 语义更明确：删除资源
 * - 后端可以真正释放存储空间
 * - 符合 RESTful API 设计规范
 *
 * @param {string} paperId - 论文 ID
 * @returns {Promise<ClearMessagesResponse>} 清空确认消息
 */
export async function clearMessages(paperId: string): Promise<ClearMessagesResponse> { // 导出清空聊天记录函数
  return request<ClearMessagesResponse>(`/papers/${paperId}/chat`, { method: 'DELETE' }); // DELETE 请求清空指定论文的聊天记录
}

// ========== 对话会话管理 API ==========
// 以下函数用于支持前端的 /new 和 /resume 斜杠命令
// 每篇论文可以有多个独立的对话会话，用户可以创建新对话或恢复历史对话

/**
 * 获取论文的所有对话会话列表
 *
 * 使用场景：
 * - /resume 命令：弹出对话历史列表，展示该论文的所有对话
 * - 每个对话包含标题、消息数量、创建时间等摘要信息
 * - 按更新时间倒序排列（最近活跃的对话排在最前面）
 *
 * @param {string} paperId - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<GetConversationsResponse>} 对话列表
 */
export async function getConversations(paperId: string, signal?: AbortSignal): Promise<GetConversationsResponse> { // 导出获取对话列表函数
  return request<GetConversationsResponse>(`/papers/${paperId}/conversations`, { signal }); // GET 请求获取论文的所有对话会话列表
}

/**
 * 创建新的对话会话
 *
 * 使用场景：
 * - /new 命令：将当前活跃对话归档，创建一个全新的空对话
 * - 用户可以在新对话中开始全新的讨论，旧对话通过 /resume 随时可恢复
 *
 * 后端行为：
 * - 将当前活跃对话设为非活跃（归档）
 * - 创建新的空对话并设为活跃状态
 * - 返回新对话的 ID
 *
 * @param {string} paperId - 论文 ID
 * @returns {Promise<CreateConversationResponse>} 新对话 ID
 */
export async function createConversation(paperId: string): Promise<CreateConversationResponse> { // 导出创建新对话函数
  const result = await request<CreateConversationResponse>(`/papers/${paperId}/conversations/new`, { method: 'POST' }); // POST 请求创建新对话
  invalidateCache(`/papers/${paperId}/chat`);
  invalidateCache(`/papers/${paperId}/conversations`);
  return result;
}

/**
 * 切换到指定的历史对话
 *
 * 使用场景：
 * - /resume 弹窗中点击某个历史对话时，切换到该对话
 * - 前端收到响应后需要重新调用 getMessages 加载新对话的消息内容
 *
 * 后端行为：
 * - 将当前活跃对话设为非活跃
 * - 将目标对话设为活跃
 *
 * @param {string} paperId - 论文 ID
 * @param {string} conversationId - 要切换到的对话会话 ID
 * @returns {Promise<ActivateConversationResponse>} 切换结果
 */
export async function activateConversation(paperId: string, conversationId: string): Promise<ActivateConversationResponse> { // 导出切换对话函数
  const result = await request<ActivateConversationResponse>(`/papers/${paperId}/conversations/${conversationId}/activate`, { method: 'POST' }); // POST 请求切换到指定对话
  invalidateCache(`/papers/${paperId}/chat`);
  return result;
}

/**
 * 删除指定的对话会话
 *
 * 使用场景：
 * - /resume 弹窗中点击删除按钮，删除不再需要的历史对话
 * - 删除后对话及其所有消息将永久丢失（物理删除）
 *
 * 后端行为：
 * - 删除对话记录及其所有消息（外键级联删除）
 * - 如果删除的是活跃对话，自动激活最近的历史对话
 * - 如果没有任何剩余对话，创建一个新的空对话
 *
 * @param {string} paperId - 论文 ID
 * @param {string} conversationId - 要删除的对话会话 ID
 * @returns {Promise<DeleteConversationResponse>} 删除结果
 */
export async function deleteConversation(paperId: string, conversationId: string): Promise<DeleteConversationResponse> { // 导出删除对话函数
  const result = await request<DeleteConversationResponse>(`/papers/${paperId}/conversations/${conversationId}`, { method: 'DELETE' }); // DELETE 请求删除指定对话
  invalidateCache(`/papers/${paperId}/chat`);
  invalidateCache(`/papers/${paperId}/conversations`);
  return result;
}

/**
 * 生成对话历史的渐进式摘要
 *
 * 使用场景：
 * - 对话历史过长时，自动压缩为结构化摘要
 * - 保留最近消息，将旧消息压缩为摘要
 * - 降低 token 消耗，提升对话响应速度
 *
 * 请求体格式：
 * - messages: 完整的消息列表（需摘要的对话历史）
 * - paper_title: 论文标题（用于摘要上下文）
 *
 * 响应格式：
 * - summary: AI 生成的结构化摘要文本
 * - summarized_count: 被摘要的消消息数量
 *
 * @param {string} paperId - 论文 ID
 * @param {Array<Message>} messages - 需要摘要的消息列表
 * @param {string} paperTitle - 论文标题（用于摘要上下文）
 * @returns {Promise<ConversationSummary>} 摘要结果
 */
export async function summarizeConversation(paperId: string, messages: Message[], paperTitle: string, signal?: AbortSignal): Promise<ConversationSummary> { // 导出生成对话摘要函数
  return request<ConversationSummary>(`/papers/${paperId}/chat/summarize`, { // POST 请求生成对话摘要
    method: 'POST', // HTTP 方法为 POST
    body: { messages, paper_title: paperTitle },  // 请求体：消息列表和论文标题
    signal,
    timeout: 60000, // Summarization may take longer
  });
}
