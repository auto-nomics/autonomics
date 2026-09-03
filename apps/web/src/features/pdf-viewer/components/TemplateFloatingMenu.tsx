/**
 * PDF 右键模板选择浮动菜单组件
 *
 * 职责：显示模板选择菜单，让用户选择对选中文本的操作类型
 *
 * 交互流程：
 * 1. 用户在 PDF 中选中文本后右键
 * 2. 菜单在鼠标位置显示浮动菜单（解释/翻译/总结/学术评价/相关文献）
 * 3. 用户选择模板后，系统根据模板生成 prompt 并注入到 AI 对话面板
 * 4. 点击空白处 → 关闭菜单
 *
 * 为什么改为显示菜单而不是直接注入？
 * - 用户可能需要不同的操作（解释、翻译、总结、学术评价、相关文献）
 * - 直接注入只能使用固定的"请解释"模板，不够灵活
 * - 菜单方式让用户自主选择最合适的操作类型
 */

import { useEffect } from 'react';
import { getAvailableTemplates, getTemplateLabel, buildContextPrompt } from '../../../components/ContextBridge';
import { App } from 'antd';

/**
 * PDF 右键模板选择浮动菜单组件
 *
 * @param {Object} props - 组件属性
 * @param {boolean} props.visible - 菜单是否可见
 * @param {number} props.x - 菜单的水平位置（视口坐标）
 * @param {number} props.y - 菜单的垂直位置（视口坐标）
 * @param {Object|null} props.context - 待注入的上下文数据
 * @param {Function} props.onClose - 关闭菜单的回调
 * @param {Function} props.onTemplateSelect - 选择模板后的回调 (templateKey: string) => void
 * @param {boolean} [props.showHighlightOption=false] - 是否显示"高亮"选项
 * @returns {JSX.Element | null} 菜单 JSX 或 null
 */
export function TemplateFloatingMenu({
  visible,
  x,
  y,
  context,
  onClose,
  onTemplateSelect,
  showHighlightOption = false, // 默认不显示高亮选项（保持向后兼容）
}: {
  visible: boolean;
  x: number;
  y: number;
  context: any;
  onClose: () => void;
  onTemplateSelect: (key: string) => void;
  showHighlightOption?: boolean;
}) {
  /**
   * 监听全局点击和滚动事件，用于关闭右键浮动菜单
   *
   * 为什么需要这个 useEffect？
   * - 浮动菜单没有"关闭"按钮，需要通过点击外部区域来关闭
   * - 滚动 PDF 时也应该关闭菜单（避免菜单遮挡内容）
   * - 这是浮动菜单的通用交互模式（类似于下拉框的点击外部关闭）
   *
   * 实现原理：
   * - 当 visible 为 true 时，注册全局事件监听器
   * - 任何 mousedown 或 scroll 事件都会关闭菜单
   * - 组件卸载或菜单关闭时，自动清理事件监听器
   */
  useEffect(() => {
    // 如果菜单不可见，不需要注册监听器
    if (!visible) return;

    /**
     * 关闭菜单的处理函数
     */
    const closeMenu = () => {
      onClose(); // 调用父组件传入的关闭回调
    };

    // 注册全局 mousedown 事件监听器（点击任何位置都会关闭菜单）
    document.addEventListener('mousedown', closeMenu);
    // 注册全局 scroll 事件监听器（滚动页面时关闭菜单）
    // 第3个参数 true = 捕获阶段，确保在滚动 PDF 内容时也能捕获到事件
    window.addEventListener('scroll', closeMenu, true);

    // 清理函数：组件卸载或依赖变化时移除事件监听器，防止内存泄漏
    return () => {
      document.removeEventListener('mousedown', closeMenu); // 移除 mousedown 监听
      window.removeEventListener('scroll', closeMenu, true); // 移除 scroll 监听
    };
  }, [visible, onClose]); // 依赖于菜单可见状态

  // 如果菜单不可见，不渲染任何内容
  if (!visible) {
    return null;
  }

  return (
    <div
      // 鼠标按下事件：阻止冒泡，防止触发全局 mousedown 监听器导致菜单立即关闭
      onMouseDown={(e) => { e.stopPropagation(); e.preventDefault(); }}
      style={{
        position: 'fixed',                    // 固定定位，基于视口坐标
        left: x,                              // 水平位置 = 右键点击的 clientX
        top: y,                               // 垂直位置 = 右键点击的 clientY
        zIndex: 1050,                         // 层级高于普通内容（与 antd overlay 一致）
        background: 'var(--bg-elevated)',     // 背景色使用 CSS 变量，适配深色模式
        borderRadius: 6,                      // 圆角 6px
        boxShadow: 'var(--shadow-md)',        // 阴影效果使用 CSS 变量
        padding: '4px 0',                     // 上下 4px 内边距，左右无
        minWidth: 120,                        // 最小宽度 120px，确保文字不被截断
      }}
    >
      {/* ==================== 快捷翻译（直接调用翻译 API） ==================== */}
      <div
        onClick={() => onTemplateSelect('quick_translate')}
        onMouseDown={(e) => { e.stopPropagation(); e.preventDefault(); }}
        style={{
          padding: '6px 16px',
          cursor: 'pointer',
          fontSize: 13,
          color: 'var(--text-primary)',
          transition: 'background 0.2s',
          whiteSpace: 'nowrap',
          fontWeight: 500,
        }}
        onMouseEnter={(e) => {
          e.currentTarget.style.background = 'var(--overlay-hover)';
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.background = 'transparent';
        }}
      >
        快捷翻译
      </div>

      {/* 分隔线 */}
      <div style={{
        height: 1,
        margin: '4px 0',
        background: 'var(--border-color)',
      }} />

      {/* ==================== AI 模板选项 ==================== */}
      {/* 遍历 pdf_selection 类型可用的所有模板，渲染菜单项 */}
      {getAvailableTemplates('pdf_selection').map((key) => (
        <div
          key={key} // React 列表渲染的唯一 key
          // 菜单项点击事件：调用模板选择处理函数
          onClick={() => onTemplateSelect(key)}
          style={{
            padding: '6px 16px',              // 上下 6px，左右 16px 内边距
            cursor: 'pointer',                // 鼠标变为手型，提示可点击
            fontSize: 13,                     // 字体大小 13px，与 antd 菜单一致
            color: 'var(--text-primary)',     // 使用主题色变量
            transition: 'background 0.2s',    // 背景色过渡动画 0.2s
            whiteSpace: 'nowrap',             // 禁止换行，避免菜单项文字折行
          }}
          // 鼠标悬停时的内联样式无法用 :hover 伪类，改用 onMouseEnter/onMouseLeave
          onMouseEnter={(e) => {
            e.currentTarget.style.background = 'var(--overlay-hover)'; // 悬停背景色使用 CSS 变量
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = 'transparent'; // 离开时恢复透明背景
          }}
        >
          {/* 菜单项文字：使用 getTemplateLabel 将模板键转为中文标签 */}
          {getTemplateLabel(key)}
        </div>
      ))}

      {/* ==================== 分隔线（AI 模板和高亮之间） ==================== */}
      {/* 只在有 AI 模板且显示高亮选项时才显示分隔线 */}
      {showHighlightOption && getAvailableTemplates('pdf_selection').length > 0 && (
        <div style={{
          height: 1,                          // 分隔线高度 1px
          margin: '4px 0',                    // 上下 4px 外边距
          background: 'var(--border-color)',  // 分隔线颜色使用 CSS 变量
        }} />
      )}

      {/* ==================== 高亮选项 ==================== */}
      {/* 如果启用高亮选项，显示"高亮"菜单项 */}
      {showHighlightOption && (
        <div
          key="highlight" // 唯一 key
          // 点击事件：调用模板选择处理函数，传入 'highlight' 键
          onClick={() => onTemplateSelect('highlight')}
          style={{
            padding: '6px 16px',              // 上下 6px，左右 16px 内边距
            cursor: 'pointer',                // 鼠标变为手型，提示可点击
            fontSize: 13,                     // 字体大小 13px，与 antd 菜单一致
            color: 'var(--color-keypoint)',                 // 橙黄色，与高亮图标颜色一致
            transition: 'background 0.2s',    // 背景色过渡动画 0.2s
            whiteSpace: 'nowrap',             // 禁止换行，避免菜单项文字折行
          }}
          // 鼠标悬停时的内联样式无法用 :hover 伪类，改用 onMouseEnter/onMouseLeave
          // 高亮选项使用特殊的浅橙色悬停背景（保持语义色，不随主题变化）
          onMouseEnter={(e) => {
            e.currentTarget.style.background = 'var(--color-keypoint-bg)'; // 悬停背景色 = 浅橙色（语义色保持不变）
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = 'transparent'; // 离开时恢复透明背景
          }}
        >
          🖍️ 高亮 {/* 使用高亮图标 emoji + 中文标签 */}
        </div>
      )}
    </div>
  );
}

/**
 * 处理模板菜单项的点击事件（辅助函数）
 *
 * 当用户从右键浮动菜单中选择一个操作模板时调用此函数：
 * 1. 根据 templateKey 从 ContextBridge 获取对应的 prompt 模板
 * 2. 用保存的上下文数据填充模板占位符（{page}, {content}, {title}）
 * 3. 将填充后的 prompt 注入到 AI 对话面板（ChatPanel）
 * 4. 关闭浮动菜单并清除 PDF 文本选区
 *
 * @param {string} templateKey - 用户选择的模板键名
 * @param {Object} context - 待注入的上下文数据
 * @param {Function} onContextInject - 上下文注入回调函数
 * @param {Function} onCloseMenu - 关闭菜单的回调函数
 */
export function handleTemplateSelect(templateKey: string, context: any, onContextInject: any, onCloseMenu: () => void, messageApi: any) {
  // 如果没有保存的上下文数据，则不做处理（安全检查）
  if (!context) return;

  // 从上下文数据中解构出所需字段
  const { content, source_page, title } = context;

  // 使用 ContextBridge 的 buildContextPrompt 生成两段提示词（system + user）
  // - systemPrompt：模板的 system 段（如翻译场景的术语白名单 / LaTeX verbatim 规则）
  // - userPrompt：含占位符替换后的用户消息文本
  // 第3个参数 templateKey：用户选择的模板键，会覆盖默认的 pdf_selection 模板
  const { systemPrompt, userPrompt } = buildContextPrompt('pdf_selection', {
    content,              // 选中的文本内容
    page: source_page,    // 来源页码（模板占位符是 {page}）
    title,                // 显示标题
  }, templateKey);

  // 调用父组件传入的回调，将选中文本注入到 ChatPanel
  onContextInject({
    type: 'pdf_selection',  // 上下文类型：PDF 选中文本
    templateKey,            // 模板键（如 pdf_translate），用于生成短指令自动发送
    payload: {              // 载荷数据
      content,              // 选中的文本内容
      source_page,          // 来源页码
      title,                // 显示标题
    },
    systemPrompt,           // 模板 system 段（透传到后端顶层 system_prompt 字段）
    userPrompt,             // 用户消息文本
    prompt: userPrompt,     // 兼容字段：旧消费者若读 context.prompt 仍可用
  });

  // 关闭浮动菜单
  onCloseMenu();

  // 清除 PDF 中的文本选区，给用户明确的视觉反馈
  const selection = window.getSelection(); // 获取当前文本选区
  selection?.removeAllRanges();             // 移除所有选区范围，取消文本高亮

  // 显示操作成功提示
  messageApi.success('已发送到 AI 面板');
}

export default TemplateFloatingMenu;
