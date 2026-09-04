/**
 * 全局应用状态管理 Store
 *
 * 管理全局 UI 状态：侧边栏、主题、活跃面板等。
 */
import { create } from 'zustand';
import { persist } from 'zustand/middleware';

function resolveSystemTheme() {
  if (typeof window === 'undefined') return 'light';
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

interface AppState {
  sidebarCollapsed: boolean;
  theme: string;
  activePanel: string | null;
  language: string;
  setSidebarCollapsed: (collapsed: boolean) => void;
  toggleSidebar: () => void;
  setTheme: (theme: string) => void;
  getEffectiveTheme: () => string;
  toggleTheme: () => void;
  setActivePanel: (panel: string | null) => void;
  setLanguage: (language: string) => void;
  reset: () => void;
}

const useAppStore = create<AppState>()(
  persist(
    (set, get) => ({
      // 侧边栏折叠状态
      sidebarCollapsed: false,
      // 主题设置：'light' | 'dark' | 'auto'
      theme: 'light',
      // 当前活跃的面板 ID
      activePanel: null,
      // 语言设置：'zh' | 'en'
      language: 'zh',

      // 设置侧边栏折叠状态
      setSidebarCollapsed: (collapsed: boolean) => set({ sidebarCollapsed: collapsed }),
      // 切换侧边栏折叠状态
      toggleSidebar: () => set((state: AppState) => ({ sidebarCollapsed: !state.sidebarCollapsed })),

      // 设置主题（light/dark/auto）
      setTheme: (theme: string) => {
        const valid = ['light', 'dark', 'auto'];
        if (!valid.includes(theme)) return;
        set({ theme });
      },
      // 获取生效的主题（auto 模式下会根据系统设置返回 light 或 dark）
      getEffectiveTheme: () => {
        const theme = get().theme;
        if (theme === 'auto') return resolveSystemTheme();
        return theme;
      },
      // 切换主题（在 light 和 dark 之间切换）
      toggleTheme: () => set((state: AppState) => ({
        theme: state.theme === 'light' ? 'dark' : 'light',
      })),

      // 设置当前活跃面板
      setActivePanel: (panel: string | null) => set({ activePanel: panel }),
      // 设置语言
      setLanguage: (language: string) => {
        if (!['zh', 'en'].includes(language)) return;
        set({ language });
      },
      // 重置所有状态为默认值
      reset: () => set({
        sidebarCollapsed: false,
        theme: 'light',
        activePanel: null,
        language: 'zh',
      }),
    }),
    {
      name: 'autonomics-app-storage',
      partialize: (state: AppState) => ({
        sidebarCollapsed: state.sidebarCollapsed,
        theme: state.theme,
        activePanel: state.activePanel,
        language: state.language,
      }),
    }
  )
);

export { resolveSystemTheme };
export default useAppStore;
