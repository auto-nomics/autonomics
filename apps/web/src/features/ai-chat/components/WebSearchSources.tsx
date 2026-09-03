/**
 * WebSearchSources.jsx
 * ============================================================
 * 网络搜索来源列表组件
 * 显示 AI 回复底部参考的搜索来源链接
 * ============================================================
 */

import React from 'react';
import { GlobalOutlined } from '@ant-design/icons';
import { useTranslation } from 'react-i18next';

/**
 * WebSearchSources 组件
 * 显示网络搜索来源列表
 *
 * @param {Object} props - 组件属性
 * @param {Array} props.sources - 来源列表 [{ title, url }]
 */
const WebSearchSources = ({ sources }: { sources: Array<{ title: string; url: string }> }) => {
    const { t } = useTranslation('chat');
    if (!sources || sources.length === 0) return null;

    return (
        <div
            style={{
                fontSize: 12,
                color: 'var(--text-tertiary, #8c8c8c)', // 文字颜色：使用全局辅助文字颜色变量
                marginTop: 6,
                padding: '6px 8px',
                background: 'var(--bg-secondary, #f5f5f5)', // 背景色：使用全局次级背景色变量
                borderRadius: 6,
                border: '1px solid var(--border-color, #f0f0f0)',
                alignSelf: 'stretch',
            }}
        >
            {/* 搜索来源标题行 */}
            <div style={{
                display: 'flex',
                alignItems: 'center',
                gap: 4,
                marginBottom: sources.length > 0 ? 4 : 0,
            }}>
                <GlobalOutlined className="accent-link-text" style={{ fontSize: 12 }} />
                <span style={{ fontWeight: 500 }}>{t('webSearch')}</span>
                <span style={{ color: 'var(--text-tertiary, #8c8c8c)' }}>· {t('referencedSources', { count: sources.length })}</span>
            </div>
            {/* 搜索来源链接列表 */}
            <div style={{
                display: 'flex',
                flexWrap: 'wrap',
                gap: '4px 8px',
            }}>
                {sources.map((source: { title: string; url: string }, srcIdx: number) => (
                    <a
                        key={srcIdx}
                        href={source.url}
                        target="_blank"
                        rel="noopener noreferrer"
                        title={source.url}
                        style={{
                            fontSize: 11,
                            color: 'var(--text-tertiary, #8c8c8c)', // 文字颜色：使用全局辅助文字颜色变量
                            textDecoration: 'none',
                            background: 'var(--bg-primary, #ffffff)', // 背景色：使用全局主背景色变量
                            border: '1px solid var(--border-color-light, #e0e0e0)', // 边框：使用全局浅色边框变量
                            borderRadius: 4,
                            padding: '1px 6px',
                            maxWidth: '100%',
                            overflow: 'hidden',
                            textOverflow: 'ellipsis',
                            whiteSpace: 'nowrap',
                            display: 'inline-block',
                            transition: 'border-color 0.2s',
                        }}
                        onMouseEnter={(e) => { e.currentTarget.style.borderColor = 'var(--color-primary)'; }}
                        onMouseLeave={(e) => { e.currentTarget.style.borderColor = 'var(--border-color-light, #e0e0e0)'; }}
                    >
                        {source.title}
                    </a>
                ))}
            </div>
        </div>
    );
};

export default React.memo(WebSearchSources);
