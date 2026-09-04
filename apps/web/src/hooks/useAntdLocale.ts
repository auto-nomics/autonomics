/**
 * Autonomics Antd 国际化语言包 Hook
 *
 * 根据用户设置的语言（useAppStore.language）返回对应的 Ant Design 语言包。
 * 用于 ConfigProvider 的 locale 属性，使 Ant Design 组件（日期选择器、分页器、
 * 表格空状态等）显示对应语言的文本。
 *
 * 支持的语言：
 * - zh: 中文（简体）
 * - en: 英文
 *
 * @module useAntdLocale
 */
import zhCN from 'antd/locale/zh_CN';
import enUS from 'antd/locale/en_US';
import useAppStore from '../stores/useAppStore';

/**
 * 语言代码到 Antd 语言包的映射表
 */
const LOCALE_MAP: Record<string, typeof zhCN> = { zh: zhCN, en: enUS };

/**
 * 获取当前语言对应的 Antd 语言包
 *
 * @returns 当前语言的 Antd 语言包对象，默认返回中文语言包
 */
export function useAntdLocale() {
  const language = useAppStore((s: any) => s.language);
  return LOCALE_MAP[language] || zhCN;
}
