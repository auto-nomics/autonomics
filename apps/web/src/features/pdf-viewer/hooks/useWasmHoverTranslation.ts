/**
 * WasmPdfViewer 悬停翻译 Hook（v2 — 基于 PdfPageGeometry 字符流）
 *
 * 链路（三步，全部对齐到 geo.text 的字符索引空间）：
 *
 * 1. **pageText 与 geo 同源**：直接读 `geo.text`（getPageGeometry 返回的未剥离
 *    原始字符流，与 runs[].charStart 共享同一字符索引空间）。**不要**改回
 *    getTextSlices——它会调用 stripPdfUnwantedMarkers 剥离零宽标记字符，
 *    导致 text.length < runs 总字形数，索引整体错位，高亮范围向左偏移。
 *
 * 2. **反向定位**（alignPageEntries）：对每个 MinerU 段落（entry.originalText）
 *    在 pageText 里跑 charAlign::locateTextInPage，得到段落级 [start, end)；
 *    然后在段落切片上跑 splitIntoSentences 重切句，与 LLM 输出的 sentences
 *    按顺序对应（绕开 LLM 对 sentence.original 的改写），把每句的 charStart/charEnd
 *    写到 entry.sentences[i] 上。
 *
 * 3. **hover 命中**：mousemove 时 glyphAt 拿到 PDFium 字符索引，findEntryByCharRange
 *    按 entry/sentence 的字符范围查找，rectsWithinSlice 把范围渲染成像素矩形。
 *
 * 坐标映射：
 * - getPageGeometry 返回设备坐标（PDF 页面坐标）
 * - 鼠标位置通过当前页 DOM 元素的 getBoundingClientRect 转换到设备坐标
 * - 高亮 rects 从设备坐标除以 scale 转回 CSS 坐标
 */

import { useState, useCallback, useRef, useEffect } from 'react';
import type { PdfEngine, PdfPageObject, PdfPageGeometry, Rect } from '@autonomics/pdfium-viewer';
import { glyphAt, rectsWithinSlice } from '@autonomics/pdfium-viewer/plugin-selection/utils';
import type { DocumentManagerCapability } from '@autonomics/pdfium-viewer/plugin-document-manager/react';
import { getParagraphTranslations, type ParagraphTranslationDto } from '../../../services/translationApi';
import type { SentencePair } from '../types';
import {
  getCacheKey,
  setPageEntries,
  alignPageEntries,
  findEntryByCharRange,
  findEntryByBBox,
  evictNonAdjacentPages,
  evictDocument,
  type TranslationCacheEntry,
} from './translationCache';

// ---------- 类型 ----------

export interface WasmHoverTranslationOptions {
  pdfAreaRef: React.RefObject<HTMLDivElement | null>;
  /** WASM PDFium 引擎实例 */
  engine: PdfEngine | null;
  /** 文档 ID（来自 DocumentManagerPlugin） */
  docId: string | null;
  /** DocumentManagerPlugin capability，用于获取 PdfDocumentObject */
  docManagerCap: DocumentManagerCapability | null;
  /** 论文 ID（UUID 字符串） */
  paperId?: string;
  /** 当前页码（1-based） */
  currentPage: number;
  /** 当前缩放级别 */
  zoomLevel: number;
  /**
   * 是否启用悬浮翻译。只有"已预翻译 + 用户在文献行 Tag 上开启悬浮"的论文才传 true，
   * 其他状态传 false（mousemove 直接短路，不显示 tooltip、不高亮）。
   */
  enabled?: boolean;
  /** 文档是否加载完成（PDF 解析就绪后 effect 才会重试，避免首次进入时文本层加载失败后永不重试） */
  isLoaded?: boolean;
  /** 隐藏延迟（ms） */
  hideDelay?: number;
}

export interface WasmTranslationTooltip {
  visible: boolean;
  x: number;
  y: number;
  /**
   * 段落顶边（视口坐标，px）。浮层用它判断"上方空间是否更大"来翻转放置；
   * 缺省时浮层只往下放（兼容旧调用方）。
   */
  paraTop?: number;
  text: string;
  sentences: SentencePair[];
  loading: boolean;
  /** 当前 hover 句子在 sentences 数组里的索引；-1=无对齐、不传=fallback 到首句 */
  hoveredSentenceIndex?: number;
}

// ---------- 工具 ----------

/**
 * 把后端 ParagraphTranslationDto 转成缓存 entry。
 * sentences 字段是 JSON 字符串，parse 一次后存进缓存（hover 命中时不再重复 parse）。
 */
function dtoToEntry(dto: ParagraphTranslationDto): TranslationCacheEntry {
  let sentences: SentencePair[] = [];
  if (dto.sentences) {
    try {
      const parsed = JSON.parse(dto.sentences);
      if (Array.isArray(parsed)) sentences = parsed as SentencePair[];
    } catch {
      // sentences JSON 解析失败 → 当作空数组，hover 时 fallback 到整段译文
    }
  }
  return {
    translation: dto.translatedText,
    sentences,
    // 保留 MinerU 整段原文，供 alignPageEntries 做段落级反向定位
    // —— 整段定位比单句定位稳定（特征强），且绕开 LLM 对 sentence.original 的改写
    originalText: dto.originalText,
    // 段落 bbox（来自 PdfBlock，后端 join 注入）：坐标命中主路径（findEntryByBBox）。
    // 与 PDFium device 坐标同系，hover 时直接几何包含判断，不依赖字符串匹配。
    bboxX0: dto.bboxX0,
    bboxY0: dto.bboxY0,
    bboxX1: dto.bboxX1,
    bboxY1: dto.bboxY1,
  };
}

/** 把 Rect（设备坐标）转成 CSS 坐标（除以 scale） */
function rectToCss(r: Rect, scaleX: number, scaleY: number) {
  return {
    x: r.origin.x / scaleX,
    y: r.origin.y / scaleY,
    w: r.size.width / scaleX,
    h: r.size.height / scaleY,
  };
}

// ---------- Hook ----------

export function useWasmHoverTranslation({
  pdfAreaRef,
  engine,
  docId,
  docManagerCap,
  paperId,
  currentPage,
  zoomLevel,
  enabled = false,
  isLoaded,
  hideDelay = 300,
}: WasmHoverTranslationOptions) {
  const [tooltip, setTooltip] = useState<WasmTranslationTooltip>({
    visible: false, x: 0, y: 0, text: '', sentences: [], loading: false,
  });
  const [highlight, setHighlight] = useState<{
    pageIndex: number;
    rects: Array<{ x: number; y: number; w: number; h: number }>;
  } | null>(null);

  const hoverTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const tooltipHoveredRef = useRef(false);
  const currentTextRef = useRef<string | null>(null);

  // 页面级缓存：geometry（runs + glyphs）+ 全页文本（geo.text 未剥离版本）
  const geoRef = useRef<PdfPageGeometry | null>(null);
  const pageTextRef = useRef<string>('');
  const pageKeyRef = useRef('');

  // 缓存当前页面的尺寸（设备坐标空间），用于坐标转换
  const pageSizeRef = useRef<{ width: number; height: number }>({ width: 0, height: 0 });

  const clearTimers = useCallback(() => {
    if (hoverTimerRef.current) { clearTimeout(hoverTimerRef.current); hoverTimerRef.current = null; }
    if (hideTimerRef.current) { clearTimeout(hideTimerRef.current); hideTimerRef.current = null; }
  }, []);

  const hideTooltip = useCallback(() => {
    clearTimers();
    currentTextRef.current = null;
    setTooltip({ visible: false, x: 0, y: 0, text: '', sentences: [], loading: false });
    setHighlight(null);
  }, [clearTimers]);

  // ---------- 加载页面文本结构（geometry + 全页文本 + 句子区间） ----------

  // pageReadyKey 在 geometry 加载完成后更新，用于触发"拉预翻译"effect。
  // 切页时新值会先变 ''（geometry 还没好），等加载完再变 `${docId}_${currentPage}`。
  const [pageReadyKey, setPageReadyKey] = useState<string>('');

  useEffect(() => {
    // enabled gate: 未开 hover 翻译时完全跳过 getPageGeometry —— 这是个重的
    // worker 任务(整页字符流 + runs 计算), 每次切页都跑会霸占 pdfium worker
    // 单线程队列, 阻塞主渲染(renderPageRect), 表现为滚动卡顿。
    // 用户后续开启 hover 翻译时, enabled 变 true 触发此 effect 重跑补加载。
    if (!enabled || !engine || !docId || !docManagerCap || currentPage < 1) return;
    if (isLoaded === false) return;

    const key = `${docId}_${currentPage}`;
    // 已加载过：只把 pageReadyKey 同步到当前 key（让下游翻译 effect 有机会重跑），
    // 不重复跑 geometry（避免切回已访问页时再次 await getPageGeometry）。
    if (key === pageKeyRef.current) {
      setPageReadyKey(key);
      return;
    }

    let cancelled = false;
    // 先标记为未就绪，让 hover 直接短路，避免读到旧页的 geometry
    setPageReadyKey('');

    (async () => {
      try {
        const docObj = docManagerCap.getDocument(docId);
        if (!docObj || !docObj.pages || docObj.pages.length < currentPage) {
          console.warn(`[useWasmHoverTranslation] getDocument not ready: docObj=${!!docObj} pages=${docObj?.pages?.length} currentPage=${currentPage}`);
          return;
        }

        const pageObj: PdfPageObject = docObj.pages[currentPage - 1];
        pageSizeRef.current = { width: pageObj.size.width, height: pageObj.size.height };

        const geo = await engine.getPageGeometry(docObj, pageObj).toPromise();
        if (cancelled) return;

        // geo.text 是未 strip 的原始字符流（含软连字符 / 零宽空格等"不可见"标记），
        // 与 runs[].charStart 共享同一字符索引空间——hover 高亮链路依赖此对齐。
        // **不要**改回 getTextSlices：它会调用 stripPdfUnwantedMarkers 剥离这些标记，
        // 导致 text.length < runs 总字形数，索引整体错位，高亮范围向左偏移。
        const text = geo.text;

        geoRef.current = geo;
        pageTextRef.current = text;
        pageKeyRef.current = key;
        setPageReadyKey(key);

        logCountRef.current = 0;

        // 切页时清理远端的缓存桶（保留当前页 ±1 页）
        evictNonAdjacentPages(docId, currentPage - 1);
      } catch (err) {
        if (!cancelled) {
          console.warn('[useWasmHoverTranslation] Failed to load page geometry:', err);
        }
      }
    })();

    return () => { cancelled = true; };
  }, [enabled, engine, docId, docManagerCap, currentPage, isLoaded]);

  // ---------- 拉预翻译结果（依赖 pageReadyKey，与 geometry 解耦） ----------

  useEffect(() => {
    if (!enabled || !paperId || !docId || currentPage < 1) return;
    // 等当前页的 geometry 加载完才拉——避免读到旧页数据
    if (pageReadyKey !== `${docId}_${currentPage}`) return;

    let cancelled = false;
    (async () => {
      try {
        const resp = await getParagraphTranslations(paperId, currentPage - 1);
        if (cancelled) return;
        const entries = new Map<string, TranslationCacheEntry>();
        for (const dto of resp.translations) {
          const entry = dtoToEntry(dto);
          // 缓存键用后端算法（getCacheKey）重新算一遍，
          // 跟 DTO 自带的 cacheKey 等价（后端写入时也是同一算法），但保证两端一致。
          entries.set(getCacheKey(dto.originalText), entry);
        }
        setPageEntries(docId, currentPage - 1, entries);
        // pageText 在 pageReadyKey 触发此 effect 前已加载完，
        // 立刻跑反向定位把 charStart/charEnd 写到每个 entry 和 sentence 上。
        // 这一步是 hover 链路 O(1) 查找的前提。
        alignPageEntries(docId, currentPage - 1, pageTextRef.current);
      } catch (err) {
        if (!cancelled) {
          console.warn('[useWasmHoverTranslation] Failed to fetch paragraph translations:', err);
        }
      }
    })();
    return () => { cancelled = true; };
  }, [enabled, paperId, docId, currentPage, pageReadyKey]);

  // ---------- 鼠标事件（原生捕获，绕过插件层事件拦截） ----------

  const logCountRef = useRef(0);

  const handleMouseEnter = useCallback(() => {
    clearTimers();
  }, [clearTimers]);

  const handleMouseLeave = useCallback(() => {
    clearTimers();
    hideTimerRef.current = setTimeout(() => {
      if (!tooltipHoveredRef.current) hideTooltip();
    }, hideDelay);
  }, [hideDelay, hideTooltip, clearTimers]);

  /** 找到当前页的 DOM 元素（通过 data-page-index 属性） */
  const findPageElement = useCallback((): HTMLElement | null => {
    if (!pdfAreaRef.current || currentPage < 1) return null;
    const el = pdfAreaRef.current.querySelector(`[data-page-index="${currentPage - 1}"]`);
    if (!el && logCountRef.current < 5) {
      const children = pdfAreaRef.current.querySelectorAll('[data-page-index]');
      console.warn('[useWasmHoverTranslation] findPageElement: no [data-page-index="' + (currentPage - 1) + '"] found, existing indices:', Array.from(children).map(e => (e as HTMLElement).dataset.pageIndex));
      logCountRef.current++;
    }
    return el as HTMLElement | null;
  }, [pdfAreaRef, currentPage]);

  // 实际处理一次 mousemove 的逻辑（不做节流，由 handleMouseMove 调度）
  const processHover = useCallback((e: React.MouseEvent) => {
    if (!enabled) return; // 未启用悬浮翻译（论文未预翻译或用户关闭了悬浮）
    if (!docId) return; // docId 还没绑定时缓存键无法构造
    if (!pdfAreaRef.current) return;

    const pageEl = findPageElement();
    if (!pageEl) return;

    const geo = geoRef.current;
    if (!geo) return; // geometry 还没加载完，忽略

    const pageRect = pageEl.getBoundingClientRect();
    const pageSize = pageSizeRef.current;

    const relX = e.clientX - pageRect.left;
    const relY = e.clientY - pageRect.top;

    if (relX < 0 || relY < 0 || relX > pageRect.width || relY > pageRect.height) {
      setHighlight(null);
      clearTimers();
      hideTimerRef.current = setTimeout(() => {
        if (!tooltipHoveredRef.current) hideTooltip();
      }, hideDelay);
      return;
    }

    const scaleX = pageSize.width > 0 ? pageSize.width / pageRect.width : 1;
    const scaleY = pageSize.height > 0 ? pageSize.height / pageRect.height : 1;
    const deviceX = relX * scaleX;
    const deviceY = relY * scaleY;

    // glyphAt 仍调用：用于句子级细分高亮（段落已确定后，在该段内找具体句子）。
    // 但它失败不再阻断段落命中——bbox 命中不依赖字符命中（鼠标在行间隙也能命中段落）。
    const charIndex = glyphAt(geo, { x: deviceX, y: deviceY }, 1.5);

    // ===== 主命中路径：坐标几何包含（findEntryByBBox） =====
    // MinerU bbox 与 PDFium device 坐标同系，鼠标落点直接对 entry.bbox 做包含判断。
    // 零字符串匹配 → 不受 LLM 改写 / 跨页拼接 / 标签差异影响。
    let hit = findEntryByBBox(docId, currentPage - 1, deviceX, deviceY);

    // ===== 降级路径：字符范围命中（findEntryByCharRange） =====
    // 仅当 bbox 命中失败（按需翻译无 blockId → bbox 全 0）且 glyphAt 命中了字符时走。
    let sentenceIndex = -1;
    if (!hit && charIndex >= 0) {
      const charHit = findEntryByCharRange(docId, currentPage - 1, charIndex);
      if (charHit) {
        hit = charHit;
        sentenceIndex = charHit.sentenceIndex;
      }
    }

    if (!hit) {
      // 鼠标不在任何预翻译段落内：隐藏并启动延迟收起。
      setHighlight(null);
      clearTimers();
      hideTimerRef.current = setTimeout(() => {
        if (!tooltipHoveredRef.current) hideTooltip();
      }, hideDelay);
      return;
    }
    const { entry } = hit;

    // bbox 命中后，在该段内用 charIndex 找具体句子（段落已定位正确，句子级细分可靠）。
    // 这样 tooltip 能高亮到具体句子而非整段，同时定位不受影响。
    if (sentenceIndex < 0 && charIndex >= 0) {
      for (let i = 0; i < entry.sentences.length; i++) {
        const sent = entry.sentences[i];
        if (
          sent.charStart !== undefined &&
          sent.charEnd !== undefined &&
          charIndex >= sent.charStart &&
          charIndex < sent.charEnd
        ) {
          sentenceIndex = i;
          break;
        }
      }
    }

    // 高亮范围（三级 fallback）：
    // 1) 命中句子的 char range（最精确）
    // 2) 段落的 char range（hover 在两句间空白，或某句定位失败）
    // 3) entry.bbox 矩形本身（charAlign 全失败，但 bbox 命中 → 仍有高亮）
    let hlStart: number | undefined;
    let hlEnd: number | undefined;
    if (sentenceIndex >= 0) {
      const sent = entry.sentences[sentenceIndex];
      hlStart = sent.charStart;
      hlEnd = sent.charEnd;
    }
    if (hlStart === undefined || hlEnd === undefined) {
      hlStart = entry.charStart;
      hlEnd = entry.charEnd;
    }

    let cssRects: ReturnType<typeof rectToCss>[];
    if (hlStart !== undefined && hlEnd !== undefined) {
      const sliceRects = rectsWithinSlice(geo, hlStart, hlEnd - 1, true);
      cssRects = sliceRects.map(r => rectToCss(r, scaleX, scaleY));
    } else {
      // bbox fallback：直接用段落边界框画高亮（与 PDFium glyph 同系，无需转换）。
      cssRects = [rectToCss(
        { origin: { x: entry.bboxX0 ?? 0, y: entry.bboxY0 ?? 0 },
          size: { width: (entry.bboxX1 ?? 0) - (entry.bboxX0 ?? 0), height: (entry.bboxY1 ?? 0) - (entry.bboxY0 ?? 0) } },
        scaleX, scaleY,
      )];
    }
    if (cssRects.length === 0 || (cssRects.length === 1 && cssRects[0].w <= 0)) {
      setHighlight(null);
      return;
    }
    setHighlight({ pageIndex: currentPage - 1, rects: cssRects });

    // 去重：同一 (entry, sentenceIndex) 内移动 mouse 不重渲染 tooltip。
    // originalText 是 DB 里 stable 的段落原文，作为 key 的一部分比 sentence 文本更稳。
    const hoverKey = `${entry.originalText}#${sentenceIndex}`;
    if (currentTextRef.current === hoverKey) return;
    currentTextRef.current = hoverKey;
    clearTimers();

    // tooltip 定位：基于段落 bbox（设备坐标，始终可用）→ 屏幕坐标。
    // 用 bbox 而非 sliceRects 是因为走 bbox fallback 高亮时没有 sliceRects，
    // 且 bbox 是段落整段边界，tooltip 居中更稳定。
    // paraTop 一并传出：浮层按上下剩余空间自行翻转，避免压住下文/溢出屏幕。
    const bx0 = entry.bboxX0 ?? 0;
    const bx1 = entry.bboxX1 ?? 0;
    const by0 = entry.bboxY0 ?? 0;
    const by1 = entry.bboxY1 ?? 0;
    const paraLeft = bx0 / scaleX + pageRect.left;
    const paraRight = bx1 / scaleX + pageRect.left;
    const paraTop = by0 / scaleY + pageRect.top;
    const paraBottom = by1 / scaleY + pageRect.top;
    const tooltipX = (paraLeft + paraRight) / 2;
    const tooltipY = paraBottom + 8;

    if (sentenceIndex >= 0) {
      // 命中具体句子：显示该句译文，sentences 数组完整渲染让用户看上下文，
      // hoveredSentenceIndex 指向命中句（tooltip 内部高亮）
      setTooltip({
        visible: true,
        x: tooltipX,
        y: tooltipY,
        paraTop,
        text: entry.sentences[sentenceIndex].translated,
        sentences: entry.sentences,
        loading: false,
        hoveredSentenceIndex: sentenceIndex,
      });
    } else {
      // 命中段落但未命中具体句子（极少：句子定位失败但段落定位成功）
      // 显示整段译文，sentences 完整渲染让用户看上下文，不高亮任何句
      setTooltip({
        visible: true,
        x: tooltipX,
        y: tooltipY,
        paraTop,
        text: entry.translation,
        sentences: entry.sentences,
        loading: false,
        hoveredSentenceIndex: -1,
      });
    }
  }, [enabled, docId, pdfAreaRef, currentPage, hideDelay, hideTooltip, clearTimers, findPageElement]);

  // rAF 节流：鼠标高频移动时每帧最多跑一次完整命中查询（glyphAt→findEntryByCharRange→rectsWithinSlice）。
  const rafIdRef = useRef<number | null>(null);
  const lastMoveEventRef = useRef<React.MouseEvent | null>(null);

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    if (!enabled) return;
    lastMoveEventRef.current = e;
    if (rafIdRef.current !== null) return; // 已有 rAF 排队，下一帧会用最新事件
    rafIdRef.current = requestAnimationFrame(() => {
      rafIdRef.current = null;
      const evt = lastMoveEventRef.current;
      if (evt) processHover(evt);
    });
  }, [enabled, processHover]);

  const handleTooltipMouseEnter = useCallback(() => {
    tooltipHoveredRef.current = true;
    if (hideTimerRef.current) { clearTimeout(hideTimerRef.current); hideTimerRef.current = null; }
  }, []);

  const handleTooltipMouseLeave = useCallback(() => {
    tooltipHoveredRef.current = false;
    hideTimerRef.current = setTimeout(hideTooltip, 200);
  }, [hideTooltip]);

  // ---------- 原生事件绑定（capture 阶段，绕过插件层 stopPropagation） ----------

  useEffect(() => {
    const el = pdfAreaRef.current;
    if (!el) return;

    const onMove = (e: MouseEvent) => handleMouseMove(e as any);
    const onEnter = () => handleMouseEnter();
    const onLeave = () => handleMouseLeave();

    el.addEventListener('mousemove', onMove, { capture: true, passive: true });
    el.addEventListener('mouseenter', onEnter);
    el.addEventListener('mouseleave', onLeave);

    return () => {
      el.removeEventListener('mousemove', onMove, { capture: true });
      el.removeEventListener('mouseenter', onEnter);
      el.removeEventListener('mouseleave', onLeave);
    };
  }, [handleMouseMove, handleMouseEnter, handleMouseLeave, pdfAreaRef]);

  // 滚动时隐藏
  useEffect(() => {
    const container = pdfAreaRef.current;
    if (!container) return;
    const handleScroll = () => {
      if (tooltip.visible) {
        hideTooltip();
      }
    };
    container.addEventListener('scroll', handleScroll, { passive: true });
    // 同时监听 Viewport（实际滚动容器）的 scroll 事件
    const viewport = container.firstElementChild;
    if (viewport) {
      viewport.addEventListener('scroll', handleScroll, { passive: true });
    }
    return () => {
      container.removeEventListener('scroll', handleScroll);
      viewport?.removeEventListener('scroll', handleScroll);
    };
  }, [pdfAreaRef, tooltip.visible, hideTooltip]);

  // 页码变化时重置
  useEffect(() => {
    currentTextRef.current = null;
    hideTooltip();
  }, [currentPage, docId]);

  // 切换文档时清掉旧文档的预翻译缓存桶，避免内存常驻与跨论文污染
  const prevDocIdRef = useRef<string | null>(null);
  useEffect(() => {
    const prev = prevDocIdRef.current;
    if (prev && prev !== docId) evictDocument(prev);
    prevDocIdRef.current = docId;
  }, [docId]);

  // 卸载清理：取消计时器 + 取消 pending rAF + 清当前文档的缓存桶
  useEffect(() => {
    return () => {
      clearTimers();
      if (rafIdRef.current !== null) {
        cancelAnimationFrame(rafIdRef.current);
        rafIdRef.current = null;
      }
      if (prevDocIdRef.current) evictDocument(prevDocIdRef.current);
    };
  }, [clearTimers]);

  return {
    tooltip,
    highlight,
    handleMouseMove,
    handleMouseEnter,
    handleMouseLeave,
    handleTooltipMouseEnter,
    handleTooltipMouseLeave,
  };
}
