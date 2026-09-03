/**
 * 分栏路由配置组件
 *
 * 为每个分栏提供独立的路由配置，定义应用中所有可访问的页面路由。
 * 使用 React.lazy 实现路由级代码分割，优化首屏加载性能。
 *
 * 路由列表：
 * - / → ChatHomePage（AI 对话首页）
 * - /papers → PaperListPage（论文列表页）
 * - /paper/:id → PaperReaderPage（论文阅读器页）
 *
 * 代码分割说明：
 * - 每个页面组件使用 lazy() 动态导入，打包为独立的 chunk
 * - 首屏加载时只下载当前路由所需的 chunk
 * - Suspense fallback 在页面组件加载期间显示旋转加载动画
 * - 使用国际化文本显示加载提示
 *
 * @module panes/PaneRoutes
 */
import React, { lazy, Suspense } from 'react';
import { Routes, Route, Navigate } from 'react-router-dom';
import { Spin } from 'antd';
import { useTranslation } from 'react-i18next';

/** 论文列表页（懒加载） */
const PaperListPage = lazy(() => import('../pages/PaperListPage/index'));
/** 论文阅读器页（懒加载） */
const PaperReaderPage = lazy(() => import('../pages/PaperReaderPage/index'));
/** AI 对话首页（懒加载） */
const ChatHomePage = lazy(() => import('../pages/ChatHomePage/index'));

/**
 * 页面加载中的回退组件
 *
 * 在懒加载页面组件加载期间显示旋转加载动画和加载提示文本。
 * 使用 Ant Design Spin 组件，居中显示在 pane 区域内。
 *
 * @returns {React.ReactElement} 加载中的 UI
 */
function PaneLoadingFallback() {
  // 获取国际化翻译函数
  const { t } = useTranslation('panes');
  return (
    <div style={{
      display: 'flex', // flex 布局
      justifyContent: 'center', // 水平居中
      alignItems: 'center', // 垂直居中
      height: '100%', // 占满父容器高度
      background: 'var(--bg-tertiary)', // 使用 CSS 变量背景色
    }}>
      <Spin size="large" tip={t('loading')}> {/* 大尺寸旋转动画 + 加载提示 */}
        <div /> {/* Spin 组件需要子元素 */}
      </Spin>
    </div>
  );
}

/**
 * 分栏路由配置组件（默认导出）
 *
 * 定义应用的所有路由规则，使用 Suspense 包裹实现懒加载。
 * 每个 Route 对应一个页面组件，路径匹配时渲染对应的组件。
 *
 * @returns {React.ReactElement} 路由配置的 JSX
 */
export default function PaneRoutes() {
  return (
    <Suspense fallback={<PaneLoadingFallback />}> {/* 懒加载回退 UI */}
      <Routes>
        {/* AI 对话首页：默认路由 */}
        <Route path="/" element={<ChatHomePage />} />
        {/* 论文列表页 */}
        <Route path="/papers" element={<PaperListPage />} />
        {/* 论文阅读器页：动态路由，:id 为论文 ID */}
        <Route path="/paper/:id" element={<PaperReaderPage />} />
        {/* 兜底：未匹配的路由重定向到首页，避免 react-router 渲染空白（例如持久化的旧路由被删除） */}
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </Suspense>
  );
}
