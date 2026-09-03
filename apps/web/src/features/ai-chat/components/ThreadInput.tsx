/**
 * ThreadInput.jsx
 * ============================================================
 * 线程追问输入组件
 *
 * 文件功能：
 * 提供一个轻量级的文本输入框，用于在追问线程中输入并发送追问消息。
 * 支持自动调整高度、Enter 发送、Shift+Enter 换行等基础功能。
 *
 * 核心设计决策：
 * - MVP 阶段不支持附件和斜杠命令，仅纯文本输入
 * - 自动调整高度（最多 5 行），超出后显示滚动条
 * - Enter 发送 / Shift+Enter 换行（标准聊天输入框行为）
 * - 禁用时显示 tooltip 提示原因
 * - 达到消息上限时禁用并显示提示
 *
 * @module ai-chat/components/ThreadInput
 */

import React, { useRef, useEffect, useCallback, useState } from 'react';

/**
 * ThreadInput 组件
 *
 * @param {Object} props - 组件属性
 * @param {Function} props.onSend - 发送消息的回调 (text: string) => void
 * @param {boolean} props.disabled - 是否禁用输入（流式中或达到上限）
 * @param {string} props.disabledReason - 禁用原因提示文本
 * @param {number} props.messageCount - 当前线程内的消息数量
 * @param {number} props.maxMessages - 每个线程允许的最大消息数量
 * @param {string} props.id - 输入框的唯一标识（用于外部聚焦操作）
 */
const ThreadInput = ({
  onSend,
  disabled = false,
  disabledReason = '请等待当前回复完成',
  messageCount = 0,
  maxMessages = 30,
  id = '',
}: {
  onSend?: (text: string) => void;
  disabled?: boolean;
  disabledReason?: string;
  messageCount?: number;
  maxMessages?: number;
  id?: string;
}) => {
  /**
   * textarea 元素的引用
   * 用于：
   * - 自动调整高度
   * - 在发送后清空内容
   * - 在展开线程时自动聚焦（由父组件控制）
   */
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  /**
   * 本地状态：输入框的值
   * 使用受控组件模式，值由 state 管理
   */
  const [value, setValue] = useState('');

  /**
   * 本地状态：是否正在按下 Shift 键
   * 用于区分 Enter（发送）和 Shift+Enter（换行）
   */
  const [isShiftPressed, setIsShiftPressed] = useState(false);

  /**
   * 本地状态：是否显示禁用原因提示
   * 鼠标悬停在禁用输入框上时显示
   */
  const [showDisabledHint, setShowDisabledHint] = useState(false);

  /**
   * 自动调整 textarea 高度
   *
   * 根据内容动态调整高度，最少 1 行，最多 5 行
   * 超过 5 行后显示滚动条
   */
  const adjustHeight = useCallback(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    // 重置高度为 auto，以便正确计算 scrollHeight
    textarea.style.height = 'auto';

    // 计算新的高度：取 scrollHeight 和最大高度之间的较小值
    // 每行约 24px（行高 1.5 × 字号 13px + padding）
    const minHeight = 36; // 单行最小高度（padding + line-height）
    const maxHeight = 120; // 5 行最大高度（36px × 5 - padding）

    const newHeight = Math.min(Math.max(textarea.scrollHeight, minHeight), maxHeight);

    // 设置计算后的高度
    textarea.style.height = `${newHeight}px`;

    // 如果超过最大高度，显示滚动条
    textarea.style.overflowY = textarea.scrollHeight > maxHeight ? 'auto' : 'hidden';
  }, []);

  /**
   * 处理输入变化
   *
   * @param {React.ChangeEvent<HTMLTextAreaElement>} e - 输入事件对象
   */
  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const newValue = e.target.value;
    setValue(newValue);
    adjustHeight();
  };

  /**
   * 处理键盘事件
   *
   * - Enter：发送消息（如果没有按下 Shift）
   * - Shift+Enter：插入换行
   *
   * @param {React.KeyboardEvent<HTMLTextAreaElement>} e - 键盘事件对象
   */
  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // 检测 Shift 键状态
    if (e.key === 'Shift') {
      setIsShiftPressed(true);
    }

    // Enter 键处理
    if (e.key === 'Enter' && !e.shiftKey && !disabled) {
      e.preventDefault(); // 阻止默认的换行行为

      // 如果输入框有内容，发送消息
      const trimmedValue = value.trim();
      if (trimmedValue) {
        handleSend(trimmedValue);
      }
    }
  };

  /**
   * 处理 Shift 键抬起
   *
   * @param {React.KeyboardEvent<HTMLTextAreaElement>} e - 键盘事件对象
   */
  const handleKeyUp = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Shift') {
      setIsShiftPressed(false);
    }
  };

  /**
   * 发送消息
   *
   * @param {string} text - 要发送的文本内容
   */
  const handleSend = useCallback((text: string) => {
    if (!text || disabled) return;

    // 调用父组件传入的发送回调
    onSend?.(text);

    // 清空输入框
    setValue('');

    // 重置 textarea 高度
    if (textareaRef.current) {
      textareaRef.current.style.height = '36px'; // 重置为单行高度
      textareaRef.current.style.overflowY = 'hidden';
    }
  }, [disabled, onSend]);

  /**
   * 处理焦点事件
   *
   * 鼠标进入禁用输入框时显示提示，移出时隐藏
   */
  const handleMouseEnter = useCallback(() => {
    if (disabled) {
      setShowDisabledHint(true);
    }
  }, [disabled]);

  const handleMouseLeave = useCallback(() => {
    setShowDisabledHint(false);
  }, []);

  /**
   * 暴露给父组件的方法
   *
   * 使用 useImperativeHandle 或直接通过 ref 暴露方法
   * 这里简化处理，父组件可以通过 props 控制聚焦
   */
  useEffect(() => {
    // 如果需要自动聚焦，可以在父组件传入 autoFocus prop
    // 这里暂时不实现，由父组件控制
  }, []);

  /**
   * 组件卸载时清理
   */
  useEffect(() => {
    return () => {
      // 清理可能存在的定时器或事件监听器
    };
  }, []);

  /**
   * 判断是否达到消息上限
   * 用于显示不同的提示信息
   */
  const isAtLimit = messageCount >= maxMessages;

  /**
   * 构建 placeholder 文本
   *
   * 根据不同状态显示不同的占位符：
   * - 达到上限：显示"已达到消息上限"
   * - 流式中：显示默认占位符
   * - 正常：显示"继续追问..."
   */
  const getPlaceholder = () => {
    if (isAtLimit) {
      return '已达到消息上限';
    }
    if (disabled) {
      return '请等待当前回复完成';
    }
    return '继续追问... (Enter 发送, Shift+Enter 换行)';
  };

  /**
   * 渲染禁用原因提示
   *
   * 仅在禁用且鼠标悬停时显示
   */
  const renderDisabledHint = () => {
    if (!disabled || !showDisabledHint) return null;

    return (
      <div style={{
        position: 'absolute',
        bottom: '100%',
        left: 0,
        marginBottom: 4,
        padding: '4px 8px',
        background: 'var(--bg-tertiary, #eaeaea)',
        border: '1px solid var(--border-color-secondary, #cccccc)',
        borderRadius: 4,
        fontSize: 11,
        color: 'var(--text-secondary, #595959)',
        whiteSpace: 'nowrap',
        zIndex: 10,
        pointerEvents: 'none',
      }}>
        {disabledReason}
      </div>
    );
  };

  /**
   * 渲染输入框组件
   *
   * 使用相对定位包裹 textarea 和禁用提示
   */
  return (
    <div
      style={{
        position: 'relative',
        marginTop: 8,
      }}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
    >
      {/* 禁用原因提示 */}
      {renderDisabledHint()}

      {/* 输入框容器 */}
      <div style={{
        position: 'relative',
        display: 'flex',
        alignItems: 'flex-end',
        gap: 8,
      }}>
        {/* 文本输入框 */}
        <textarea
          ref={textareaRef}
          id={id}
          value={value}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          onKeyUp={handleKeyUp}
          disabled={disabled || isAtLimit}
          placeholder={getPlaceholder()}
          aria-label="追问输入"
          style={{
            flex: 1,
            minWidth: 0, // 允许 flex 子元素收缩
            height: 36, // 初始高度（单行）
            maxHeight: 120, // 最大高度（5 行）
            padding: '8px 10px',
            fontSize: 13,
            lineHeight: 1.5,
            color: 'var(--text-primary, #262626)',
            background: 'var(--bg-primary, #ffffff)',
            border: '1px solid var(--border-color, #f0f0f0)',
            borderRadius: 6,
            resize: 'none', // 禁止手动调整大小
            outline: 'none',
            transition: 'border-color 0.2s, box-shadow 0.2s',
            cursor: disabled || isAtLimit ? 'not-allowed' : 'text',
          }}
          onFocus={(e) => {
            if (!disabled && !isAtLimit) {
              e.target.style.borderColor = 'var(--color-primary)';
              e.target.style.boxShadow = '0 0 0 2px color-mix(in srgb, var(--color-primary) 10%, transparent)';
            }
          }}
          onBlur={(e) => {
            e.target.style.borderColor = 'var(--border-color, #f0f0f0)';
            e.target.style.boxShadow = 'none';
          }}
        />

        {/* 发送按钮（仅在非禁用且有输入内容时显示） */}
        {value.trim() && !disabled && !isAtLimit && (
          <button
            onClick={() => handleSend(value.trim())}
            style={{
              padding: '6px 12px',
              fontSize: 12,
              color: 'var(--accent-content-text)',
              background: 'var(--color-primary)',
              border: 'none',
              borderRadius: 6,
              cursor: 'pointer',
              transition: 'background-color 0.2s',
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.backgroundColor = 'var(--text-secondary)';
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.backgroundColor = 'var(--color-primary)';
            }}
          >
            发送
          </button>
        )}
      </div>

      {/* 消息数量提示 */}
      {!isAtLimit && (
        <div style={{
          marginTop: 4,
          fontSize: 10,
          color: 'var(--text-tertiary, #8c8c8c)',
          textAlign: 'right',
        }}>
          {messageCount} / {maxMessages}
        </div>
      )}
    </div>
  );
};

/**
 * 使用 React.memo 优化性能
 *
 * 比较函数：仅在关键 props 变化时重渲染
 * - disabled 状态变化
 * - messageCount 变化
 * - maxMessages 变化（很少变化）
 * - id 变化（线程 ID 变化时需要重渲染）
 */
export default React.memo(ThreadInput, (prevProps: { disabled?: boolean; messageCount?: number; maxMessages?: number; disabledReason?: string; id?: string }, nextProps: { disabled?: boolean; messageCount?: number; maxMessages?: number; disabledReason?: string; id?: string }) => {
  if (prevProps.disabled !== nextProps.disabled) return false;
  if (prevProps.messageCount !== nextProps.messageCount) return false;
  if (prevProps.maxMessages !== nextProps.maxMessages) return false;
  if (prevProps.disabledReason !== nextProps.disabledReason) return false;
  if (prevProps.id !== nextProps.id) return false;
  return true;
});
