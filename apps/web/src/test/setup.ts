/**
 * Vitest 测试环境配置文件
 *
 * 本文件在测试运行前执行，负责：
 * - 配置全局测试工具（如 vi.mock）
 * - Mock 浏览器 API（IntersectionObserver、ResizeObserver）
 * - Mock Ant Design 组件和图标
 */

import { vi } from 'vitest';
import '@testing-library/jest-dom';

// ============================================================================
// 浏览器 API Mock
// ============================================================================

/**
 * Mock IntersectionObserver
 *
 * react-pdf 库需要此 API 来检测元素是否在视口中可见。
 * 测试环境中不需要真实的功能，返回空实现即可。
 */
global.IntersectionObserver = vi.fn().mockImplementation(() => ({
  observe: vi.fn(),        // 开始观察元素
  unobserve: vi.fn(),      // 停止观察元素
  disconnect: vi.fn(),     // 断开所有观察
}));

/**
 * Mock ResizeObserver
 *
 * 布局组件（如 SplitPane）需要此 API 来监听容器尺寸变化。
 * 测试环境中不需要真实的功能，返回空实现即可。
 */
global.ResizeObserver = vi.fn().mockImplementation(() => ({
  observe: vi.fn(),        // 开始观察元素
  unobserve: vi.fn(),      // 停止观察元素
  disconnect: vi.fn(),     // 断开所有观察
}));

// ============================================================================
// Ant Design 组件 Mock
// ============================================================================

/**
 * Mock Ant Design 组件
 *
 * 将部分 Ant Design 组件替换为直接返回子组件的简单实现，
 * 避免测试时渲染复杂的 UI 组件逻辑。
 */
vi.mock('antd', () => ({
  /**
   * Tooltip 组件 Mock
   * 直接返回子元素，不渲染提示框。
   */
  Tooltip: ({ children }: { children: React.ReactNode }) => children,
  /**
   * Tag 组件 Mock
   * 直接返回子元素，不渲染标签样式。
   */
  Tag: ({ children }: { children: React.ReactNode }) => children,
}));

// ============================================================================
// Ant Design 图标 Mock
// ============================================================================

/**
 * Mock Ant Design 图标
 *
 * 包含所有项目中使用的图标组件。
 * 返回 null 表示不渲染图标（纯函数组件）。
 * 这样可以避免测试时渲染大量无用的 SVG 图标元素。
 */
vi.mock('@ant-design/icons', () => ({
  // ---------- 状态相关图标 ----------
  /** 成功状态图标 */
  CheckCircleOutlined: () => null,
  /** 失败状态图标 */
  CloseCircleOutlined: () => null,
  /** 等待中状态图标 */
  ClockCircleOutlined: () => null,
  /** 加载中状态图标 */
  SyncOutlined: () => null,

  // ---------- 导览节点类型图标 ----------
  /** 章节图标 */
  FileTextOutlined: () => null,
  /** 要点图标 */
  BulbOutlined: () => null,
  /** 洞察图标 */
  SearchOutlined: () => null,
  /** 文档图标 */
  FormOutlined: () => null,
  /** 对话图标 */
  MessageOutlined: () => null,

  // ---------- PDF Viewer 组件使用的图标 ----------
  /** 左箭头图标（导航到上一页） */
  LeftOutlined: () => null,
  /** 右箭头图标（导航到下一页） */
  RightOutlined: () => null,
  /** 高亮图标 */
  HighlightOutlined: () => null,
  /** 太阳图标（关闭 PDF 深色模式） */
  SunOutlined: () => null,
  /** 月亮图标（开启 PDF 深色模式） */
  MoonOutlined: () => null,
}));
