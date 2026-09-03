/**
 * CSL 引用 API 客户端
 *
 * 对接后端 endpoint（`/api/citation/styles`、`/api/items/:id/csl-json`）：
 *   - `getStyles()`        → GET /api/citation/styles
 *   - `getStyle(id)`       → GET /api/citation/styles/{id}
 *   - `getCslJson(itemId)` → GET /api/items/{id}/csl-json
 *
 * 类型对齐后端 serde `#[serde(rename_all="camelCase")]` 输出。
 *
 * 注：默认样式偏好（`setDefaultStyle` / `getDefaultStyle`，`/api/settings/citation-style`）
 * 已随 CitationSettings 删除于 2026-09-02 移除——导出弹窗内自选样式，无全局默认偏好。
 */
import request from './client';

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

/** `GET /api/items/:id/csl-json` 响应包裹层（后端 `CslJsonResponse`） */
export interface CslJsonResponse {
    /** CSL 1.0.2 schema item，直接喂给 citeproc-js */
    item: Record<string, unknown>;
}

/**
 * 列出所有 CSL 样式（不含 XML 正文）
 *
 * 列表场景：Copy Citation / 批量导出 / Settings 的样式选择器。
 * 单条 XML 走 `getStyle(id)` 按需拉取（避免一次 list 膨胀 ~400KB）。
 *
 * @param signal 可选 AbortSignal
 */
export async function getStyles(signal?: AbortSignal): Promise<CslStyleSummary[]> {
    return request<CslStyleSummary[]>('/citation/styles', { signal });
}

/**
 * 取单个样式完整内容（含 cslXml）
 *
 * 仅在用户切样式 / 服务端渲染（P5）时拉取。前端 `@autonomics/citation-engine`
 * 默认打包 5 个 bundled 样式，不需要走这个接口；P3+ 引入按需下载未 bundled
 * 样式时才用。
 */
export async function getStyle(id: string, signal?: AbortSignal): Promise<CslStyleDetail> {
    return request<CslStyleDetail>(`/citation/styles/${encodeURIComponent(id)}`, { signal });
}

/**
 * 取单条文献的 CSL JSON（核心 endpoint）
 *
 * 前端 `@autonomics/citation-engine` 用返回的 `item` 喂给 citeproc-js 渲染
 * 引用 cluster 和参考文献表。后端从 EAV 拼出 CSL 1.0.2 兼容结构。
 */
export async function getCslJson(itemId: string, signal?: AbortSignal): Promise<CslJsonResponse> {
    return request<CslJsonResponse>(`/items/${encodeURIComponent(itemId)}/csl-json`, { signal });
}

/** 上传自定义 CSL 样式的请求体 */
export interface CreateStylePayload {
    /** CSL XML 原文（必填） */
    cslXml: string;
    /** 可选 id（不传后端从 `<info><id>` 提取） */
    id?: string;
    /** 可选展示标题（不传后端从 `<info><title>` 提取） */
    title?: string;
    /** 可选 locale（不传后端从 `<style default-locale>` 提取，再缺省 en-US） */
    locale?: string;
    /** 可选分类标签 */
    categories?: string[];
}

/**
 * 上传自定义 CSL 样式
 *
 * 后端 `validate_csl_xml` 会做基本格式校验 + 元数据提取，挡掉非 CSL XML 和
 * 过大输入（500KB 上限）。冲突（id 已存在）返回 400。
 *
 * @returns 创建的样式完整资源（含 cslXml）
 */
export async function uploadStyle(
    payload: CreateStylePayload,
): Promise<CslStyleDetail> {
    return request<CslStyleDetail>('/citation/styles', {
        method: 'POST',
        body: payload,
    });
}

/**
 * 删除自定义 CSL 样式
 *
 * 内置种子样式（`isDefault: true` 的 5 个）后端拒绝删除，返回 400。
 * 用户上传的样式删除时，关联的 CitationRender 由外键 CASCADE 自动清理。
 */
export async function deleteStyle(id: string): Promise<void> {
    await request<void>(`/citation/styles/${encodeURIComponent(id)}`, {
        method: 'DELETE',
    });
}
