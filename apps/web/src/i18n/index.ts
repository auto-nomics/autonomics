import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

// 所有语言的命名空间
const namespaces = [
  'common', 'paperList', 'paperReader',
  'chat', 'panes', 'template',
] as const;

export type Namespace = typeof namespaces[number];

// 支持的语言列表
export const supportedLanguages = ['zh', 'en'] as const;
export type SupportedLanguage = typeof supportedLanguages[number];

// 异步加载翻译资源的函数
async function loadResources(lng: string, ns: string): Promise<Record<string, string>> {
  try {
    const modules = import.meta.glob<{ default: Record<string, string> }>(
      './locales/*/*.json',
    ) as Record<string, () => Promise<{ default: Record<string, string> }>>;
    const path = `./locales/${lng}/${ns}.json`;
    const loader = modules[path];
    if (!loader) return {};
    const mod = await loader();
    return mod.default;
  } catch {
    return {};
  }
}

// 同步加载的初始资源（zh 作为默认语言）
import commonZh from './locales/zh/common.json';
import paperListZh from './locales/zh/paperList.json';
import paperReaderZh from './locales/zh/paperReader.json';
import chatZh from './locales/zh/chat.json';
import panesZh from './locales/zh/panes.json';
import templateZh from './locales/zh/template.json';

const defaultResources = {
  zh: {
    common: commonZh,
    paperList: paperListZh,
    paperReader: paperReaderZh,
    chat: chatZh,
    panes: panesZh,
    template: templateZh,
  },
};

// 从 localStorage 或浏览器语言检测用户偏好
function detectLanguage(): string {
  const stored = localStorage.getItem('i18n-language');
  if (stored && supportedLanguages.includes(stored as SupportedLanguage)) {
    return stored;
  }
  const browserLang = navigator.language.split('-')[0];
  if (supportedLanguages.includes(browserLang as SupportedLanguage)) {
    return browserLang;
  }
  return 'zh';
}

i18n.use(initReactI18next).init({
  resources: defaultResources,
  lng: detectLanguage(),
  fallbackLng: 'zh',
  supportedLngs: [...supportedLanguages],
  ns: [...namespaces],
  defaultNS: 'common',
  interpolation: {
    escapeValue: false,
  },
});

// 语言切换时保存偏好
i18n.on('languageChanged', (lng: string) => {
  localStorage.setItem('i18n-language', lng);
});

export default i18n;
export { namespaces };
