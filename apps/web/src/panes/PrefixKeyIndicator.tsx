/**
 * 前缀键操作提示指示器组件
 *
 * 在分栏模式下，当用户按下 Ctrl+B 前缀键后，屏幕底部显示操作提示，
 * 帮助用户了解接下来可以执行的快捷键操作。
 *
 * 工作流程：
 * 1. 用户按下 Ctrl+B → prefixKeyState 变为 'pending'
 * 2. 屏幕底部显示操作提示（切换面板、分屏、关闭等快捷键说明）
 * 3. 用户按下后续操作键（如 h/l/j/k）→ 执行对应操作
 * 4. 用户按下 s 键 → prefixKeyState 变为 'pendingSplit'
 * 5. 屏幕底部显示分屏提示（选择新建面板的页面类型）
 * 6. 用户按下页面选择键（如 a/p/n/b）→ 执行分屏操作
 * 7. 用户按 Esc → 取消操作，prefixKeyState 变为 'idle'
 *
 * @module panes/PrefixKeyIndicator
 */
import React from 'react';
import { useTranslation } from 'react-i18next';
import { PrefixKeyState } from './types';

/**
 * PrefixKeyIndicator 组件的 Props 接口
 */
interface Props {
  /** 前缀键状态，决定显示哪种提示信息 */
  state: PrefixKeyState;
}

/**
 * 前缀键操作提示指示器组件
 *
 * 根据当前前缀键状态显示不同的操作提示：
 * - 'pendingSplit': 显示分屏模式提示（选择新建面板的页面类型）
 * - 'pending'/'active': 显示通用操作提示（切换、分屏、关闭等快捷键）
 *
 * 使用国际化文本，支持多语言显示。
 * 使用 React.memo 优化，避免不必要的重渲染。
 *
 * @param {Props} props - 组件属性
 * @returns {React.ReactElement} 操作提示的 JSX
 */
function PrefixKeyIndicator({ state }: Props) {
  // 获取国际化翻译函数，命名空间为 'panes'
  const { t } = useTranslation('panes');

  // 分屏模式：显示页面类型选择提示
  if (state === 'pendingSplit') {
    return (
      <div className="prefix-key-indicator">
        {t('splitHint')} {/* 分屏提示文本 */}
      </div>
    );
  }

  // 通用操作模式：显示切换、分屏、关闭等快捷键提示
  return (
    <div className="prefix-key-indicator">
      {t('shortcutHint')} {/* 快捷键提示文本 */}
    </div>
  );
}

// 使用 React.memo 优化，避免在 prefixKeyState 未变化时重渲染
export default React.memo(PrefixKeyIndicator);
