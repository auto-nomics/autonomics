/**
 * 高亮标注 API 客户端（autonomics annotations）
 *
 * jayread 的高亮（Highlight）映射到 autonomics 的 annotation：
 *   kind = 'highlight'
 *   content = 选中的文字
 *   page = 1 起页码
 *   data = { rects: [{x,y,w,h}...], color: '#...', note?: '...' }
 *
 * 反向（annotation → Highlight）时 `note` 不落回 Highlight —— jayread 的
 * Highlight 模型没有 note 字段，颜色和矩形才是阅读器渲染所需。
 */
import request from './client';
import { fromSafeId, encodeIdSegment } from './mapping';
import type {
  Highlight,
  FetchHighlightsResponse,
  CreateHighlightRequest,
  UpdateHighlightRequest,
  HighlightRect,
} from '@/types';

// ============================================================
// 后端契约类型
// ============================================================

export interface BibAnnotation {
  id: string;
  kind: 'note' | 'highlight' | 'comment';
  content: string;
  /** 1 起页码；null = 未锚定到页 */
  page: number | null;
  /** 自由 JSON；高亮存 {rects, color, note} */
  data: Record<string, unknown> | null;
  created_at: string | null;
}

// ============================================================
// 映射
// ============================================================

/** annotation → Highlight。`paperSafeId` 是调用方持有的论文 safeId。 */
function annotationToHighlight(a: BibAnnotation, paperSafeId: string): Highlight {
  const data = (a.data ?? {}) as { rects?: HighlightRect[]; color?: string };
  return {
    id: a.id,
    // jayread 的 Highlight.paper_id 是组件持有的论文 id（safeId）。后端的
    // Annotation 没有 article_id 字段（标注靠挂载路径 /articles/{enc}/annotations
    // 关联到文章），所以论文归属只能由调用方带进来。
    paper_id: paperSafeId,
    page: typeof a.page === 'number' ? a.page : 1,
    text: a.content ?? '',
    color: data.color || '#ffeb3b',
    rects: Array.isArray(data.rects) ? data.rects : [],
    created_at: a.created_at ?? new Date().toISOString(),
  };
}

// ============================================================
// 导出 API（签名与 jayread 版本一致）
// ============================================================

/**
 * 获取指定论文的高亮标注列表
 *
 * 支持按页码过滤（只取该页），减少数据传输量。
 *
 * 为什么 page 参数是可选的？
 * - 有些场景需要获取全部高亮（如高亮管理面板）
 * - 有些场景只需要当前页的高亮（如 PDF 阅读器渲染）
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {number|null} [page=null] - 可选的页码过滤
 * @returns {Promise<FetchHighlightsResponse>} 高亮列表数据
 */
export async function fetchHighlights(paperId: string, page: number | null = null): Promise<FetchHighlightsResponse> {
  const enc = encodeIdSegment(fromSafeId(paperId));
  const path = page !== null
    ? `/articles/${enc}/annotations?page=${page}`
    : `/articles/${enc}/annotations`;

  const data = await request<{ annotations?: BibAnnotation[] }>(path);
  const annotations = Array.isArray(data?.annotations) ? data.annotations : [];
  const highlights = annotations.map((a) => annotationToHighlight(a, paperId));

  return { highlights, total: highlights.length };
}

/**
 * 创建新的高亮标注
 *
 * 高亮数据结构说明：
 * - page: PDF 页码（从 1 开始）
 * - text: 选中的文本内容 → annotation.content
 * - rects: 选区的矩形坐标数组（跨行选区会有多个矩形）→ annotation.data.rects
 * - color: 高亮颜色，默认黄色 → annotation.data.color
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {CreateHighlightRequest} data - 高亮数据对象
 * @returns {Promise<Highlight>} 创建成功的高亮对象
 */
export async function createHighlight(paperId: string, data: CreateHighlightRequest): Promise<Highlight> {
  const enc = encodeIdSegment(fromSafeId(paperId));
  // 响应是 {"annotation": {...}}（201），需要解包
  const res = await request<{ annotation: BibAnnotation }>(`/articles/${enc}/annotations`, {
    method: 'POST',
    body: {
      kind: 'highlight',
      content: data.text,
      page: data.page,
      data: {
        rects: data.rects,
        color: data.color || '#ffeb3b',
      },
    },
  });
  return annotationToHighlight(res.annotation, paperId);
}

/**
 * 更新指定高亮的颜色
 *
 * autonomics 的 `PUT /annotations/{id}` 收 `{content?, page?, data?}`，是**三态**
 * 语义：省略字段 = 不动；`data: null` = 清空载荷。而 `data` 一旦给出就是整体
 * 覆盖，所以先取回现值、把新颜色合并进 rects 所在的那个对象再写回，避免把
 * 矩形坐标冲掉。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {string} highlightId - 高亮 ID
 * @param {UpdateHighlightRequest} data - 需要更新的字段
 * @returns {Promise<Highlight>} 更新后的高亮对象
 */
export async function updateHighlight(paperId: string, highlightId: string, data: UpdateHighlightRequest): Promise<Highlight> {
  const enc = encodeIdSegment(fromSafeId(paperId));

  // 先取现值（PUT 的 data 是覆盖语义，不能只发增量）
  const list = await request<{ annotations?: BibAnnotation[] }>(`/articles/${enc}/annotations`);
  const existing = (list?.annotations ?? []).find((a) => a.id === highlightId);
  const existingData = (existing?.data ?? {}) as Record<string, unknown>;

  const mergedData = {
    ...existingData,
    ...(data.color !== undefined ? { color: data.color } : {}),
  };

  // 响应是 {"annotation": {...}}；后端对缺 id 返回 404
  const res = await request<{ annotation: BibAnnotation }>(`/annotations/${encodeURIComponent(highlightId)}`, {
    method: 'PUT',
    body: { data: mergedData },
  });

  // 兜底：万一后端没把 data 回显全，用本地合并结果补上，调用方拿到的形状不变
  const annotation = res?.annotation ?? existing;
  return annotationToHighlight(
    {
      ...(annotation as BibAnnotation),
      id: highlightId,
      data: { ...((annotation?.data ?? {}) as Record<string, unknown>), ...mergedData },
    },
    paperId,
  );
}

/**
 * 删除指定的高亮标注
 *
 * 注意：删除操作不可逆。后端返回 200 + `{"deleted":true,"id"}`（不是 204），
 * 前端仍需要从本地状态移除。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {string} highlightId - 高亮 ID
 * @returns {Promise<void>} 成功时无返回内容
 */
export async function deleteHighlight(paperId: string, highlightId: string): Promise<void> {
  void paperId; // 删除端点挂在 /annotations/{id} 下，不需要论文 id
  await request<{ deleted?: boolean }>(`/annotations/${encodeURIComponent(highlightId)}`, {
    method: 'DELETE',
  });
}
