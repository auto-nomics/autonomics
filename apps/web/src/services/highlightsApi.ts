/**
 * JayRead 高亮标注 API 客户端
 *
 * 封装所有 PDF 高亮标注相关的后端接口调用，包括：
 * - 获取论文的高亮列表（支持按页码过滤）
 * - 创建新的高亮标注
 * - 更新高亮的颜色和备注
 * - 删除指定的高亮标注
 *
 * 为什么需要这个封装层？
 * 1. 统一管理所有高亮相关的 API 端点，便于后续维护
 * 2. 隐藏具体的请求格式细节，调用方只需关注业务逻辑
 * 3. 便于添加统一的错误处理和日志记录
 * 4. 与其他 API 模块保持一致的代码风格
 */
import request from './client'; // 导入通用 API 客户端封装函数
import type {
  Highlight,
  FetchHighlightsResponse,
  CreateHighlightRequest,
  UpdateHighlightRequest,
} from '@/types';

/**
 * 获取指定论文的高亮标注列表
 *
 * 支持按页码过滤，只获取特定页面的高亮，减少数据传输量。
 *
 * 为什么 page 参数是可选的？
 * - 有些场景需要获取全部高亮（如高亮管理面板）
 * - 有些场景只需要当前页的高亮（如 PDF 阅读器渲染）
 * - 通过可选参数支持两种使用模式，避免创建两个接口
 *
 * @param {string} paperId - 论文 ID
 * @param {number|null} [page=null] - 可选的页码过滤，只返回该页的高亮
 * @returns {Promise<FetchHighlightsResponse>} 高亮列表数据
 */
export async function fetchHighlights(paperId: string, page: number | null = null): Promise<FetchHighlightsResponse> { // 导出获取高亮列表函数，page 默认为 null（获取全部）
  // 构建请求路径，如果指定了页码则添加查询参数
  // 例如：/papers/123/highlights?page=2 只获取第 2 页的高亮
  const path = page !== null
    ? `/papers/${paperId}/highlights?page=${page}` // 带页码过滤的路径
    : `/papers/${paperId}/highlights`;             // 不带过滤，获取所有高亮

  // 发送 GET 请求获取高亮列表
  return request<FetchHighlightsResponse>(path); // GET 请求（request 默认方法为 GET）
}

/**
 * 创建新的高亮标注
 *
 * 用途：用户在 PDF 上选择文本后，将选区保存为高亮标注。
 *
 * 高亮数据结构说明：
 * - page: PDF 页码（从 1 开始）
 * - text: 选中的文本内容，用于显示和管理
 * - rects: 选区的矩形坐标数组，用于渲染高亮覆盖层
 *   - 为什么是数组？一个选区可能包含多个不连续的矩形
 *   - 例如：选中的文本跨行时，每行是一个独立的矩形
 * - color: 高亮颜色，默认黄色
 *
 * @param {string} paperId - 论文 ID
 * @param {CreateHighlightRequest} data - 高亮数据对象
 * @returns {Promise<Highlight>} 创建成功的高亮对象
 */
export async function createHighlight(paperId: string, data: CreateHighlightRequest): Promise<Highlight> { // 导出创建高亮函数
  return request<Highlight>(`/papers/${paperId}/highlights`, { // POST 请求创建高亮
    method: 'POST', // HTTP 方法为 POST（创建资源）
    body: data,     // 请求体：高亮数据对象
  });
}

/**
 * 更新指定高亮的颜色或备注
 *
 * 用途：
 * - 用户修改高亮颜色（如将黄色改为绿色表示"已理解"）
 * - 部分更新：只传需要修改的字段
 *
 * 为什么使用 PUT 而不是 PATCH？
 * - 虽然 RESTful 规范推荐部分更新用 PATCH，
 * - 但 FastAPI 的 PUT 更新实现同样支持部分字段更新
 * - 与后端 API 保持一致的实现方式
 *
 * @param {string} paperId - 论文 ID
 * @param {string} highlightId - 高亮 ID
 * @param {UpdateHighlightRequest} data - 需要更新的字段
 * @returns {Promise<Highlight>} 更新后的高亮对象
 */
export async function updateHighlight(paperId: string, highlightId: string, data: UpdateHighlightRequest): Promise<Highlight> { // 导出更新高亮函数
  return request<Highlight>(`/papers/${paperId}/highlights/${highlightId}`, { // PUT 请求更新高亮
    method: 'PUT', // HTTP 方法为 PUT（更新资源）
    body: data,    // 请求体：需要更新的字段对象
  });
}

/**
 * 删除指定的高亮标注
 *
 * 用途：用户删除不需要的高亮。
 *
 * 注意事项：
 * - 删除操作不可逆，请确保用户已确认
 * - 后端返回 204 状态码，无响应体
 * - 前端需要从本地状态中移除已删除的高亮
 *
 * @param {string} paperId - 论文 ID
 * @param {string} highlightId - 高亮 ID
 * @returns {Promise<void>} 成功时无返回内容（204 状态码）
 */
export async function deleteHighlight(paperId: string, highlightId: string): Promise<void> { // 导出删除高亮函数
  return request<void>(`/papers/${paperId}/highlights/${highlightId}`, { // DELETE 请求删除高亮
    method: 'DELETE', // HTTP 方法为 DELETE（删除资源）
  });
}
