/**
 * @file React 适配层 —— 框架依赖重导出
 *
 * 本文件作为 React 框架依赖的统一适配层，将渲染插件中所需的 React API
 * 集中从此处导出，而非在各组件文件中直接 import 'react'。
 *
 * 设计决策：
 * - 通过适配层间接引用 React，便于未来替换框架实现（如 Preact）或
 *   在测试中注入 mock，只需修改此文件即可全局生效。
 * - 仅导出渲染插件实际使用的 React API，保持接口最小化。
 */

/** React hooks 和组件 —— 渲染插件中实际使用的子集 */
export { Fragment, useEffect, useRef, useState, useMemo } from 'react';
/** React 类型 —— 用于组件 Props 的类型标注 */
export type { CSSProperties, HTMLAttributes } from 'react';
