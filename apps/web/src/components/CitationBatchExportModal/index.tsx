/**
 * 批量引用导出模态框
 *
 * 输入：`initialItemIds`（从论文列表多选按钮进入）
 * 输出：用户选择样式 + 输出模式 → 生成引用文本 → 复制 / 下载
 *
 * 流程：
 *   1. `getStyles()` 拿样式列表（含 isDefault 标记）→ 默认选 isDefault 那条
 *   2. `getCslJson(itemId)` × N 拿 CSL JSON
 *   3. `getEngineForStyle(styleId)` 拿引擎（命中缓存近乎零开销）
 *   4. 输出模式：
 *      - `bibliography`：`engine.bibliography()` 一次性输出参考文献表
 *      - `citations`：`engine.citeCluster([id])` 逐条输出 in-text 引用
 *   5. 复制 / 下载 .txt / .html
 *
 * @module components/CitationBatchExportModal
 */

/**
 * Minimal HTML sanitizer for citeproc-js bibliography output.
 *
 * citeproc-js 输出含 `<div class="csl-entry">` 等 formatting 标签, 但数据源是
 * 用户导入的题录 metadata (title / author)。citeproc 通常会 escape, 但为了
 * defense-in-depth 在 dangerouslySetInnerHTML 入口加一层 DOM walk:
 * - 删 script / iframe / object / embed / link / style / meta
 * - 删所有 on* 属性
 * - 删 javascript: / data:text/html 的 href / src
 *
 * 不引 DOMPurify 依赖 (体积), 用 browser 内置 DOMParser 足够。
 */
function sanitizeCitationHtml(html: string): string {
    if (typeof window === 'undefined') return html;
    try {
        const doc = new DOMParser().parseFromString(html, 'text/html');
        doc
            .querySelectorAll('script, iframe, object, embed, link, style, meta')
            .forEach((el) => el.remove());
        doc.querySelectorAll('*').forEach((el) => {
            [...(el as HTMLElement).attributes].forEach((attr) => {
                const name = attr.name.toLowerCase();
                const val = attr.value.toLowerCase().trim();
                if (name.startsWith('on')) {
                    el.removeAttribute(attr.name);
                } else if (
                    (name === 'href' || name === 'src') &&
                    (val.startsWith('javascript:') || val.startsWith('data:text/html'))
                ) {
                    el.removeAttribute(attr.name);
                }
            });
        });
        return doc.body.innerHTML;
    } catch {
        // DOMParser 失败时 fallback 到原始 HTML (citeproc 通常已 escape)
        return html;
    }
}

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import NiceModal, { useModal } from '@ebay/nice-modal-react';
import {
    Modal,
    Button,
    Select,
    Radio,
    Space,
    Typography,
    Alert,
    Spin,
    App,
    Tag,
    Empty,
    Table,
} from 'antd';
import type { TableColumnsType } from 'antd';
import {
    CopyOutlined,
    DownloadOutlined,
    FileTextOutlined,
} from '@ant-design/icons';
import { getEngineForStyle } from '@autonomics/citation-engine/react';
import { DEFAULT_STYLE_ID } from '@autonomics/citation-engine/styles';
import type { CiteprocItem } from '@autonomics/citation-engine';
import { getStyles, getCslJson } from '../../services/cslApi';
import type { CslStyleSummary } from '../../services/cslApi';

const { Text, Paragraph } = Typography;

export interface CitationBatchExportModalProps {
    /** 预选中的 item ids（来自 PaperTable 多选） */
    initialItemIds?: string[];
}

type OutputMode = 'bibliography' | 'citations';

interface ResolvedItem {
    itemId: string;
    title?: string;
    csl: CiteprocItem;
    error?: string;
}

/**
 * 触发浏览器下载一个文本文件。
 */
function downloadText(filename: string, content: string, mime = 'text/plain'): void {
    const blob = new Blob([content], { type: `${mime};charset=utf-8` });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
}

const CitationBatchExportModal = NiceModal.create(
    ({ initialItemIds = [] }: CitationBatchExportModalProps) => {
        const modal = useModal();
        const { message } = App.useApp();
        const [styles, setStyles] = useState<CslStyleSummary[]>([]);
        const [styleId, setStyleId] = useState<string>(DEFAULT_STYLE_ID);
        const [mode, setMode] = useState<OutputMode>('bibliography');
        const [items, setItems] = useState<ResolvedItem[]>([]);
        const [loading, setLoading] = useState(true);
        const [rendering, setRendering] = useState(false);
        const [output, setOutput] = useState<string>('');
        const [selectedIds, setSelectedIds] = useState<Set<string>>(
            () => new Set(initialItemIds),
        );

        // 拉样式 + 初始 CSL JSON
        useEffect(() => {
            let cancelled = false;
            (async () => {
                setLoading(true);
                try {
                    const list = await getStyles();
                    if (cancelled) return;
                    setStyles(list);
                    const def = list.find(s => s.isDefault);
                    setStyleId(def?.id ?? DEFAULT_STYLE_ID);

                    // 并行拉 CSL JSON（避免 N 个串行 RTT）
                    const results = await Promise.all(
                        initialItemIds.map(async id => {
                            try {
                                const { item } = await getCslJson(id);
                                return {
                                    itemId: id,
                                    csl: item as CiteprocItem,
                                    title:
                                        (item as { title?: string }).title ??
                                        id,
                                } satisfies ResolvedItem;
                            } catch (err) {
                                return {
                                    itemId: id,
                                    csl: { id, type: 'article' } as CiteprocItem,
                                    title: id,
                                    error:
                                        err instanceof Error
                                            ? err.message
                                            : String(err),
                                } satisfies ResolvedItem;
                            }
                        }),
                    );
                    if (cancelled) return;
                    setItems(results);
                } catch (err) {
                    if (!cancelled) {
                        message.error(
                            err instanceof Error ? err.message : String(err),
                        );
                    }
                } finally {
                    if (!cancelled) setLoading(false);
                }
            })();
            return () => {
                cancelled = true;
            };
            // initialItemIds 是 props，进入 modal 时固定，不再重新拉
            // eslint-disable-next-line react-hooks/exhaustive-deps
        }, []);

        const resolvedSelected = useMemo(
            () => items.filter(it => selectedIds.has(it.itemId) && !it.error),
            [items, selectedIds],
        );

        const handleRender = useCallback(async () => {
            if (resolvedSelected.length === 0) {
                message.warning('请至少选择一篇文献');
                return;
            }
            setRendering(true);
            try {
                const engine = getEngineForStyle(styleId);
                engine.addItems(resolvedSelected.map(r => r.csl));
                if (mode === 'bibliography') {
                    const bib = engine.bibliography();
                    setOutput(
                        `${bib.bibstart}${bib.entries.join('\n')}${bib.bibend}`,
                    );
                } else {
                    const lines = resolvedSelected.map(r => {
                        const { html } = engine.citeCluster([r.csl.id]);
                        return html;
                    });
                    setOutput(lines.join('\n\n'));
                }
            } catch (err) {
                message.error(
                    err instanceof Error ? err.message : String(err),
                );
            } finally {
                setRendering(false);
            }
        }, [resolvedSelected, styleId, mode, message]);

        const handleCopy = useCallback(async () => {
            if (!output) return;
            try {
                await navigator.clipboard.writeText(output);
                message.success('已复制到剪贴板');
            } catch (err) {
                message.error(
                    err instanceof Error ? err.message : String(err),
                );
            }
        }, [output, message]);

        const handleDownloadTxt = useCallback(() => {
            if (!output) return;
            downloadText('citations.txt', output);
        }, [output]);

        const handleDownloadHtml = useCallback(() => {
            if (!output) return;
            const html = `<!doctype html>
<html><head><meta charset="utf-8"><title>Citations</title></head>
<body>${output}</body></html>`;
            downloadText('citations.html', html, 'text/html');
        }, [output]);

        return (
            <Modal
                open={modal.visible}
                onCancel={() => modal.hide()}
                title="批量导出引用"
                width={720}
                footer={
                    <Space>
                        <Button onClick={() => modal.hide()}>关闭</Button>
                    </Space>
                }
            >
                {loading ? (
                    <Spin tip="加载中…">
                        <div style={{ height: 200 }} />
                    </Spin>
                ) : items.length === 0 ? (
                    <Empty description="没有可导出的文献" />
                ) : (
                    <Space direction="vertical" size="middle" style={{ width: '100%' }}>
                        <Space wrap>
                            <span>样式：</span>
                            <Select
                                style={{ minWidth: 240 }}
                                value={styleId}
                                onChange={setStyleId}
                                options={styles.map(s => ({
                                    value: s.id,
                                    label: s.title,
                                }))}
                            />
                            <span>输出：</span>
                            <Radio.Group
                                value={mode}
                                onChange={e => setMode(e.target.value)}
                            >
                                <Radio.Button value="bibliography">
                                    参考文献表
                                </Radio.Button>
                                <Radio.Button value="citations">
                                    in-text 引用
                                </Radio.Button>
                            </Radio.Group>
                        </Space>

                        <div>
                            <Paragraph type="secondary" style={{ marginBottom: 8 }}>
                                文献（{items.length} 篇，已选 {resolvedSelected.length} 篇）
                            </Paragraph>
                            <Table<ResolvedItem>
                                virtual
                                size="small"
                                rowKey="itemId"
                                dataSource={items}
                                scroll={{ y: 240 }}
                                pagination={false}
                                rowSelection={{
                                    selectedRowKeys: [...selectedIds],
                                    onChange: keys =>
                                        setSelectedIds(new Set(keys as string[])),
                                    getCheckboxProps: r => ({
                                        disabled: !!r.error,
                                    }),
                                }}
                                columns={
                                    [
                                        {
                                            title: '文献',
                                            dataIndex: 'title',
                                            ellipsis: true,
                                            render: (_, r) =>
                                                r.error ? (
                                                    <Space size={4}>
                                                        <Text type="secondary" ellipsis>
                                                            {r.title}
                                                        </Text>
                                                        <Tag color="red" style={{ margin: 0 }}>
                                                            加载失败
                                                        </Tag>
                                                    </Space>
                                                ) : (
                                                    <Text ellipsis>{r.title}</Text>
                                                ),
                                        },
                                    ] as TableColumnsType<ResolvedItem>
                                }
                            />
                        </div>

                        <Space>
                            <Button
                                type="primary"
                                icon={<FileTextOutlined />}
                                onClick={handleRender}
                                loading={rendering}
                                disabled={resolvedSelected.length === 0}
                            >
                                生成
                            </Button>
                            <Button
                                icon={<CopyOutlined />}
                                onClick={handleCopy}
                                disabled={!output}
                            >
                                复制
                            </Button>
                            <Button
                                icon={<DownloadOutlined />}
                                onClick={handleDownloadTxt}
                                disabled={!output}
                            >
                                下载 .txt
                            </Button>
                            <Button
                                icon={<DownloadOutlined />}
                                onClick={handleDownloadHtml}
                                disabled={!output || mode !== 'bibliography'}
                            >
                                下载 .html
                            </Button>
                        </Space>

                        {output && (
                            <Alert
                                type="info"
                                message={
                                    <div
                                        style={{
                                            maxHeight: 240,
                                            overflowY: 'auto',
                                            whiteSpace: 'pre-wrap',
                                        }}
                                        dangerouslySetInnerHTML={
                                            mode === 'bibliography'
                                                ? { __html: sanitizeCitationHtml(output) }
                                                : undefined
                                        }
                                    >
                                        {mode === 'citations' ? output : null}
                                    </div>
                                }
                            />
                        )}
                    </Space>
                )}
            </Modal>
        );
    },
);

export default CitationBatchExportModal;
