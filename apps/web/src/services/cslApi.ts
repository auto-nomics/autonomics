/**
 * CSL 引用 API 客户端（autonomics）
 *
 * - `getStyles()`        → 本地 `@autonomics/citation-engine` 内置注册表（**无网络**）。
 *   autonomics 后端不提供样式存储，5 个 bundled 样式随包发布，id/locale/cslXml
 *   本地全有 —— `getStyle()` 在引用渲染路径上从来不需要被调用。
 * - `getCslJson(itemId)` → GET /api/v1/bib/articles/{enc}/csl-json
 *
 * 类型对齐 jayread 前端的既有消费方（useCopyCitation、CitationBatchExportModal），
 * 它们读 `isDefault` / `id` / `title` / `item`。
 *
 * 注：默认样式偏好（setDefaultStyle / getDefaultStyle）已随 CitationSettings
 * 于 2026-09-02 移除——导出弹窗内自选样式，无全局默认偏好。
 */
import request from './client';
import { BUNDLED_STYLES, DEFAULT_STYLE_ID } from '@autonomics/citation-engine/styles';
import { fromSafeId, encodeIdSegment } from './mapping';

/** CSL 样式列表项（不含 cslXml 大字段） */
export interface CslStyleSummary {
    id: string;
    title: string;
    locale: string;
    /** JSON 数组，例如 `["numeric", "engineering"]` */
    categories: unknown;
    isDefault: boolean;
}

/** 单个 CSL 样式完整内容（含 XML 正文） */
export interface CslStyleDetail extends CslStyleSummary {
    cslXml: string;
    createdAt: string;
    updatedAt: string;
}

/** `GET /articles/{enc}/csl-json` 响应（jayread `CslJsonResponse` 形状） */
export interface CslJsonResponse {
    /** CSL 1.0.2 schema item，直接喂给 citeproc-js */
    item: Record<string, unknown>;
}

/**
 * 列出所有 CSL 样式（不含 XML 正文）
 *
 * 数据源是本地 bundled 注册表，同步可得，这里包一层 async 保持签名不变。
 * `isDefault` 由 `DEFAULT_STYLE_ID` 推导 —— 后端没有「默认样式」概念，
 * 引用引擎的 `getEngineForStyle()` 兜底也用同一个常量。
 *
 * @param signal 可选 AbortSignal（兼容签名，本地数据不需要取消）
 */
export async function getStyles(_signal?: AbortSignal): Promise<CslStyleSummary[]> {
    void _signal;
    return BUNDLED_STYLES.map((s) => ({
        id: s.id,
        title: s.title,
        locale: s.locale,
        categories: s.categories,
        isDefault: s.id === DEFAULT_STYLE_ID,
    }));
}

/**
 * 取单个样式完整内容（含 cslXml）
 *
 * 从本地注册表读取。仅 bundled 的 5 个样式可查，未知的 id 抛错
 * （与 `getBundledStyle` 的行为一致）。
 */
export async function getStyle(id: string, _signal?: AbortSignal): Promise<CslStyleDetail> {
    void _signal;
    const style = BUNDLED_STYLES.find((s) => s.id === id);
    if (!style) {
        throw new Error(`CSL 样式 "${id}" 不在内置注册表中（autonomics 无自定义样式存储）`);
    }
    return {
        id: style.id,
        title: style.title,
        locale: style.locale,
        categories: style.categories,
        isDefault: style.id === DEFAULT_STYLE_ID,
        cslXml: style.cslXml,
        createdAt: '',
        updatedAt: '',
    };
}

/**
 * 取单条文献的 CSL JSON（核心 endpoint）
 *
 * 前端 `@autonomics/citation-engine` 用返回的 `item` 喂给 citeproc-js 渲染
 * 引用 cluster 和参考文献表。
 *
 * 后端信封是 `{"csl_json": {...}}`，jayread 消费方期待 `{item: {...}}`，这里换名。
 */
export async function getCslJson(itemId: string, signal?: AbortSignal): Promise<CslJsonResponse> {
    const data = await request<{ csl_json?: Record<string, unknown> }>(
        `/articles/${encodeIdSegment(fromSafeId(itemId))}/csl-json`,
        { signal },
    );
    const item = data?.csl_json;
    if (!item || typeof item !== 'object') {
        throw new Error('该文献缺少可用的 CSL 元数据');
    }
    return { item };
}

/** 上传自定义 CSL 样式的请求体 */
export interface CreateStylePayload {
    /** CSL XML 原文（必填） */
    cslXml: string;
    /** 可选 id */
    id?: string;
    /** 可选展示标题 */
    title?: string;
    /** 可选 locale */
    locale?: string;
    /** 可选分类标签 */
    categories?: string[];
}

/**
 * 上传自定义 CSL 样式
 *
 * ⚠️ 桩：autonomics 后端无样式存储，样式集合固定为 bundled 的 5 个。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function uploadStyle(_payload: CreateStylePayload): Promise<CslStyleDetail> {
    void _payload;
    throw new Error('自定义 CSL 样式暂未开放（autonomics 无样式存储）');
}

/**
 * 删除自定义 CSL 样式
 *
 * ⚠️ 桩：bundled 样式不可删除（与 jayread 后端对 isDefault 样式返回 400 的
 * 行为一致，只是这里没有可删除的对象）。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function deleteStyle(_id: string): Promise<void> {
    void _id;
    throw new Error('CSL 样式不可删除（autonomics 仅内置只读样式集）');
}
