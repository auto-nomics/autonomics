import React, { useEffect, useState } from 'react';
import { Layout, ConfigProvider, theme as antdTheme, App as AntApp } from 'antd';
import { useTranslation } from 'react-i18next';
import NiceModal from '@ebay/nice-modal-react';
import useAppStore, { resolveSystemTheme } from './stores/useAppStore';
import useTmuxKeyHandler from './hooks/useTmuxKeyHandler';
import { useAntdLocale } from './hooks/useAntdLocale';
import PaneManager from './panes/PaneManager';
import AddinSetupWizard from './components/AddinSetupWizard';
import './modals';

const { Content } = Layout;

/**
 * 应用内部组件
 *
 * 路由和页面渲染已委托给 PaneManager（支持 tmux 风格分屏）。
 * 此组件负责：主题同步、全局快捷键、分屏键盘处理。
 */
function AppContent() {
  const appTheme = useAppStore((state: unknown) => (state as { theme: string }).theme);

  // 注册 tmux 分屏快捷键（Ctrl+B 前缀键）
  useTmuxKeyHandler();

  // 同步主题到 DOM 元素
  useEffect(() => {
    const effectiveTheme = appTheme === 'auto' ? resolveSystemTheme() : appTheme;
    document.documentElement.setAttribute('data-theme', effectiveTheme);

    let metaTag = document.querySelector('meta[name="color-scheme"]');
    if (!metaTag) {
      metaTag = document.createElement('meta');
      (metaTag as HTMLMetaElement).name = 'color-scheme';
      document.head.appendChild(metaTag);
    }
    (metaTag as HTMLMetaElement).content = effectiveTheme;
  }, [appTheme]);

  // 监听系统深色偏好变化（auto 模式）
  useEffect(() => {
    if (appTheme !== 'auto') return;
    const mql = window.matchMedia('(prefers-color-scheme: dark)');
    const handler = (e: MediaQueryListEvent) => {
      const resolved = e.matches ? 'dark' : 'light';
      document.documentElement.setAttribute('data-theme', resolved);
    };
    mql.addEventListener('change', handler);
    return () => mql.removeEventListener('change', handler);
  }, [appTheme]);

  // Ctrl+B d: 切换主题（在 useTmuxKeyHandler 中处理）
  // Ctrl+B a: 导航到首页（在 useTmuxKeyHandler 中处理）
  // Ctrl+B p: 导航到论文列表（在 useTmuxKeyHandler 中处理）
  // Ctrl+B b: 导航到知识库（在 useTmuxKeyHandler 中处理）
  // Ctrl+B w: 导航到知识 Wiki（在 useTmuxKeyHandler 中处理）

  return (
    <div style={{ height: '100%' }}>
      <PaneManager />
    </div>
  );
}

/**
 * App 根组件
 */
export default function App() {
  const currentTheme = useAppStore((state: unknown) => (state as { theme: string }).theme);
  const language = useAppStore((state: unknown) => (state as { language: string }).language);
  const antdLocale = useAntdLocale();
  const { i18n } = useTranslation();
  const [effectiveTheme, setEffectiveTheme] = useState(
    currentTheme === 'auto' ? resolveSystemTheme() : currentTheme
  );

  // 同步 store 语言到 i18next
  useEffect(() => {
    if (i18n.language !== language) {
      i18n.changeLanguage(language);
    }
  }, [language, i18n]);

  useEffect(() => {
    if (currentTheme !== 'auto') {
      setEffectiveTheme(currentTheme);
      return;
    }
    const resolved = resolveSystemTheme();
    setEffectiveTheme(resolved);
    const mql = window.matchMedia('(prefers-color-scheme: dark)');
    const handler = (e: MediaQueryListEvent) => setEffectiveTheme(e.matches ? 'dark' : 'light');
    mql.addEventListener('change', handler);
    return () => mql.removeEventListener('change', handler);
  }, [currentTheme]);

  // 全局禁用 Ctrl+wheel 页面缩放
  useEffect(() => {
    const handleWheel = (e: WheelEvent) => {
      if (e.ctrlKey) {
        e.preventDefault();
      }
    };
    window.addEventListener('wheel', handleWheel, { passive: false, capture: true });
    return () => window.removeEventListener('wheel', handleWheel, { capture: true });
  }, []);

  // Ant Design 的所有种子色 token 必须用真实 hex/rgb 值，不能用 CSS 变量。
  // 原因：antd 不仅在 @ant-design/colors 的 generate() 派生主色调色板时会调用 FastColor，
  // 许多组件的 prepareComponentToken() 也会直接对 colorBgContainer/colorText/colorBorder
  // 等调用 FastColor 做颜色运算。例：antd/es/tag/style/index.js 的
  //   defaultBg = new FastColor(colorFillQuaternary).onBackground(colorBgContainer).toHexString()
  // FastColor 只识别 hex/rgb/hsl/hsv，遇到 'var(--xxx)' 会静默 fallback 到 {r:0,g:0,b:0}，
  // 导致 Tag `color="default"` 的 defaultBg 变成纯黑；再叠加 colorText（亮色模式下本来就是
  // 深色），整个 Tag 看起来就是「黑底黑字」的黑方块。
  // 这里按 effectiveTheme 在 light/dark 镜像值之间切换，镜像值必须与 styles/global.css 中
  // 对应 CSS 变量保持一致 —— 修改任一方都要同步另一方。
  const isDark = effectiveTheme === 'dark';
  const seedPrimary         = isDark ? '#c8c4be'             : '#44403c';            // --color-primary
  const seedBgContainer     = isDark ? '#1a1a1a'             : '#faf8f5';            // --bg-primary
  const seedBgElevated      = isDark ? '#333333'             : '#faf8f5';            // --bg-elevated
  const seedBgLayout        = isDark ? '#222222'             : '#f0ede8';            // --bg-secondary
  const seedText            = isDark ? '#dcdcdc'             : '#262626';            // --text-primary
  const seedTextSecondary   = isDark ? '#b0b0b0'             : '#595959';            // --text-secondary
  const seedTextTertiary    = isDark ? '#909090'             : '#767676';            // --text-tertiary
  const seedBorder          = isDark ? '#333333'             : '#e8e3db';            // --border-color
  const seedBorderSecondary = isDark ? 'rgba(255,255,255,0.08)' : '#d9d2c8';         // --border-color-light

  return (
    <ConfigProvider
      locale={antdLocale}
      theme={{
        algorithm: effectiveTheme === 'dark' ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
        token: {
          colorPrimary: seedPrimary,
          colorBgContainer: seedBgContainer,
          colorBgElevated: seedBgElevated,
          colorBgLayout: seedBgLayout,
          colorText: seedText,
          colorTextSecondary: seedTextSecondary,
          colorTextTertiary: seedTextTertiary,
          colorBorder: seedBorder,
          colorBorderSecondary: seedBorderSecondary,
          colorSuccess: isDark ? '#3e9412' : '#52c41a', // 与 --status-success 对齐
          colorError:   isDark ? '#ff7875' : '#f5222d', // 与 --text-error 对齐
          colorWarning: isDark ? '#e8b339' : '#ad6800', // 与 --text-warning 对齐
          colorInfo:    seedPrimary, // 维持原 colorInfo === colorPrimary 的语义
          colorLink:    seedPrimary,
        },
      }}
    >
      <NiceModal.Provider>
        <AntApp>
          <Layout style={{ minHeight: '100vh' }}>
            <Content style={{ height: '100vh', overflow: 'auto' }}>
              <AppContent />
            </Content>
            {/* P6 Word 加载项首次启动向导（Windows + Tauri 环境自动触发） */}
            <AddinSetupWizard />
          </Layout>
        </AntApp>
      </NiceModal.Provider>
    </ConfigProvider>
  );
}
