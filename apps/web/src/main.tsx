/**
 * Autonomics React 应用入口文件
 *
 * 路由由 PaneManager 的每个 pane 独立管理（各自持有 MemoryRouter），
 * 此处不再包裹顶层 Router，避免嵌套 Router 报错。
 */
import React from 'react';
import ReactDOM from 'react-dom/client';
import './i18n';
import App from './App';
import ErrorBoundary from './components/ErrorBoundary';
import BackendExitOverlay from './components/BackendExitOverlay';
import './styles/global.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
      {/* 与 App 平级：App 子树崩溃（ErrorBoundary 降级 UI）时遮罩仍挂载 */}
      <BackendExitOverlay />
    </ErrorBoundary>
  </React.StrictMode>
);
