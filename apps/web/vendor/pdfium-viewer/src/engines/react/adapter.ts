/**
 * @file React API 适配层
 *
 * 从 React 包中统一导出引擎模块所需的 React API，
 * 避免在各个文件中重复导入，同时方便未来替换为其他 React-like 框架。
 */
export { useState, useEffect, useRef, useContext, createContext } from 'react';
export type { JSX, ReactNode } from 'react';
