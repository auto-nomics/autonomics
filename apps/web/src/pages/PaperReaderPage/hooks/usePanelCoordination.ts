/**
 * 面板协调 Hook
 *
 * 管理论文阅读器的三栏布局协调，包括：
 * - 面板折叠状态：左侧导览栏和右侧聊天面板的展开/折叠
 * - 面板宽度调整：通过 useResizable hook 实现拖拽调整宽度
 * - 环境感知：Tauri 桌面端默认折叠面板，浏览器端默认展开
 * - 拖拽状态管理：标识当前正在调整哪个面板，防止多手柄冲突
 *
 * @module PaperReaderPage/hooks/usePanelCoordination
 */

import { useState, useCallback } from 'react';
import useResizable from './useResizable';
import { DEFAULT_WIDTHS, WIDTH_CONSTRAINTS, DRAGGING_KEYS } from '../constants';
import { isTauri } from '../../../services/client';

/**
 * 自定义 Hook，用于管理面板协调
 *
 * @returns 包含面板状态和调整处理函数的对象
 */
export function usePanelCoordination() {
  // 面板折叠状态（桌面端：Tauri 环境默认折叠，浏览器环境默认展开）
  const [guideCollapsed, setGuideCollapsed] = useState(isTauri);
  const [chatCollapsed, setChatCollapsed] = useState(isTauri);

  // 面板宽度状态
  const [guideWidth, setGuideWidth] = useState<number>(DEFAULT_WIDTHS.GUIDE);
  const [chatWidth, setChatWidth] = useState<number>(DEFAULT_WIDTHS.CHAT);

  // 当前正在拖拽的手柄标识符
  const [isDragging, setIsDragging] = useState<string | null>(null);

  // 导航面板切换处理函数
  const toggleGuide = useCallback(() => {
    setGuideCollapsed(prev => !prev);
  }, []);

  // 聊天面板切换处理函数
  const toggleChat = useCallback(() => {
    setChatCollapsed(prev => !prev);
  }, []);

  // 使用提取的 useResizable Hook 创建导航面板调整处理函数
  const handleGuideResizeStart = useResizable({
    currentWidth: guideWidth,                    // 当前宽度
    minWidth: WIDTH_CONSTRAINTS.GUIDE.MIN,       // 最小宽度约束
    maxWidth: WIDTH_CONSTRAINTS.GUIDE.MAX,       // 最大宽度约束
    direction: 'left',                           // 向左调整
    onWidthChange: setGuideWidth,                // 宽度变化回调
    onDragStart: setIsDragging,                  // 拖拽状态回调
    draggingKey: DRAGGING_KEYS.GUIDE,            // 拖拽标识键
  });

  // 使用提取的 useResizable Hook 创建聊天面板调整处理函数
  const handleChatResizeStart = useResizable({
    currentWidth: chatWidth,                     // 当前宽度
    minWidth: WIDTH_CONSTRAINTS.CHAT.MIN,        // 最小宽度约束
    maxWidth: WIDTH_CONSTRAINTS.CHAT.MAX,        // 最大宽度约束
    direction: 'right',                          // 向右调整
    onWidthChange: setChatWidth,                 // 宽度变化回调
    onDragStart: setIsDragging,                  // 拖拽状态回调
    draggingKey: DRAGGING_KEYS.CHAT,             // 拖拽标识键
  });

  return {
    // 导航面板相关
    guideCollapsed,          // 导航面板折叠状态
    guideWidth,              // 导航面板宽度
    toggleGuide,             // 切换导航面板
    handleGuideResizeStart,  // 开始调整导航面板

    // 聊天面板相关
    chatCollapsed,           // 聊天面板折叠状态
    chatWidth,               // 聊天面板宽度
    toggleChat,              // 切换聊天面板
    handleChatResizeStart,   // 开始调整聊天面板

    // 拖拽状态
    isDragging,              // 当前正在拖拽的面板标识
  };
}
