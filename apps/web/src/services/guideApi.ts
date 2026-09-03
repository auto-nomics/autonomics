/**
 * V2 导览相关 API 客户端
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * jayread 的段落导览（guide）依赖 MinerU 结构化解析（章节树 + bbox 坐标）与
 * AI 生成管线。autonomics 没有 PDF 解析管线，自然也没有导览数据源，全部函数桩化。
 *
 * 降级策略：
 * - 读侧（getGuideTree / getBlocks / searchGuideNodes / getNodeDetail）返回
 *   **空结果**，导览面板渲染为空而不是报错 —— 与 `useGuideBlockData` 里
 *   `paper.guide_version !== 'v2'` 时的行为一致（autonomics 的 Paper 不带
 *   guide_version，本就不会触发加载，这里是双保险）。
 * - 写侧 / 生成侧（generateGuideV2 / regenerateNode / updateNodeImportance /
 *   cancelGuide）显式 reject，调用方 toast 失败提示。
 *
 * 导出签名与 jayread 版本一致；UI 入口隐藏属 Phase 5 清扫。
 */
import type {
  GenerateGuideV2Request,
  GenerateGuideV2Response,
  CancelGuideResponse,
  GetGuideTreeResponse,
  GetBlocksResponse,
  GetNodeDetailResponse,
  RegenerateNodeResponse,
  UpdateNodeImportanceResponse,
  SearchGuideNodesRequest,
  GuideNodeSearchResult,
} from '@/types';

/** 空导览树（GuideTree 形状） */
function emptyGuideTree(): GetGuideTreeResponse {
  return { nodes: [], total: 0, generated_at: new Date().toISOString() };
}

/**
 * 获取论文的层次化导览树数据
 *
 * ⚠️ 桩：autonomics 无导览数据源，恒返回空树。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} paperId - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetGuideTreeResponse>} 空导览树
 */
export async function getGuideTree(_paperId: string, _signal?: AbortSignal): Promise<GetGuideTreeResponse> {
  return emptyGuideTree();
}

/**
 * 触发 AI 生成导览（Phase A + Phase B）
 *
 * ⚠️ 桩：autonomics 无导览生成管线。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function generateGuideV2(_paperId: string, _options: GenerateGuideV2Request = {}): Promise<GenerateGuideV2Response> {
  void _options;
  throw new Error('导览生成暂不可用（autonomics 无段落解析与 AI 导览管线）');
}

/**
 * 取消导览生成
 *
 * ⚠️ 桩：无生成任务可取消。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function cancelGuide(_paperId: string): Promise<CancelGuideResponse> {
  return { message: '没有正在进行的导览任务', task_id: '' };
}

/**
 * 获取页面块数据
 *
 * ⚠️ 桩：autonomics 无 PDF 块级解析结果，恒返回空集合。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} paperId - 论文 ID
 * @param {number|null} [pageIdx=null] - 页码索引（0-based）
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetBlocksResponse>} 空块数据
 */
export async function getBlocks(_paperId: string, _pageIdx: number | null = null, _signal?: AbortSignal): Promise<GetBlocksResponse> {
  return { blocks: [], page_idx: 0, total_pages: 0 };
}

/**
 * 获取导览生成的 SSE 实时推送端点 URL
 *
 * ⚠️ 桩：返回一个不存在的端点路径。没有任何生成任务会启动，所以这个 URL
 * 不应被使用；保留 `/api/v1/bib` 前缀以与统一的 API 根保持一致。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export function getGuideStreamUrl(_paperId: string): string {
  return `/api/v1/bib/no-guide-stream`;
}

/**
 * 获取单个导览节点详情
 *
 * ⚠️ 桩：无导览数据。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function getNodeDetail(_paperId: string, _nodeId: string): Promise<GetNodeDetailResponse> {
  throw new Error('导览节点详情暂不可用（autonomics 无导览数据）');
}

/**
 * 重新生成单个导览节点
 *
 * ⚠️ 桩：autonomics 无导览生成管线。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function regenerateNode(_paperId: string, _nodeId: string): Promise<RegenerateNodeResponse> {
  throw new Error('导览节点重新生成暂不可用（autonomics 无 AI 导览管线）');
}

/**
 * 更新导览节点重要度
 *
 * ⚠️ 桩：无导览数据可更新。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function updateNodeImportance(_paperId: string, _nodeId: string, _importance: number): Promise<UpdateNodeImportanceResponse> {
  throw new Error('导览节点重要度暂不可更新（autonomics 无导览数据）');
}

/**
 * 搜索导览节点
 *
 * ⚠️ 桩：无导览数据，恒返回空匹配。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function searchGuideNodes(
  _paperId: string,
  _query: string,
  _options: Omit<SearchGuideNodesRequest, 'query'> = {},
): Promise<GuideNodeSearchResult> {
  return { matches: [], total: 0 };
}
