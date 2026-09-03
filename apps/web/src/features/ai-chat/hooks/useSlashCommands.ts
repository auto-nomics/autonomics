/**
 * AI 聊天斜杠命令处理 Hook
 *
 * 负责处理所有斜杠命令（/new, /resume, /prompt, /attachment, /model, /clear, /hide）的逻辑。
 * 从 ChatPanel 组件中提取，使代码结构更清晰，职责更单一。
 *
 * 支持的命令：
 * - /new：创建新对话（保留旧对话为历史记录）
 * - /resume：打开对话历史弹窗，展示所有历史对话
 * - /prompt：打开模板管理弹窗
 * - /attachment：打开文件选择对话框（上传附件）
 * - /model：打开模型配置弹窗
 * - /clear：清空当前对话的所有消息
 * - /hide：收起侧边栏对话面板
 * - /paper：前往论文管理页面
 * - /find-refs：快捷操作 - 为观点找文献支持
 * - /find-related：快捷操作 - 查找相关文献
 * - /landscape：快捷操作 - 梳理研究方向
 * - /evaluate：快捷操作 - 评估科研想法
 * - /discover：快捷操作 - 发现科研方向
 *
 * @module ai-chat/hooks/useSlashCommands
 */

import { useCallback, useRef } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // 导入 Ant Design 消息提示组件
import { classifyFile } from '../components/FileUploader'; // 导入文件分类工具函数
import { createConversation } from '../../../services/chatApi'; // 导入对话管理 API

/**
 * 斜杠命令列表定义
 *
 * 用于输入框的自动补全提示。
 * 每个命令包含名称和描述，用户输入 "/" 时会显示此列表供选择。
 *
 * @type {Array<{name: string, description: string}>}
 */
export const SLASH_COMMANDS = [
  { name: '/new', description: '创建新对话' },
  { name: '/resume', description: '恢复历史对话' },
  { name: '/prompt', description: '打开模板管理' },
  { name: '/attachment', description: '上传附件' },
  { name: '/model', description: '管理模型配置' },
  { name: '/clear', description: '清空当前对话' },
  { name: '/hide', description: '收起对话面板' },
  { name: '/paper', description: '前往论文管理' },
  { name: '/find-refs', description: '为观点找文献支持' },
  { name: '/find-related', description: '查找相关文献' },
  { name: '/landscape', description: '梳理研究方向' },
  { name: '/evaluate', description: '评估科研想法' },
  { name: '/discover', description: '发现科研方向' },
];

/**
 * AI 聊天斜杠命令处理 Hook
 *
 * @param {object} params - Hook 参数
 * @param {number} params.paperId - 当前论文 ID，用于关联对话记录
 * @param {Function} params.persistMessages - 持久化消息到后端的函数
 * @param {Function} params.onToggleCollapse - 收起/展开对话面板的回调函数
 * @param {React.RefObject} params.slateInputRef - SlateInputWithSender 组件的 ref，用于调用其暴露的方法
 * @param {Function} params.setMessages - 设置消息列表的函数（用于 /clear 命令清空消息）
 * @returns {object} 包含斜杠命令处理相关函数的对象
 */
export function useSlashCommands({
  paperId,
  persistMessages,
  onToggleCollapse,
  slateInputRef,
  setMessages,
  onNavigateToPapers,
}: {
  paperId?: string | number;
  persistMessages?: any;
  onToggleCollapse?: () => void;
  slateInputRef?: any;
  setMessages?: any;
  onNavigateToPapers?: () => void;
}) {
  const { message } = App.useApp();
  // 隐藏的文件选择 input 引用，用于 /attachment 斜杠命令独立触发文件选择
  const attachmentInputRef = useRef<HTMLInputElement>(null);

  /**
   * 处理斜杠命令
   *
   * 检测用户输入是否为斜杠命令（/new 或 /resume 等），
   * 如果是则执行对应的命令逻辑并返回 true（阻止消息正常发送）。
   *
   * 命令匹配规则：
   * - 使用 text.trim() 去除首尾空格后进行精确匹配
   * - "/new" 和 "/new "（带尾随空格）都会匹配
   * - "/new something" 不会匹配，会作为普通消息发送
   *
   * @param {string} text - 用户输入的文本内容
   * @returns {boolean} true 表示输入是斜杠命令（消息不应正常发送），false 表示普通消息
   */
  const handleSlashCommand = useCallback((text: string) => {
    // 去除首尾空格后获取纯净的输入文本
    const trimmed = text.trim();

    // /new 命令：创建新的对话会话
    if (trimmed === '/new') {
      handleNewConversation(); // 调用新建对话的处理函数
      return true;             // 返回 true，阻止消息正常发送
    }

    // /resume 命令：打开对话历史弹窗
    // 此命令由 useConversationManager hook 处理，这里返回 false 让主组件处理
    // 不在这里直接处理，因为 resumeModalOpen 状态在 useConversationManager 中管理
    if (trimmed === '/resume') {
      // 返回 false 表示这是一个需要主组件处理的命令
      return false;
    }

    // /prompt 命令：打开模板管理弹窗
    // 用户在输入框中输入 /prompt 后，直接弹出模板管理弹窗
    // 这样用户无需移动鼠标去点击工具栏按钮，在键盘操作流程中即可快速访问模板管理功能
    // 适用场景：用户正在快速输入时，想查看或修改某个 prompt 模板，不想中断打字节奏
    if (trimmed === '/prompt') {
      // 此命令由主组件处理（setIsTemplateModalOpen）
      return false;
    }

    // /attachment 命令：独立打开文件选择对话框（上传附件）
    // 通过隐藏的原生 <input type="file"> 实现文件选择，不依赖任何 UI 按钮组件
    if (trimmed === '/attachment') {
      attachmentInputRef.current?.click(); // 触发隐藏的文件选择 input 的点击事件
      return true;                         // 返回 true，阻止消息正常发送
    }

    // /model 命令：打开模型配置弹窗
    // 与点击模型下拉菜单中的"管理自定义模型"选项功能完全一致
    // 底层调用 SlateInputWithSender 组件通过 useImperativeHandle 暴露的 openConfigModal() 方法
    // 适用场景：用户正在键盘操作流程中，想快速新增/编辑/删除模型配置
    if (trimmed === '/model') {
      slateInputRef.current?.openConfigModal(); // 通过 ref 调用 SlateInputWithSender 暴露的 openConfigModal() 方法
      return true;                              // 返回 true，阻止消息正常发送
    }

    // /clear 命令：清空当前对话的所有消息
    // 与 /new 不同，/clear 不会在后端创建新的对话会话，只是清空当前会话的消息列表
    // 适用场景：用户想快速清除当前对话内容重新开始，但不需要保留历史记录
    if (trimmed === '/clear') {
      setMessages([]);                 // 清空前端消息列表
      persistMessages([]);             // 持久化空消息列表到后端
      message.success('已清空当前对话'); // 显示成功提示
      return true;                     // 返回 true，阻止消息正常发送
    }

    // /hide 命令：收起侧边栏对话面板
    // 适用场景：用户正在键盘操作流程中，想快速收起对话面板以获得更大的阅读区域
    if (trimmed === '/hide') {
      onToggleCollapse?.(); // 调用父组件传入的收起回调，切换面板折叠状态
      return true;        // 返回 true，阻止消息正常发送
    }

    // /paper 命令：导航到论文管理页面
    // 适用场景：用户在 AI Chat 首页想快速切换到文献管理面板
    if (trimmed === '/paper') {
      if (onNavigateToPapers) {
        onNavigateToPapers();
        return true;
      }
      return false;
    }

    // ===== 快捷操作命令：将预格式化的 Prompt 文本插入输入框 =====
    // 这些命令不会发送消息，而是将模板文本插入编辑器，用户可以补充内容后手动发送
    const QUICK_ACTION_TEMPLATES = {
      '/find-refs': '请为以下观点找到支持或反驳的文献证据：\n',
      '/find-related': '请帮我查找与以下主题相关的文献：\n',
      '/landscape': '请帮我梳理以下研究方向的现状和趋势：\n',
      '/evaluate': '请帮我评估以下科研想法的可行性和创新性：\n',
      '/discover': '请基于以下研究方向，帮我发现潜在的新研究方向和课题：\n',
    };

    if (QUICK_ACTION_TEMPLATES[trimmed as keyof typeof QUICK_ACTION_TEMPLATES]) {
      const templateText = QUICK_ACTION_TEMPLATES[trimmed as keyof typeof QUICK_ACTION_TEMPLATES];
      if (slateInputRef?.current?.insertText) {
        slateInputRef.current.insertText(templateText);
      }
      return true;
    }

    // 不是斜杠命令，返回 false，让消息正常发送
    return false;
  }, [paperId, persistMessages, onToggleCollapse, setMessages, onNavigateToPapers]);

  /**
   * 创建新对话
   *
   * 调用后端 API 创建一个新的空对话会话：
   * 1. 后端将当前活跃对话归档（设为非活跃）
   * 2. 创建新的空对话并设为活跃
   * 3. 前端清空消息列表，开始全新的对话
   *
   * 用户之前对话的内容不会丢失，可以通过 /resume 恢复
   */
  const handleNewConversation = useCallback(async () => {
    try {
      // 调用后端 API 创建新对话
      await createConversation(String(paperId));
      // 清空前端消息列表（新对话从空开始）
      setMessages([]);
      // 显示成功提示
      message.success('已创建新对话');
    } catch (err: any) {
      // 显示错误提示
      message.error('创建新对话失败: ' + err.message);
    }
  }, [paperId, setMessages]); // 依赖论文 ID 和设置消息函数

  /**
   * 处理 /attachment 命令选择的文件
   *
   * 独立于 FileUploader 组件，直接使用原生 <input type="file"> + classifyFile 工具函数
   * 根据 classifyFile 的分类结果（image/text/binary）执行不同的读取逻辑
   *
   * 处理流程：
   * 1. 获取用户选择的文件
   * 2. 使用 classifyFile 对文件进行分类
   * 3. 根据分类结果选择不同的读取策略：
   *    - image: 转换为 base64（限制 10MB）
   *    - text: 读取为纯文本（限制 1MB）
   *    - binary: 保留文件对象（限制 20MB）
   * 4. 调用 SlateInputWithSender 的 handleFileSelect 方法添加附件
   */
  const handleAttachmentFileChange = useCallback(async (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]; // 获取用户选择的第一个文件
    if (!file) return; // 如果没有选择文件，直接返回

    // 使用 classifyFile 工具函数对文件进行分类
    // 分类结果包含：category（image/text/binary）、lang（编程语言，仅 text）、ext（文件扩展名）
    const classification = classifyFile(file.name);

    // 图片类文件处理
    if (classification.category === 'image') {
      // 检查文件大小限制（10MB）
      if (file.size > 10 * 1024 * 1024) {
        message.error('图片大小不能超过 10MB');
        return;
      }
      // 使用 FileReader 读取图片文件为 base64 格式
      const reader = new FileReader();
      reader.onload = (e) => {
        const base64 = e.target?.result; // 获取 base64 数据
        if (base64) {
          // 调用 SlateInputWithSender 的 handleFileSelect 方法添加图片附件
          slateInputRef.current?.handleFileSelect?.({
            category: 'image', // 附件类别：图片
            base64,            // base64 编码的图片数据
            name: file.name,   // 原始文件名
            sourceType: 'upload', // 来源类型：上传（相对于拖拽、粘贴等）
          });
        }
      };
      reader.onerror = () => message.error('读取图片失败'); // 读取失败时的错误提示
      reader.readAsDataURL(file); // 以 Data URL 格式读取文件（base64）

    // 文本类文件处理
    } else if (classification.category === 'text') {
      // 检查文件大小限制（1MB）
      if (file.size > 1 * 1024 * 1024) {
        message.error('文本文件大小不能超过 1MB');
        return;
      }
      // 使用 FileReader 读取文本文件
      const reader = new FileReader();
      reader.onload = (e) => {
        const textContent = e.target?.result; // 获取文本内容
        if (textContent !== undefined) {
          // 调用 SlateInputWithSender 的 handleFileSelect 方法添加文本附件
          slateInputRef.current?.handleFileSelect?.({
            category: 'text',             // 附件类别：文本
            name: file.name,              // 原始文件名
            textContent: String(textContent), // 文本内容
            language: classification.lang || '', // 编程语言（如果可识别）
          });
        }
      };
      reader.onerror = () => message.error('读取文件失败'); // 读取失败时的错误提示
      reader.readAsText(file); // 以纯文本格式读取文件

    // 二进制文件处理
    } else {
      // 检查文件大小限制（20MB）
      if (file.size > 20 * 1024 * 1024) {
        message.error('文件大小不能超过 20MB');
        return;
      }
      // 调用 SlateInputWithSender 的 handleFileSelect 方法添加二进制附件
      slateInputRef.current?.handleFileSelect?.({
        category: 'binary',         // 附件类别：二进制
        name: file.name,            // 原始文件名
        originalType: classification.ext, // 原始文件类型/扩展名
        file,                       // 文件对象
      });
    }

    // 清空 input 的 value，允许用户重复选择同一文件
    event.target.value = '';
  }, [slateInputRef]); // 依赖 slateInputRef

  // 返回斜杠命令处理相关的函数和 ref
  return {
    // 处理斜杠命令的主函数
    handleSlashCommand,

    // 创建新对话的函数
    handleNewConversation,

    // 附件文件选择处理的 ref 和函数
    attachmentInputRef,
    handleAttachmentFileChange,

    // 斜杠命令列表（用于自动补全提示）
    slashCommandList: SLASH_COMMANDS,
  };
}
