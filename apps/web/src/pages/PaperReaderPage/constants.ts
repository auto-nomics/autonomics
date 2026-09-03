/**
 * 论文阅读页面常量配置
 *
 * 定义三栏布局的配置常量，包括面板默认宽度、宽度约束、拖拽手柄尺寸等
 */

/**
 * 面板默认宽度（像素）
 */
export const DEFAULT_WIDTHS = {
  GUIDE: 280,   // 左侧导航面板默认宽度
  CHAT: 380,    // 右侧聊天面板默认宽度
} as const;

/**
 * 面板宽度约束（像素）
 */
export const WIDTH_CONSTRAINTS = {
  GUIDE: {
    MIN: 200,   // 导航面板最小宽度
    MAX: 500,   // 导航面板最大宽度
  },
  CHAT: {
    MIN: 250,   // 聊天面板最小宽度
    MAX: 600,   // 聊天面板最大宽度
  },
} as const;

/**
 * 拖拽手柄尺寸
 */
export const DRAG_HANDLE = {
  WIDTH: 4,     // 拖拽手柄宽度（像素）
  HEIGHT: 40,   // 拖拽手柄高度（像素）
} as const;

/**
 * 拖拽状态键，用于标识当前正在调整哪个面板
 */
export const DRAGGING_KEYS = {
  GUIDE: 'guide',   // 导航面板拖拽键
  CHAT: 'chat',     // 聊天面板拖拽键
} as const;
