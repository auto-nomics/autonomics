import React, { useState, useCallback, useRef, useEffect, forwardRef, useMemo } from 'react';
import { App } from 'antd';
import { PdfToolbar } from './PdfToolbar';
import { PdfFloatingLayers } from './PdfFloatingLayers';
import { useAnnotations } from '../hooks/useAnnotations';
import { handleTemplateSelect } from './TemplateFloatingMenu';
import { AnnotationManager } from '../annotation/AnnotationManager';
import useKeyboardShortcut from '../../../hooks/useKeyboardShortcut';
import useAppStore, { resolveSystemTheme } from '../../../stores/useAppStore';
import styles from '../PdfViewer.module.css';

import { usePdfiumEngine } from '@autonomics/pdfium-viewer/engines/react';
import { EmbedPDF } from '@autonomics/pdfium-viewer/core/react';
import { createPluginRegistration } from '@autonomics/pdfium-viewer/core';
import { Viewport, ViewportPluginPackage } from '@autonomics/pdfium-viewer/plugin-viewport/react';
import { Scroller, ScrollPluginPackage, ScrollStrategy } from '@autonomics/pdfium-viewer/plugin-scroll/react';
import {
  DocumentManagerPluginPackage,
  DocumentContent,
  DocumentContext,
  DocumentManagerPlugin,
  useDocumentManagerCapability,
} from '@autonomics/pdfium-viewer/plugin-document-manager/react';
import { RenderLayer, RenderPluginPackage } from '@autonomics/pdfium-viewer/plugin-render/react';
import { TilingLayer, TilingPluginPackage } from '@autonomics/pdfium-viewer/plugin-tiling/react';
import { ZoomMode, ZoomPluginPackage } from '@autonomics/pdfium-viewer/plugin-zoom/react';
import { SelectionPluginPackage, SelectionLayer } from '@autonomics/pdfium-viewer/plugin-selection/react';
import { AnnotationPluginPackage, AnnotationLayer } from '@autonomics/pdfium-viewer/plugin-annotation/react';
import { FormPluginPackage } from '@autonomics/pdfium-viewer/plugin-form/react';
import { HistoryPluginPackage } from '@autonomics/pdfium-viewer/plugin-history/react';
import { Rotate, RotatePluginPackage } from '@autonomics/pdfium-viewer/plugin-rotate/react';
import { InteractionManagerPluginPackage, GlobalPointerProvider, PagePointerProvider } from '@autonomics/pdfium-viewer/plugin-interaction-manager/react';
import { SearchPluginPackage, SearchLayer } from '@autonomics/pdfium-viewer/plugin-search/react';
import { ThumbnailPluginPackage } from '@autonomics/pdfium-viewer/plugin-thumbnail/react';

// Plugin hooks re-exported through react entry points
import { useScroll } from '@autonomics/pdfium-viewer/plugin-scroll/react';
import { useZoom } from '@autonomics/pdfium-viewer/plugin-zoom/react';
import { useSearch } from '@autonomics/pdfium-viewer/plugin-search/react';
import { useSelectionCapability } from '@autonomics/pdfium-viewer/plugin-selection/react';
import { useThumbnailCapability } from '@autonomics/pdfium-viewer/plugin-thumbnail/react';

import { isTauri } from '../../../services/client';
import type { PdfEngine } from '@autonomics/pdfium-viewer';

const THUMB_SIDEBAR_WIDTH = 120;
const THUMB_MAX_WIDTH = 96;
const THUMB_ASPECT_RATIO = 1.414;

interface PdfViewerProps {
  paperId?: string;
  pdfUrl?: string;
  onContextInject?: (content: string, sourcePage: number) => void;
  onTextClick?: (text: string) => void;
  onCoordinateClick?: (coord: any) => void;
  onSearchChange?: (e: React.ChangeEvent<HTMLInputElement>) => void;
  navigation?: any;
  onPageChange?: (page: number) => void;
  anchors?: any[];
}

interface TemplateMenuState {
  visible: boolean;
  x: number;
  y: number;
  context: {
    content: string;
    source_page: number;
    title: string;
    approximate_y: number | null;
  } | null;
}

const INITIAL_TEMPLATE_MENU: TemplateMenuState = { visible: false, x: 0, y: 0, context: null };

const PdfViewer = forwardRef<HTMLDivElement, PdfViewerProps>(function PdfViewer({
  paperId,
  pdfUrl,
  onContextInject,
  onSearchChange,
  navigation,
  onPageChange,
}, ref) {
  const { engine, isLoading: engineLoading, error: engineError } = usePdfiumEngine();

  const plugins = useMemo(() => [
    createPluginRegistration(ViewportPluginPackage, { viewportGap: 10 }),
    createPluginRegistration(ScrollPluginPackage, { defaultStrategy: ScrollStrategy.Vertical }),
    createPluginRegistration(DocumentManagerPluginPackage),
    createPluginRegistration(InteractionManagerPluginPackage),
    createPluginRegistration(ZoomPluginPackage, { defaultZoomLevel: ZoomMode.FitWidth }),
    createPluginRegistration(RenderPluginPackage),
    // tileSize=768, dpr 用系统值(不 cap) —— 保持视觉质量。
    // extraRings: 1 = 预渲染一圈缓冲, 慢速滚动时下一屏已就位。
    // 快速翻屏超出缓冲范围时仍会卡, 这是 wasm 单线程的固有限制。
    createPluginRegistration(TilingPluginPackage, { tileSize: 768, overlapPx: 2.5, extraRings: 1 }),
    createPluginRegistration(SelectionPluginPackage),
    createPluginRegistration(SearchPluginPackage),
    createPluginRegistration(HistoryPluginPackage),
    createPluginRegistration(AnnotationPluginPackage),
    createPluginRegistration(FormPluginPackage),
    createPluginRegistration(ThumbnailPluginPackage, { width: 120, paddingY: 10 }),
    createPluginRegistration(RotatePluginPackage),
  ], []);

  if (engineError) {
    return (
      <div className={styles.container} style={{ display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
        <div style={{ maxWidth: 480, padding: 24, textAlign: 'center' }}>
          <h3 style={{ marginBottom: 8 }}>PDF 引擎加载失败</h3>
          <p style={{ color: 'var(--text-secondary)', marginBottom: 12, fontSize: 13 }}>
            {engineError.message}
          </p>
        </div>
      </div>
    );
  }

  if (engineLoading || !engine) {
    return (
      <div className={styles.container} style={{ display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
        <div style={{ color: 'var(--text-secondary)' }}>正在加载 PDF 引擎...</div>
      </div>
    );
  }

  return (
    <div ref={ref} className={styles.container}>
      <EmbedPDF
        engine={engine}
        plugins={plugins}
        onInitialized={async (registry: any) => {
          if (!pdfUrl) return;
          const dm = registry?.getPlugin(DocumentManagerPlugin.id)?.provides();
          if (pdfUrl.startsWith('http://') || pdfUrl.startsWith('https://')) {
            dm?.openDocumentUrl({ url: pdfUrl }).toPromise();
          } else if (isTauri && pdfUrl) {
            try {
              const { readFile } = await import('@tauri-apps/plugin-fs');
              const data = await readFile(pdfUrl);
              dm?.openDocumentBuffer({ buffer: data.buffer, name: pdfUrl }).toPromise();
            } catch (err) {
              console.error('Failed to load PDF from file:', err);
            }
          }
        }}
      >
        {({ pluginsReady }: { pluginsReady: boolean }) => (
          pluginsReady ? (
            <WasmPdfContent
              engine={engine}
              paperId={paperId}
              onContextInject={onContextInject}
              onSearchChange={onSearchChange}
              navigation={navigation}
              onPageChange={onPageChange}
            />
          ) : (
            <div className={styles.pdfArea}>
              <div style={{ color: 'var(--text-secondary)' }}>正在初始化插件...</div>
            </div>
          )
        )}
      </EmbedPDF>
    </div>
  );
});

export default React.memo(PdfViewer);

function WasmPdfContent({
  engine,
  paperId,
  onContextInject,
  onSearchChange,
  navigation,
  onPageChange,
}: {
  engine: PdfEngine | null;
  paperId?: string;
  onContextInject?: (content: string, sourcePage: number) => void;
  onSearchChange?: (e: React.ChangeEvent<HTMLInputElement>) => void;
  navigation?: any;
  onPageChange?: (page: number) => void;
}) {
  const { provides: selectionCap } = useSelectionCapability();
  const { provides: thumbnailCap } = useThumbnailCapability();

  // 默认关闭缩略图侧栏: 全量串行 renderThumb 会霸占 pdfium worker 单线程队列,
  // 让主渲染(renderPageRect)排在后面, 表现为滚动卡顿/下半部分不渲染。
  // 用户需要时手动打开, 或后续做 viewport 驱动(P1.6)后改回 true。
  const [thumbVisible, setThumbVisible] = useState(false);
  const [activeHighlight, setActiveHighlight] = useState<any>(null);
  const [popoverPosition, setPopoverPosition] = useState({ x: 0, y: 0 });
  const [templateMenu, setTemplateMenu] = useState<TemplateMenuState>(INITIAL_TEMPLATE_MENU);
  const [searchTarget, setSearchTarget] = useState<'outline' | 'chat' | 'pdf'>('outline');
  const [searchKeyword, setSearchKeyword] = useState('');

  const appTheme = useAppStore((s: any) => s.theme);
  const pdfDarkMode = (appTheme === 'auto' ? resolveSystemTheme() : appTheme) === 'dark';
  const { message } = App.useApp();
  const pdfAreaRef = useRef<HTMLDivElement>(null);
  const searchDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 缓存选区文本：右键 mousedown 会触发 InteractionManager 清空选区，
  // 但 contextmenu 事件在之后才触发，所以需要提前缓存。
  const lastSelectionRef = useRef<{ text: string; page: number; approximateY: number | null } | null>(null);

  // 阻止右键 pointerdown 传播到 InteractionManager，避免右键时清空选区高亮。
  // InteractionManager 通过 pointerdown（非 mousedown）触发 SelectionPlugin 的 onPointerDown，
  // 其中调用 onClear() 清空选区。在 pdfArea capture 阶段拦截即可阻止事件到达子元素。
  useEffect(() => {
    const el = pdfAreaRef.current;
    if (!el) return;
    const stopRightClick = (e: Event) => {
      if ((e as PointerEvent).button === 2) e.stopPropagation();
    };
    el.addEventListener('pointerdown', stopRightClick, true);
    return () => el.removeEventListener('pointerdown', stopRightClick, true);
  }, []);

  // 监听选区变化，选区存在时异步提取文本并缓存
  // 注意：onSelectionChange 回调接收 SelectionChangeEvent { documentId, selection, modeId }，
  // 实际选区在 event.selection 中（SelectionRangeX | null）。
  useEffect(() => {
    if (!selectionCap) return;
    return selectionCap.onSelectionChange((event: any) => {
      const sel = event?.selection;
      if (!sel) return;
      try {
        const textTask = selectionCap.getSelectedText();
        if (textTask && typeof textTask.wait === 'function') {
          textTask.wait((texts: string[]) => {
            const text = texts?.join('\n')?.trim();
            if (text) {
              const page = sel.start.page + 1;
              lastSelectionRef.current = { text, page, approximateY: null };
            }
          }, () => {});
        }
      } catch { /* ignore */ }
    });
  }, [selectionCap]);

  // Annotations (REST API persistence)
  const annotations = useAnnotations({ paperId: paperId != null ? String(paperId) : '', enabled: !!paperId });
  const annotationManagerRef = useRef(new AnnotationManager());

  useKeyboardShortcut(
    (e: KeyboardEvent) => (e.ctrlKey || e.metaKey) && !e.shiftKey && e.key === 'z',
    (e: KeyboardEvent) => {
      e.preventDefault();
      const cmd = annotationManagerRef.current.undo();
      if (cmd && cmd.type === 'delete') {
        annotations.create({
          page: cmd.annotation.page, text: cmd.annotation.text,
          rects: cmd.annotation.rects, color: cmd.annotation.color,
          approximate_y: cmd.annotation.approximate_y,
        });
      }
    },
    [annotations]
  );

  useKeyboardShortcut(
    (e: KeyboardEvent) => (e.ctrlKey || e.metaKey) && e.shiftKey && (e.key === 'z' || e.key === 'Z'),
    (e: KeyboardEvent) => {
      e.preventDefault();
      const cmd = annotationManagerRef.current.redo();
      if (cmd && cmd.type === 'delete') {
        annotations.remove(cmd.annotation.id as any, cmd.annotation.page as number);
      }
    },
    [annotations]
  );

  // Context menu — 始终阻止 WebKitGTK 原生右键菜单，有缓存选区时显示模板菜单
  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    // 先阻止默认菜单（WebKitGTK 的 back/forward/reload 等），再决定是否显示自定义菜单
    e.preventDefault();

    const cached = lastSelectionRef.current;
    if (!cached) return;

    // 尝试从 bounding rects 计算 approximateY
    let approximateY: number | null = null;
    if (selectionCap) {
      try {
        const bRects = selectionCap.getBoundingRects();
        const containerBCR = pdfAreaRef.current?.getBoundingClientRect();
        if (bRects?.length > 0 && containerBCR) {
          const r = bRects[0].rect;
          const centerY = r.origin.y + r.size.height / 2;
          approximateY = Math.max(0, Math.min(1, centerY / containerBCR.height));
        }
      } catch { /* ignore */ }
    }

    setTemplateMenu({
      visible: true,
      x: e.clientX,
      y: e.clientY,
      context: { content: cached.text, source_page: cached.page, title: `第 ${cached.page} 页选中内容`, approximate_y: approximateY },
    });

    lastSelectionRef.current = null;
  }, [pdfAreaRef, selectionCap]);

  const handleTemplateItemClick = useCallback((templateKey: string) => {
    handleTemplateSelect(
      templateKey,
      templateMenu.context,
      onContextInject,
      () => setTemplateMenu(INITIAL_TEMPLATE_MENU),
      message,
    );
  }, [templateMenu.context, onContextInject, message]);

  const handleCreateHighlight = useCallback(async () => {
    // 优先从缓存获取选区（plugin-based PDFium 无 DOM 文本层，window.getSelection() 为空）
    const cached = lastSelectionRef.current;
    if (!cached?.text) return;

    // selection plugin 返回的 rect 是页面设备坐标（PDF 点 × 当前缩放）。
    // 必须按"该页 DOM 元素"归一化，而不是整个 pdfArea viewport——
    // 否则页面相对 viewport 的偏移会被算进归一化结果，存进 DB 后渲染位置整体错位。
    const pageEl = pdfAreaRef.current?.querySelector(
      `[data-page-index="${cached.page - 1}"]`,
    ) as HTMLElement | null;
    const pageBCR = pageEl?.getBoundingClientRect() ?? null;
    const rects: any[] = [];
    let approximateY: number | null = null;

    if (selectionCap && pageBCR) {
      try {
        const formatted = selectionCap.getFormattedSelection();
        for (const s of formatted) {
          const r = s.rect;
          if (r.size.width < 1 || r.size.height < 1) continue;
          rects.push({
            x: Math.max(0, r.origin.x / pageBCR.width),
            y: Math.max(0, r.origin.y / pageBCR.height),
            w: Math.min(r.size.width / pageBCR.width, 1),
            h: Math.min(r.size.height / pageBCR.height, 1),
          });
        }
        const filtered = rects.filter((r: any) => r.w >= 0.001 && r.h >= 0.001);
        if (filtered.length > 0) {
          approximateY = Math.max(0, Math.min(1, filtered[0].y + filtered[0].h / 2));
        }
      } catch { /* ignore */ }
    }

    try {
      await annotations.create({
        page: cached.page, text: cached.text, approximate_y: approximateY,
        rects: rects.length > 0 ? rects : undefined, color: '#ffe066',
      });
      selectionCap?.clear();
    } catch (err) {
      console.error('创建高亮失败:', err);
    }
  }, [annotations, pdfAreaRef, selectionCap]);

  // Alt+Left 切换缩略图侧栏。
  // preventDefault 屏蔽浏览器 Alt+Left "后退" 默认行为，避免阅读时误触跳走。
  // excludeInputs:false —— Alt+方向键不产生字符输入，聊天框聚焦时也要能用。
  useKeyboardShortcut(
    (e: KeyboardEvent) => e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey && e.key === 'ArrowLeft',
    (e: KeyboardEvent) => {
      e.preventDefault();
      setThumbVisible(v => !v);
    },
    [],
    { excludeInputs: false }
  );
  const handleCloseTemplateMenu = useCallback(() => setTemplateMenu(INITIAL_TEMPLATE_MENU), []);
  const handleCloseHighlightPopover = useCallback(() => setActiveHighlight(null), []);
  const handleDeleteHighlight = useCallback((highlight: any) => {
    annotations.remove(highlight.id, highlight.page);
    annotationManagerRef.current.execute({ type: 'delete', annotation: highlight, timestamp: Date.now() });
    setActiveHighlight(null);
  }, [annotations]);

  // 点击已渲染的高亮 → 弹出删除 popover。
  // highlight.rects 是按"页 DOM 元素"归一化的（见 handleCreateHighlight），渲染时直接当百分比用。
  const handleHighlightClick = useCallback((e: React.MouseEvent, highlight: any) => {
    e.stopPropagation();
    setActiveHighlight(highlight);
    setPopoverPosition({ x: e.clientX, y: e.clientY });
  }, []);

  return (
    <DocumentContext>
      {({ activeDocumentId: docId }: { activeDocumentId: string | null }) => {
        if (!docId) return null;
        return (
          <DocumentContent documentId={docId}>
            {({ isLoading: docLoading, isLoaded }: { isLoading: boolean; isLoaded: boolean }) => (
              <InnerDocument
                engine={engine}
                docId={docId}
                paperId={paperId}
                docLoading={docLoading}
                isLoaded={isLoaded}
                pdfDarkMode={pdfDarkMode}
                thumbVisible={thumbVisible}
                pdfAreaRef={pdfAreaRef}
                onContextMenu={handleContextMenu}
                annotations={annotations}
                activeHighlight={activeHighlight}
                popoverPosition={popoverPosition}
                onDeleteHighlight={handleDeleteHighlight}
                onCloseHighlightPopover={handleCloseHighlightPopover}
                onHighlightClick={handleHighlightClick}
                templateMenu={templateMenu}
                onCloseTemplateMenu={handleCloseTemplateMenu}
                onTemplateSelect={handleTemplateItemClick}
                onCreateHighlight={handleCreateHighlight}
                thumbnailCap={thumbnailCap}
                searchTarget={searchTarget}
                setSearchTarget={setSearchTarget}
                searchKeyword={searchKeyword}
                setSearchKeyword={setSearchKeyword}
                searchDebounceRef={searchDebounceRef}
                onSearchChange={onSearchChange}
                onPageChange={onPageChange}
                navigation={navigation}
              />
            )}
          </DocumentContent>
        );
      }}
    </DocumentContext>
  );
}

interface InnerDocumentProps {
  engine: PdfEngine | null;
  docId: string;
  paperId?: string;
  docLoading: boolean;
  isLoaded: boolean;
  pdfDarkMode: boolean;
  thumbVisible: boolean;
  pdfAreaRef: React.RefObject<HTMLDivElement>;
  onContextMenu: (e: React.MouseEvent) => void;
  annotations: ReturnType<typeof useAnnotations>;
  activeHighlight: any;
  popoverPosition: { x: number; y: number };
  onDeleteHighlight: (highlight: any) => void;
  onCloseHighlightPopover: () => void;
  onHighlightClick: (e: React.MouseEvent, highlight: any) => void;
  templateMenu: TemplateMenuState;
  onCloseTemplateMenu: () => void;
  onTemplateSelect: (key: string) => void;
  onCreateHighlight: () => void;
  thumbnailCap: any;
  searchTarget: 'outline' | 'chat' | 'pdf';
  setSearchTarget: (target: 'outline' | 'chat' | 'pdf') => void;
  searchKeyword: string;
  setSearchKeyword: (keyword: string) => void;
  searchDebounceRef: React.MutableRefObject<ReturnType<typeof setTimeout> | null>;
  onSearchChange?: (e: React.ChangeEvent<HTMLInputElement>) => void;
  onPageChange?: (page: number) => void;
  navigation?: any;
}

function InnerDocument({
  engine, docId, paperId, docLoading, isLoaded, pdfDarkMode, thumbVisible, pdfAreaRef,
  onContextMenu,
  annotations, activeHighlight, popoverPosition,
  onDeleteHighlight, onCloseHighlightPopover, onHighlightClick,
  templateMenu, onCloseTemplateMenu, onTemplateSelect, onCreateHighlight,
  thumbnailCap,
  searchTarget, setSearchTarget, searchKeyword, setSearchKeyword, searchDebounceRef,
  onSearchChange,
  onPageChange, navigation,
}: InnerDocumentProps) {
  const { state: scrollState, provides: scrollProvides } = useScroll(docId);
  const { state: zoomState, provides: zoomProvides } = useZoom(docId);
  const { state: searchState, provides: searchProvides } = useSearch(docId);
  const { provides: innerSelCap } = useSelectionCapability();

  const { provides: docManagerCap } = useDocumentManagerCapability();
  const currentPage = scrollState.currentPage;

  // PDF text copy: Ctrl+C → selection plugin extracts text → Tauri clipboard API writes it
  // The selection plugin uses glyph-based selection (no DOM text layer),
  // so browser native copy can't work. We must wire Ctrl+C ourselves.
  useKeyboardShortcut(
    (e: KeyboardEvent) => (e.ctrlKey || e.metaKey) && e.key === 'c',
    () => {
      if (!innerSelCap || !docId) return;
      innerSelCap.copyToClipboard(docId);
    },
    [innerSelCap, docId],
  );

  useEffect(() => {
    if (!innerSelCap) return;
    return innerSelCap.onCopyToClipboard(async ({ text }) => {
      if (!text) return;
      // pdfium FPDFText_GetText returns per-line text with \r\n — merge
      // paragraph-internal line breaks into spaces so pasted text flows
      // as continuous paragraphs instead of one line per sentence.
      const processed = text
        .replace(/\r\n/g, '\n')                // normalize line endings
        .replace(/\n{2,}/g, '\n\n')            // preserve paragraph breaks
        .replace(/([^\n])\n([^\n])/g, '$1 $2') // single \n → space
        .replace(/ +/g, ' ')                   // collapse extra spaces
        .trim();
      if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
        const { writeText } = await import('@tauri-apps/plugin-clipboard-manager');
        await writeText(processed);
        return;
      }
      await navigator.clipboard.writeText(processed);
    });
  }, [innerSelCap]);

  const totalPages = scrollState.totalPages;

  // Zoom: Ctrl+scroll + Tauri pinch-zoom IPC
  // Handles both direct Ctrl+wheel events (scroll) and Tauri IPC (touchpad pinch).
  // 用 ref 跟踪 zoomState/zoomProvides，避免每次 zoom 变化都重建监听器（否则累积窗口失效、IPC 抖动）。
  const zoomStateRef = useRef(zoomState);
  zoomStateRef.current = zoomState;
  const zoomProvidesRef = useRef(zoomProvides);
  zoomProvidesRef.current = zoomProvides;

  useEffect(() => {
    const el = pdfAreaRef.current;
    if (!el || !docId) return;

    let wheelTimeout: ReturnType<typeof setTimeout> | null = null;
    let accumulatedScale = 1;
    let initialZoom = 0;
    let pendingDy = 0;
    let lastApplyTime = 0;
    const APPLY_INTERVAL_MS = 60;

    const flushPending = () => {
      if (Math.abs(pendingDy) <= 0.001 || initialZoom <= 0) {
        pendingDy = 0;
        return;
      }
      const provides = zoomProvidesRef.current;
      if (!provides) {
        pendingDy = 0;
        return;
      }
      const factor = 1 - pendingDy * 0.008;
      accumulatedScale *= factor;
      accumulatedScale = Math.max(0.1, Math.min(10, accumulatedScale));
      const target = initialZoom * accumulatedScale;
      const rect = el.getBoundingClientRect();
      provides.requestZoom(target, {
        vx: rect.width / 2,
        vy: rect.height / 2,
      });
      pendingDy = 0;
    };

    const commitZoom = () => {
      flushPending();
      accumulatedScale = 1;
      initialZoom = 0;
      pendingDy = 0;
      lastApplyTime = 0;
      wheelTimeout = null;
    };

    const applyZoomDelta = (dy: number) => {
      if (wheelTimeout === null) {
        initialZoom = zoomStateRef.current.currentZoomLevel ?? 1;
        accumulatedScale = 1;
        pendingDy = 0;
        lastApplyTime = 0;
      } else {
        clearTimeout(wheelTimeout);
      }
      pendingDy += dy;

      const now = performance.now();
      if (now - lastApplyTime >= APPLY_INTERVAL_MS) {
        lastApplyTime = now;
        flushPending();
      }
      wheelTimeout = setTimeout(commitZoom, 200);
    };

    const handleWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      e.stopPropagation();
      applyZoomDelta(e.deltaY);
    };

    el.addEventListener('wheel', handleWheel, { passive: false, capture: true });

    // Tauri IPC for touchpad pinch gestures
    let unlistenFn: (() => void) | null = null;
    let ipcCancelled = false;
    if (isTauri) {
      (async () => {
        const { listen } = await import('@tauri-apps/api/event');
        if (ipcCancelled) return;
        unlistenFn = await listen<number>('pinch-zoom', (event) => {
          applyZoomDelta(event.payload);
        });
      })();
    }

    return () => {
      el.removeEventListener('wheel', handleWheel, { capture: true });
      ipcCancelled = true;
      unlistenFn?.();
      if (wheelTimeout) clearTimeout(wheelTimeout);
    };
  }, [docId, pdfAreaRef]);

  useEffect(() => {
    if (onPageChange && currentPage > 0) onPageChange(currentPage);
  }, [currentPage, onPageChange]);

  useEffect(() => {
    if (!scrollProvides || !navigation) return;
    if (navigation.type === 'page' && navigation.page) {
      scrollProvides.scrollToPage({ pageNumber: navigation.page });
    }
  }, [navigation, scrollProvides]);

  // 桌面端缩略图在 WasmPdfViewer 内联渲染。
  // totalPages 来自 useScroll hook（plugin-scroll/shared/hooks/use-scroll.ts），
  // 初始值为 1，通过 onPageChange 和 onLayoutReady 事件更新为真实页数。
  // 如果 onLayoutReady 未被监听，totalPages 会在用户首次滚动后才更新，
  // 导致缩略图在打开文档时不渲染（只循环 1 次）。
  const [thumbnailUrls, setThumbnailUrls] = useState<Map<number, string>>(new Map());
  // Ref 同步当前 thumbnailUrls, 用于 unmount / docId 切换时批量 revoke blob URL。
  // 不能直接在 setState 闭包里 revoke —— React strict mode 会重复调用 setter
  // 导致 url 被 revoke 多次（无害但浪费）或被 revoke 后又被读取（broken image）。
  const thumbnailUrlsRef = useRef<Map<number, string>>(new Map());
  useEffect(() => {
    thumbnailUrlsRef.current = thumbnailUrls;
  }, [thumbnailUrls]);

  useEffect(() => {
    if (!thumbnailCap || !docId || !thumbVisible) return;
    const scope = thumbnailCap.forDocument(docId);
    if (!scope) return;

    // docId / thumbVisible 切换时, revoke 上一份所有 url 并 clear state。
    // 旧实现 (prevUrlsRef.forEach revoke) 在每次 setThumbnailUrls 累积更新时
    // 触发, 把"还在用的上一轮 url"全部 revoke 掉 → 缩略图从第 2 张起变 broken image。
    const prevUrls = thumbnailUrlsRef.current;
    prevUrls.forEach(url => URL.revokeObjectURL(url));
    thumbnailUrlsRef.current = new Map();
    setThumbnailUrls(new Map());

    let cancelled = false;
    const dpr = window.devicePixelRatio || 1;

    (async () => {
      for (let i = 0; i < totalPages; i++) {
        if (cancelled) break;
        try {
          const blob = await scope.renderThumb(i, dpr).toPromise();
          if (!cancelled && blob) {
            const url = URL.createObjectURL(blob);
            setThumbnailUrls(prev => new Map(prev).set(i, url));
          }
        } catch { /* skip */ }
      }
    })();

    return () => { cancelled = true; };
  }, [thumbnailCap, docId, thumbVisible, totalPages]);

  // 组件 unmount: revoke 所有当前累积的 blob URL, 避免内存泄漏。
  useEffect(() => {
    return () => {
      thumbnailUrlsRef.current.forEach(url => URL.revokeObjectURL(url));
      thumbnailUrlsRef.current = new Map();
    };
  }, []);

  // Search UI handlers
  const handleSearchInputChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value;
    setSearchKeyword(value);
    onSearchChange?.(e);

    if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current);
    if (!searchProvides || !value.trim()) return;
    searchDebounceRef.current = setTimeout(() => {
      searchProvides.searchAllPages(value.trim());
    }, 300);
  }, [searchProvides, onSearchChange]);

  const handleSearchTargetChange = useCallback((target: 'outline' | 'chat' | 'pdf') => setSearchTarget(target), []);

  const handleSearchKeyDown = useCallback((e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' && searchKeyword.trim()) {
      if (searchProvides) {
        searchProvides.searchAllPages(searchKeyword.trim());
      }
    }
  }, [searchKeyword, searchProvides]);

  const pdfSearchMatches = searchState.results ?? [];
  const currentMatchIndex = searchState.activeResultIndex ?? -1;

  return (
    <div style={{ display: 'flex', flex: 1, overflow: 'hidden', height: '100%' }}>
      {/* Thumbnail sidebar */}
      {thumbVisible && totalPages > 0 && (
        <div
          className={`auto-scrollbar ${pdfDarkMode ? 'pdf-dark-invert' : ''}`}
          style={{
            width: THUMB_SIDEBAR_WIDTH, flexShrink: 0, overflow: 'auto',
            borderRight: '1px solid var(--border-color)',
            background: 'var(--bg-secondary)', padding: '8px 0',
          }}
        >
          {Array.from({ length: totalPages }, (_, i) => {
            const thumbUrl = thumbnailUrls.get(i);
            const isCurrent = i === currentPage - 1;
            return (
              <div
                key={i}
                onClick={() => scrollProvides?.scrollToPage({ pageNumber: i + 1 })}
                style={{
                  cursor: 'pointer', margin: '4px 8px', padding: 4, borderRadius: 4,
                  overflow: 'hidden', display: 'flex', flexDirection: 'column', alignItems: 'center',
                  border: isCurrent ? '2px solid var(--color-primary)' : '2px solid transparent',
                  background: isCurrent ? 'var(--color-primary-bg)' : 'var(--bg-primary)',
                  boxShadow: isCurrent ? '0 0 4px var(--color-primary-shadow)' : 'var(--shadow-sm)',
                  transition: 'all 0.2s ease',
                }}
              >
                {thumbUrl ? (
                  <img src={thumbUrl} alt={`Page ${i + 1}`} width={THUMB_MAX_WIDTH}
                    className="pdfium-thumb-image"
                    style={{ borderRadius: 2 }} draggable={false} />
                ) : (
                  <div style={{
                    width: THUMB_MAX_WIDTH, height: Math.round(THUMB_MAX_WIDTH * THUMB_ASPECT_RATIO),
                    display: 'flex', alignItems: 'center', justifyContent: 'center',
                    fontSize: 10, color: 'var(--text-tertiary)', background: 'var(--bg-tertiary)', borderRadius: 2,
                  }}>
                    {i + 1}
                  </div>
                )}
                <div style={{
                  textAlign: 'center', fontSize: 10, marginTop: 2,
                  color: isCurrent ? 'var(--color-primary)' : 'var(--text-secondary)',
                  fontWeight: isCurrent ? 600 : 400,
                  whiteSpace: 'nowrap',
                }}>
                  {isCurrent ? `${i + 1} / ${totalPages}` : i + 1}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {/* Main content */}
      <div style={{ display: 'flex', flexDirection: 'column', flex: 1, overflow: 'hidden' }}>
        <PdfToolbar
          searchTarget={searchTarget}
          searchKeyword={searchKeyword}
          pdfSearching={searchState.loading}
          pdfSearchMatches={pdfSearchMatches}
          currentMatchIndex={currentMatchIndex}
          onSearchTargetChange={handleSearchTargetChange}
          onSearchChange={handleSearchInputChange}
          onSearchKeyDown={handleSearchKeyDown}
          onPrevMatch={() => searchProvides?.previousResult()}
          onNextMatch={() => searchProvides?.nextResult()}
          onCreateHighlight={onCreateHighlight}
        />

        <div
          ref={pdfAreaRef}
          className={`auto-scrollbar ${styles.pdfArea} ${pdfDarkMode ? 'pdf-dark-invert' : ''}`}
          onContextMenu={onContextMenu}
          style={{ flex: 1 }}
        >
          {docLoading && (
            <div style={{ color: 'var(--text-secondary)' }}>正在加载文档...</div>
          )}
          {isLoaded && (
            <GlobalPointerProvider documentId={docId}>
              <Viewport
                documentId={docId}
                style={{
                  width: '100%', height: '100%', flexGrow: 1,
                  backgroundColor: 'var(--bg-tertiary)', overflow: 'auto',
                }}
              >
                  <Scroller
                    documentId={docId}
                    renderPage={({ pageIndex }: { pageIndex: number }) => (
                      <div data-page-index={pageIndex} style={{ width: '100%', height: '100%', position: 'relative' }}>
                        <Rotate documentId={docId} pageIndex={pageIndex} style={{ backgroundColor: 'var(--pdf-bg)' }}>
                          <PagePointerProvider documentId={docId} pageIndex={pageIndex}>
                            <RenderLayer documentId={docId} pageIndex={pageIndex} style={{ pointerEvents: 'none' }} />
                            <TilingLayer documentId={docId} pageIndex={pageIndex} style={{ pointerEvents: 'none' }} />
                            <SearchLayer documentId={docId} pageIndex={pageIndex} />
                            <SelectionLayer documentId={docId} pageIndex={pageIndex} />
                            <AnnotationLayer documentId={docId} pageIndex={pageIndex} />
                            {/* 用户保存的高亮：rects 按页 DOM 元素归一化，渲染时直接当百分比 */}
                            {annotations.getPageHighlights(pageIndex + 1).map((h: any) => {
                              if (!h.rects || h.rects.length === 0) return null;
                              const isActive = activeHighlight?.id === h.id;
                              return h.rects.map((r: any, i: number) => (
                                <div
                                  key={`${h.id}-${i}`}
                                  onClick={(e) => onHighlightClick(e, h)}
                                  style={{
                                    position: 'absolute',
                                    left: `${r.x * 100}%`,
                                    top: `${r.y * 100}%`,
                                    width: `${r.w * 100}%`,
                                    height: `${r.h * 100}%`,
                                    backgroundColor: h.color || '#ffe066',
                                    opacity: isActive ? 0.75 : 0.4,
                                    cursor: 'pointer',
                                    zIndex: 9,
                                    borderRadius: 2,
                                    transition: 'opacity 0.15s',
                                  }}
                                />
                              ));
                            })}
                          </PagePointerProvider>
                        </Rotate>
                      </div>
                    )}
                  />
              </Viewport>
            </GlobalPointerProvider>
          )}
        </div>
      </div>

      {/* Floating layers */}
      <PdfFloatingLayers
        imagePreviewUrl={null}
        onCloseImagePreview={() => {}}
        templateMenu={templateMenu}
        onCloseTemplateMenu={onCloseTemplateMenu}
        onTemplateSelect={onTemplateSelect}
        pdfiumActiveHighlight={activeHighlight}
        pdfiumPopoverPosition={popoverPosition}
        onDeleteHighlight={onDeleteHighlight}
        onCloseHighlightPopover={onCloseHighlightPopover}
      />
    </div>
  );
}
