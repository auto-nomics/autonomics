/**
 * JayRead 分类管理 API 客户端
 *
 * 封装所有文献分类相关的后端接口调用，包括：
 * - 获取分类树（扁平列表，前端自行构建树形结构）
 * - 分类 CRUD：创建、重命名、删除分类
 * - 论文分配：将论文归入/移出分类（多对多关系）
 * - 查询论文所属分类
 *
 * 为什么需要独立的 API 模块？
 * - 职责分离：分类管理与论文管理是不同的业务领域
 * - 便于维护：分类相关的接口集中在一个文件中
 * - 复用性：多个组件（侧边栏、论文列表）都可以使用同一套 API
 */
import request, { invalidateCache } from './client'; // 导入通用 API 客户端封装函数，自动处理 JSON 序列化和错误和缓存失效函数
import type {
  GetCategoryTreeResponse,
  CreateCategoryResponse,
  RenameCategoryResponse,
  DeleteCategoryResponse,
  AssignPapersResponse,
  UnassignPapersResponse,
  GetPaperCategoriesResponse,
  MoveCategoryResponse,
} from '@/types';


/**
 * 获取分类树（扁平列表）
 *
 * 返回所有分类及其直接归属的论文数量。
 * 前端根据 parent_id 字段自行构建树形结构。
 *
 * 为什么返回扁平列表而不是嵌套树？
 * - 后端逻辑更简单，不需要递归构建
 * - 分类数量通常较少（< 100），前端构建树性能完全够用
 * - Ant Design Tree 组件可以直接使用扁平数据
 *
 * @returns {Promise<GetCategoryTreeResponse>} 分类扁平列表
 */
export async function getCategoryTree(): Promise<GetCategoryTreeResponse> { // 导出获取分类树函数
  // GET 请求获取所有分类数据
  return request<GetCategoryTreeResponse>('/categories/tree'); // 返回分类扁平列表
}


/**
 * 创建新分类
 *
 * 支持在根级别或某个分类下创建子分类。
 * 通过 parentId 参数指定父分类。
 *
 * @param {string} name - 分类名称，长度 1-100 字符
 * @param {string|null} [parentId=null] - 父分类 ID，null 表示顶级分类
 * @returns {Promise<CreateCategoryResponse>} 新创建的分类信息
 */
export async function createCategory(name: string, parentId: string | null = null): Promise<CreateCategoryResponse> { // 导出创建分类函数
  // POST 请求创建新分类
  const result = await request<CreateCategoryResponse>('/categories', { // 发送 POST 请求到分类端点
    method: 'POST', // HTTP 方法为 POST（创建资源）
    body: { // 请求体
      name: name, // 分类名称
      parentId, // 父分类 ID（null 时为顶级分类，camelCase 匹配后端 serde rename）
    },
  });
  invalidateCache('/categories');
  return result;
}


/**
 * 重命名分类
 *
 * 只修改分类的显示名称，不改变其在树形结构中的位置。
 *
 * @param {string} id - 要重命名的分类 ID
 * @param {string} name - 新的分类名称
 * @returns {Promise<RenameCategoryResponse>} 更新后的分类信息
 */
export async function renameCategory(id: string, name: string): Promise<RenameCategoryResponse> { // 导出重命名分类函数
  // PUT 请求更新分类名称
  const result = await request<RenameCategoryResponse>(`/categories/${id}`, { // 发送 PUT 请求到指定分类端点
    method: 'PUT', // HTTP 方法为 PUT（更新资源）
    body: { name: name }, // 请求体：只包含新名称
  });
  invalidateCache('/categories');
  return result;
}


/**
 * 删除分类
 *
 * 级联删除策略：
 * - 删除该分类本身
 * - 自动删除所有子分类（数据库外键 CASCADE）
 * - 自动删除所有论文-分类关联（数据库外键 CASCADE）
 * - 不会删除论文本身，只移除关联关系
 *
 * @param {string} id - 要删除的分类 ID
 * @returns {Promise<DeleteCategoryResponse>} 操作结果消息
 */
export async function deleteCategory(id: string): Promise<DeleteCategoryResponse> { // 导出删除分类函数
  // DELETE 请求删除指定分类
  const result = await request<DeleteCategoryResponse>(`/categories/${id}`, { // 发送 DELETE 请求到指定分类端点
    method: 'DELETE', // HTTP 方法为 DELETE（删除资源）
  });
  invalidateCache('/categories');
  return result;
}


/**
 * 将论文分配到分类（批量操作）
 *
 * 支持将多篇论文一次性归入多个分类。
 * 使用交叉插入：paperIds × categoryIds 的所有组合。
 * 后端使用 INSERT OR IGNORE 保证幂等性（重复归入不报错）。
 *
 * @param {string[]} paperIds - 论文 ID 列表
 * @param {string[]} categoryIds - 分类 ID 列表
 * @returns {Promise<AssignPapersResponse>} 操作结果消息
 */
export async function assignPapers(paperIds: string[], categoryIds: string[]): Promise<AssignPapersResponse> { // 导出分配论文到分类函数
  // POST 请求批量分配论文到分类
  const result = await request<AssignPapersResponse>('/categories/assign', { // 发送 POST 请求到分配端点
    method: 'POST', // HTTP 方法为 POST
    body: { // 请求体
      itemIds: paperIds, // 论文 ID 数组（camelCase 匹配后端 AssignItemsRequest.itemIds）
      categoryIds, // 分类 ID 数组
    },
  });
  invalidateCache('/categories');
  invalidateCache('/papers');
  return result;
}


/**
 * 将论文从分类中移除（批量操作）
 *
 * 支持将多篇论文从多个分类中一次性移除。
 * 如果关联不存在，后端 DELETE 语句静默忽略（不报错）。
 *
 * @param {string[]} paperIds - 论文 ID 列表
 * @param {string[]} categoryIds - 分类 ID 列表
 * @returns {Promise<UnassignPapersResponse>} 操作结果消息
 */
export async function unassignPapers(paperIds: string[], categoryIds: string[]): Promise<UnassignPapersResponse> { // 导出从分类移除论文函数
  // POST 请求批量从分类移除论文
  const result = await request<UnassignPapersResponse>('/categories/unassign', { // 发送 POST 请求到取消分配端点
    method: 'POST', // HTTP 方法为 POST
    body: { // 请求体
      itemIds: paperIds, // 论文 ID 数组（camelCase 匹配后端 UnassignItemsRequest.itemIds）
      categoryIds, // 分类 ID 数组
    },
  });
  invalidateCache('/categories');
  invalidateCache('/papers');
  return result;
}


/**
 * 获取某篇论文所属的所有分类
 *
 * 用于在论文列表项上显示分类标签，
 * 或在分类管理弹窗中显示论文当前所属的分类。
 *
 * @param {string} paperId - 论文 ID
 * @returns {Promise<GetPaperCategoriesResponse>} 论文的分类 ID 列表
 */
export async function getPaperCategories(paperId: string): Promise<GetPaperCategoriesResponse> { // 导出获取论文分类函数
  // GET 请求获取指定论文的分类列表
  return request<GetPaperCategoriesResponse>(`/categories/paper/${paperId}`); // 返回论文的分类 ID 数组
}


/**
 * 移动分类（拖拽排序）
 *
 * 通过拖拽操作改变分类的父级关系和排序位置。
 * 后端会自动校验循环引用，并调整同级节点的排序。
 *
 * 使用场景：
 * - 在分类树中拖拽分类节点到新位置
 * - 将分类从一个父节点移动到另一个父节点
 * - 调整分类在同级节点中的排列顺序
 *
 * @param {string} id - 要移动的分类 ID
 * @param {string|null} parentId - 新的父分类 ID，null 表示移动到根级别
 * @param {number} sortOrder - 目标排序位置，值越小越靠前（0 为第一个位置）
 * @returns {Promise<MoveCategoryResponse>} 移动后的分类信息
 */
export async function moveCategory(id: string, parentId: string | null, sortOrder: number): Promise<MoveCategoryResponse> { // 导出移动分类函数
  // PUT 请求更新分类的父级和排序位置
  const result = await request<MoveCategoryResponse>(`/categories/${id}/move`, { // 发送 PUT 请求到移动端点
    method: 'PUT', // HTTP 方法为 PUT（更新资源）
    body: { // 请求体
      parentId, // 新的父分类 ID（null 表示根级别，camelCase 匹配后端 serde rename）
      sortOrder, // 目标排序位置（camelCase 匹配后端 serde rename）
    },
  });
  invalidateCache('/categories');
  return result;
}
