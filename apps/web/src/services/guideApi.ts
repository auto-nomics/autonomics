/**
 * JayRead V2 导览相关 API 客户端
 *
 * 封装所有 V2 导览相关的后端接口调用，包括：
 * - 获取导览树数据
 * - 触发 AI 生成导览（Phase A + Phase B）
 * - 取消导览生成
 * - 获取页面块数据
 * - SSE 端点 URL 获取
 *
 * 为什么需要这个封装层？
 * 1. 统一管理所有导览相关的 API 端点，便于后续维护
 * 2. 隐藏具体的请求格式细节，调用方只需关注业务逻辑
 * 3. 便于添加统一的错误处理和日志记录
 * 4. 与 papersApi.js 保持一致的设计模式
 *
 * V1 vs V2 API 区别：
 * - V1: GET /papers/{id}/paragraph-guide（获取扁平化段落列表）
 * - V2: GET /papers/{id}/guide/tree（获取层次化导览树）
 * - V2: POST /papers/{id}/guide/generate（触发 AI 生成，返回 task_id）
 * - V2: GET /papers/{id}/guide/stream（SSE 端点，实时推送进度）
 */

import request from './client'; // 导入通用 API 客户端封装函数
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

/**
 * 获取论文的层次化导览树数据
 *
 * 用于在导览面板中展示论文的层次化结构：
 * - 根节点：论文标题
 * - 一级节点：主要章节（Introduction、Methodology 等）
 * - 二级节点：小节
 * - 三级节点：段落级导览（含摘要、重要度、坐标）
 *
 * 数据结构说明：
 * - 每个节点包含：id, title, summary, parentId, children, pageIdx, bboxes, importance
 * - 坐标数据（bboxes）使用 MinerU 归一化坐标（0-1000）
 * - 支持懒加载：initial_nodes + detail_url
 *
 * @param {string} paperId - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<GetGuideTreeResponse>} 导览树数据对象
 */
export async function getGuideTree(paperId: string, signal?: AbortSignal): Promise<GetGuideTreeResponse> {
  // 导出获取导览树函数
  return request<GetGuideTreeResponse>(`/papers/${paperId}/guide/tree`, { signal }); // GET 请求获取导览树数据
}

/**
 * 触发 AI 生成层次化导览
 *
 * 调用后端 API 触发 AI 导览生成任务，包括：
 * - Phase A: 基于全文生成初步导览（大上下文模型）
 * - Phase B: 对重点章节进行精细化导览（小上下文模型）
 *
 * 生成流程：
 * 1. 后端创建异步任务，返回 task_id
 * 2. 前端通过 SSE 端点监听生成进度
 * 3. 生成完成后，前端调用 getGuideTree 获取完整导览树
 *
 * 为什么需要 SSE 而不是轮询？
 * - 导览生成可能需要 30s-5min（取决于论文长度）
 * - SSE 是服务器主动推送，延迟更低
 * - 自动保持连接，无需客户端频繁发起请求
 * - 浏览器原生支持，使用 EventSource API 即可接收
 *
 * @param {string} paperId - 论文 ID
 * @param {GenerateGuideV2Request} [options={}] - 可选配置对象
 * @returns {Promise<GenerateGuideV2Response>} 任务提交确认对象
 */
export async function generateGuideV2(paperId: string, options: GenerateGuideV2Request = {}): Promise<GenerateGuideV2Response> {
  // 导出生成 V2 导览函数
  const { strategy = 'adaptive' } = options; // 解构配置选项，设置默认值

  return request<GenerateGuideV2Response>(`/papers/${paperId}/guide/generate`, {
    // POST 请求触发生成
    method: 'POST', // HTTP 方法为 POST
    body: {
      // POST 请求体
      strategy, // 生成策略
    },
  });
}

/**
 * 取消正在进行的导览生成任务
 *
 * 用于用户主动取消生成中的导览任务。
 * 取消后，已生成的部分数据可能会被保留（取决于后端实现）。
 *
 * @param {string} paperId - 论文 ID
 * @returns {Promise<CancelGuideResponse>} 取消确认对象
 */
export async function cancelGuide(paperId: string): Promise<CancelGuideResponse> {
  // 导出取消导览函数
  return request<CancelGuideResponse>(`/papers/${paperId}/guide/cancel`, {
    // POST 请求取消生成
    method: 'POST', // HTTP 方法为 POST（虽然是取消操作，但使用 POST 语义更清晰）
  });
}

/**
 * 获取论文的页面块数据
 *
 * 获取指定页面的 MinerU 解析块数据，包括：
 * - 块类型（text/title/table/image/equation/list）
 * - 块文本内容
 * - 归一化坐标（bbox，0-1000）
 * - 页码索引（page_idx）
 *
 * 使用场景：
 * - 前端需要渲染页面的详细结构时
 * - 调试和验证 MinerU 解析结果时
 * - 实现页面级导航时
 *
 * 为什么需要单独的端点而不是包含在导览树中？
 * - 导览树只包含摘要级别的信息，不包含完整块数据
 * - 按需加载，避免一次性传输过多数据
 * - 便于实现虚拟滚动和懒加载
 *
 * @param {string} paperId - 论文 ID
 * @param {number|null} [pageIdx=null] - 页码索引（可选），不指定时返回所有页面的块数据
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<GetBlocksResponse>} 页面块数据对象
 */
export async function getBlocks(paperId: string, pageIdx: number | null = null, signal?: AbortSignal): Promise<GetBlocksResponse> {
  // 导出获取块数据函数
  const url = pageIdx !== null
    ? `/papers/${paperId}/blocks?page_idx=${pageIdx}` // 指定页码时添加查询参数
    : `/papers/${paperId}/blocks`; // 不指定页码时返回所有页面

  return request<GetBlocksResponse>(url, { signal }); // GET 请求获取块数据
}

/**
 * 获取 SSE（Server-Sent Events）实时推送端点的 URL
 *
 * 为什么使用 SSE 而不是轮询？
 * - SSE 是服务器主动推送，延迟更低
 * - 自动保持连接，无需客户端频繁发起请求
 * - 浏览器原生支持，使用 EventSource API 即可接收
 * - 单向推送（服务器→客户端），适合导览生成进度推送场景
 *
 * SSE 事件类型：
 * - guide_ready: 完整导览树生成完成，返回节点列表
 * - section_refined: 单个章节精细化导览完成
 * - progress: 生成进度更新（percent, phase, eta_ms）
 * - done: 导览生成完成
 * - error: 生成失败
 * - timeout: 生成超时
 *
 * 使用示例：
 * ```javascript
 * const eventSource = new EventSource(getGuideStreamUrl(paperId));
 * eventSource.addEventListener('guide_ready', (event) => {
 *   const data = JSON.parse(event.data);
 *   console.log('导览生成完成:', data.nodes);
 * });
 * eventSource.addEventListener('progress', (event) => {
 *   const data = JSON.parse(event.data);
 *   console.log('生成进度:', data.percent);
 * });
 * ```
 *
 * @param {string} paperId - 论文 ID
 * @returns {string} SSE 端点的完整 URL，用于 new EventSource()
 */
export function getGuideStreamUrl(paperId: string): string {
  // 导出获取 SSE URL 函数（同步函数）
  return `/api/papers/${paperId}/guide/stream`; // 返回 SSE 实时推送端点的 URL
}

/**
 * 获取单个节点的详细导览内容
 *
 * 用于懒加载场景：
 * - 导览树初始只返回节点框架（id, title, pageIdx）
 * - 用户点击节点时，调用此接口获取详细内容（summary, bboxes）
 * - 减少初始加载时间，提升首屏渲染速度
 *
 * @param {string} paperId - 论文 ID
 * @param {string} nodeId - 节点唯一标识
 * @returns {Promise<GetNodeDetailResponse>} 节点详细数据对象
 */
export async function getNodeDetail(paperId: string, nodeId: string): Promise<GetNodeDetailResponse> {
  // 导出获取节点详情函数
  return request<GetNodeDetailResponse>(`/papers/${paperId}/guide/node/${nodeId}`); // GET 请求获取节点详情
}

/**
 * 重新生成指定节点的导览
 *
 * 用于用户对某个节点的导览不满意时，单独重新生成该节点。
 * 比重新生成整个导览树更快，资源消耗更少。
 *
 * @param {string} paperId - 论文 ID
 * @param {string} nodeId - 节点唯一标识
 * @returns {Promise<RegenerateNodeResponse>} 任务提交确认对象
 */
export async function regenerateNode(paperId: string, nodeId: string): Promise<RegenerateNodeResponse> {
  // 导出重新生成节点函数
  return request<RegenerateNodeResponse>(`/papers/${paperId}/guide/node/${nodeId}/regenerate`, {
    // POST 请求重新生成节点
    method: 'POST', // HTTP 方法为 POST
  });
}

/**
 * 更新节点的重要程度标注
 *
 * 用于用户手动调整节点的重要程度。
 * 调整后的重要程度会影响节点的显示样式（颜色、星星数量）。
 *
 * @param {string} paperId - 论文 ID
 * @param {string} nodeId - 节点唯一标识
 * @param {number} importance - 新的重要程度（1-5）
 * @returns {Promise<UpdateNodeImportanceResponse>} 更新确认对象
 */
export async function updateNodeImportance(paperId: string, nodeId: string, importance: number): Promise<UpdateNodeImportanceResponse> {
  // 导出更新节点重要度函数
  return request<UpdateNodeImportanceResponse>(`/papers/${paperId}/guide/node/${nodeId}/importance`, {
    // POST 请求更新重要度
    method: 'POST', // HTTP 方法为 POST
    body: {
      importance, // 新的重要程度值
    },
  });
}

/**
 * 搜索导览节点
 *
 * 根据关键词在导览树中搜索匹配的节点。
 * 支持模糊匹配和语义搜索（取决于后端实现）。
 *
 * @param {string} paperId - 论文 ID
 * @param {string} query - 搜索关键词
 * @param {SearchGuideNodesRequest} [options={}] - 可选配置对象
 * @returns {Promise<GuideNodeSearchResult>} 搜索结果对象
 */
export async function searchGuideNodes(paperId: string, query: string, options: Omit<SearchGuideNodesRequest, 'query'> = {}): Promise<GuideNodeSearchResult> {
  // 导出搜索导览节点函数
  const { limit = 20 } = options; // 解构配置选项，设置默认值

  return request<GuideNodeSearchResult>(`/papers/${paperId}/guide/search`, {
    // POST 请求搜索节点（关键词较长时使用 POST 避免 URL 过长）
    method: 'POST', // HTTP 方法为 POST
    body: {
      query, // 搜索关键词
      limit, // 结果数量限制
    },
  });
}
