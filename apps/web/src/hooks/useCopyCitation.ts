/**
 * useCopyCitation — 共享「复制引用」hook
 *
 * 三个入口（PaperListPage 右键菜单 / MetadataView 按钮 / 批量选择工具栏）
 * 都用同一个 hook，保证「默认样式解析 + 引擎获取 + CSL JSON 拉取 + 渲染 + 写剪贴板」
 * 流程一致。
 *
 * 流程：
 *   1. `getStyles()` 找 isDefault: true 的样式（client.ts 缓存 2s，开销可忽略）
 *      失败/为空 → 回落到 @autonomics/citation-engine 的 DEFAULT_STYLE_ID（gb-t-7714-2015-numeric）
 *   2. `getEngineForStyle(styleId)` 拿 LRU 池里的引擎（命中 = 零开销）
 *   3. `getCslJson(itemId)` 拉后端拼好的 CSL JSON
 *   4. `engine.addItems([item]) + engine.citeCluster([id])` 渲染
 *   5. `navigator.clipboard.writeText(html)` 写剪贴板
 *
 * Tauri 桌面模式优先用 @tauri-apps/plugin-clipboard-manager（解决 webview 剪贴板权限问题）。
 */
import { useCallback } from 'react';
import { App } from 'antd';
import { useTranslation } from 'react-i18next';
import { getEngineForStyle } from '@autonomics/citation-engine/react';
import { DEFAULT_STYLE_ID } from '@autonomics/citation-engine/styles';
import type { CiteprocItem } from '@autonomics/citation-engine';
import { getStyles, getCslJson } from '../services/cslApi';

/** 桌面环境标记：Tauri WebView 内部有 __TAURI_INTERNALS__ */
const isTauri =
    typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/**
 * 把文本写入剪贴板，桌面走 Tauri 插件，Web 走 navigator.clipboard。
 * 桌面 webview 的 navigator.clipboard 经常因为非 https / 不聚焦而拒绝写入，
 * Tauri 插件绕过这个限制。
 */
async function writeClipboard(text: string): Promise<void> {
    if (isTauri) {
        try {
            const { writeText } = await import(
                '@tauri-apps/plugin-clipboard-manager'
            );
            await writeText(text);
            return;
        } catch {
            // fallthrough to web API
        }
    }
    await navigator.clipboard.writeText(text);
}

/**
 * 解析当前默认样式 id。
 *
 * 优先从 `getStyles()` 找 isDefault: true 的样式（后端 CslStyle 表的标志位）；
 * 失败或为空时回落到 bundled DEFAULT_STYLE_ID。流程同步阻塞调用方，
 * 但 client.ts 的 2s 缓存让重复调用近乎零开销。
 */
async function resolveDefaultStyleId(): Promise<string> {
    try {
        const styles = await getStyles();
        const def = styles.find(s => s.isDefault);
        if (def?.id) return def.id;
    } catch {
        // 网络错误 → 用 bundled 默认值，不阻塞用户
    }
    return DEFAULT_STYLE_ID;
}

export function useCopyCitation() {
    const { message } = App.useApp();
    const { t } = useTranslation();

    return useCallback(
        async (itemId: string): Promise<boolean> => {
            try {
                const styleId = await resolveDefaultStyleId();
                const engine = getEngineForStyle(styleId);
                const { item } = await getCslJson(itemId);
                engine.addItems([item as CiteprocItem]);
                const { html } = engine.citeCluster([
                    (item as { id: string }).id,
                ]);
                if (!html) {
                    throw new Error('citeproc returned empty citation');
                }
                await writeClipboard(html);
                message.success(
                    t('citation.copySuccess', { defaultValue: '已复制引用' }),
                );
                return true;
            } catch (err) {
                const msg = err instanceof Error ? err.message : String(err);
                message.error(
                    t('citation.copyFailed', {
                        defaultValue: '复制失败：{{error}}',
                        error: msg,
                    }),
                );
                return false;
            }
        },
        [message, t],
    );
}
