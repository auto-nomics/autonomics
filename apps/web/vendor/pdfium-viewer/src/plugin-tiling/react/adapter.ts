/**
 * @file 瓦片渲染插件 — React 适配层
 *
 * 本文件从 React 库中重新导出组件和 hooks 所需的类型和函数，
 * 作为框架无关的抽象层。这样组件文件可以通过相对路径引用
 * '../framework' 而不直接依赖 'react'，便于未来切换渲染框架。
 *
 * 导出的内容分为两类：
 * - 值导出：Fragment, useEffect, useRef, useState, useCallback, useMemo
 * - 类型导出：ReactNode, HTMLAttributes, CSSProperties
 */
export { Fragment, useEffect, useRef, useState, useCallback, useMemo } from 'react';
export type { ReactNode, HTMLAttributes, CSSProperties } from 'react';
