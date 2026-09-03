/**
 * 分类管理 API 客户端（autonomics collections）
 *
 * 前端概念「分类 / category」对应 autonomics 的「集合 / collection」。
 * 后端 `/collections` 永远返回**扁平**列表（含 parent_id + sort_order），树形
 * 结构在本层构建 —— 与 jayread 的既有契约（getCategoryTree 返回扁平列表 +
 * 前端自行建树）完全一致，组件零改动。
 *
 * autonomics 后端没有的、需要在前端推导的：
 * - `paper_count`（每分类的文献数）
 * - `total_count` / `uncategorized_count`（两个虚拟节点「全部 / 未分类」用）
 *
 * 好消息：`GET /collections` 返回的每个 Collection 都带注水好的 `article_ids`
 * （后端一次性批量查询），所以这三个数字都能从**同一次** /collections 请求推出：
 * paper_count = article_ids.length，uncategorized = total − 出现过的文章数。
 * 不需要再拉全量文章。
 */
import request, { invalidateCache } from './client';
import { fromSafeId, encodeIdSegment, collectionMembershipOf } from './mapping';
import type {
  GetCategoryTreeResponse,
  CreateCategoryResponse,
  RenameCategoryResponse,
  DeleteCategoryResponse,
  AssignPapersResponse,
  UnassignPapersResponse,
  GetPaperCategoriesResponse,
  MoveCategoryResponse,
  Category,
} from '@/types';

// ============================================================
// 后端契约类型
// ============================================================

/**
 * autonomics Collection（扁平，无 children）。
 *
 * `article_ids` 由后端批量注水（见 bib-base collections.rs 的 list_collections），
 * 前端不写它 —— 成员关系的增删只走 POST/DELETE /collections/{id}/articles。
 */
export interface BibCollection {
  id: string;
  name: string;
  description: string | null;
  tags: string[];
  parent_id: string | null;
  sort_order: number;
  status?: string;
  article_ids?: string[];
  created_at: string | null;
  updated_at: string | null;
}

// ============================================================
// 后端读取 + 树构建
// ============================================================

/** 拉取扁平集合列表（按 sort_order 稳定排序） */
async function fetchFlatCollections(): Promise<BibCollection[]> {
  const data = await request<{ collections?: BibCollection[] }>('/collections');
  const flat = Array.isArray(data?.collections) ? data.collections : [];
  return [...flat].sort((a, b) => {
    if (a.sort_order !== b.sort_order) return a.sort_order - b.sort_order;
    return String(a.name).localeCompare(String(b.name));
  });
}

/**
 * 从一次 `/collections` 请求推导 paper_count / total / unfiled。
 *
 * `total` 是全库文章数，得单独问一次 /articles（limit=1 只为拿 total，
 * offset=0 的那一页内容直接丢掉）。集合成员数直接用 article_ids.length。
 */
async function fetchCountSnapshot(): Promise<{ byCollection: Map<string, number>; total: number; unfiled: number }> {
  const [flat, pageRes] = await Promise.all([
    fetchFlatCollections(),
    request<{ total?: number }>('/articles?limit=1&offset=0').catch(() => null),
  ]);

  const total = typeof pageRes?.total === 'number' ? pageRes.total : 0;
  const byCollection = new Map<string, number>();
  const filed = new Set<string>();
  for (const col of flat) {
    const ids = col.article_ids ?? [];
    byCollection.set(col.id, ids.length);
    for (const id of ids) filed.add(id);
  }

  return { byCollection, total, unfiled: Math.max(0, total - filed.size) };
}

/** BibCollection → jayread Category */
function toCategory(col: BibCollection, paperCount: number): Category {
  return {
    id: col.id,
    name: col.name,
    parent_id: col.parent_id ?? null,
    sort_order: col.sort_order,
    paper_count: paperCount,
    created_at: col.created_at ?? new Date().toISOString(),
    updated_at: col.updated_at ?? new Date().toISOString(),
  };
}

// ============================================================
// 导出 API（签名与 jayread 版本一致）
// ============================================================

/**
 * 获取分类树（扁平列表）
 *
 * 返回所有分类及其直接归属的文献数量，外加 total_count / uncategorized_count
 * 供侧边栏的「全部 / 未分类」两个虚拟节点使用。前端根据 parent_id 自行建树。
 *
 * 为什么返回扁平列表而不是嵌套树？
 * - 与 jayread 契约一致，CategorySidebar 的 buildTreeData 已经这么用了
 * - Ant Design Tree 组件可以直接使用扁平数据
 *
 * @returns {Promise<GetCategoryTreeResponse>} 分类扁平列表
 */
export async function getCategoryTree(): Promise<GetCategoryTreeResponse> {
  // 集合列表和文章快照相互独立，并行拉取
  const [flat, snapshot] = await Promise.all([fetchFlatCollections(), fetchCountSnapshot()]);

  const categories = flat.map((col) => toCategory(col, snapshot.byCollection.get(col.id) ?? 0));

  return {
    categories,
    total_count: snapshot.total,
    uncategorized_count: snapshot.unfiled,
  };
}


/**
 * 创建新分类
 *
 * 支持在根级别或某个分类下创建子分类。
 *
 * @param {string} name - 分类名称
 * @param {string|null} [parentId=null] - 父分类 ID，null 表示顶级分类
 * @returns {Promise<CreateCategoryResponse>} 新创建的分类信息
 */
export async function createCategory(name: string, parentId: string | null = null): Promise<CreateCategoryResponse> {
  // 新节点排到兄弟末尾：先数一下现有兄弟数作为 sort_order
  const flat = await fetchFlatCollections();
  const siblingCount = flat.filter((c) => (c.parent_id ?? null) === (parentId ?? null)).length;

  const created = await request<{ collection: BibCollection }>('/collections', {
    method: 'POST',
    body: {
      name,
      ...(parentId ? { parent_id: parentId } : {}),
      sort_order: siblingCount,
    },
  });
  invalidateCache('/collections');
  return toCategory(created.collection, 0);
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
export async function renameCategory(id: string, name: string): Promise<RenameCategoryResponse> {
  const result = await request<{ collection: BibCollection }>(`/collections/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: { name },
  });
  invalidateCache('/collections');
  return toCategory(result.collection, 0);
}


/**
 * 删除分类
 *
 * autonomics 后端决定子分类与成员关系的级联策略（删除集合不删除文章本身）。
 * 前端只需要发一个 DELETE。
 *
 * @param {string} id - 要删除的分类 ID
 * @returns {Promise<DeleteCategoryResponse>} 操作结果消息
 */
export async function deleteCategory(id: string): Promise<DeleteCategoryResponse> {
  // 后端返回 200 + {"deleted":true,"id"}，不是 204；成败只看状态码
  await request<{ deleted?: boolean }>(`/collections/${encodeURIComponent(id)}`, { method: 'DELETE' });
  invalidateCache('/collections');
  return { message: '已删除' };
}


/**
 * 将论文分配到分类（批量操作）
 *
 * 支持「论文 × 分类」的所有组合。autonomics 只有单篇端点
 * `POST /collections/{id}/articles`，这里按组合循环展开。
 * 后端对重复归入幂等（响应里 `inserted:false` 表示本来就在），所以无需前端去重。
 *
 * 注意 body 的 `article_id` 必须是**真实 ID**：后端先按它查文章，查不到直接 404。
 *
 * @param {string[]} paperIds - 论文 ID 列表（safeId）
 * @param {string[]} categoryIds - 分类 ID 列表
 * @returns {Promise<AssignPapersResponse>} 操作结果消息
 */
export async function assignPapers(paperIds: string[], categoryIds: string[]): Promise<AssignPapersResponse> {
  for (const categoryId of categoryIds) {
    for (const paperId of paperIds) {
      await request<{ inserted?: boolean }>(`/collections/${encodeURIComponent(categoryId)}/articles`, {
        method: 'POST',
        body: { article_id: fromSafeId(paperId) },
      });
    }
  }
  invalidateCache('/collections');
  return { message: '已分配' };
}


/**
 * 将论文从分类中移除（批量操作）
 *
 * 路径段同样是**真实 ID**（后端不做 base64 解码，直接进 SQL），所以这里用
 * 百分号编码而不是 safeId。关联不存在时后端也返回 200 + removed:true，与
 * jayread 的「静默忽略」语义一致。
 *
 * @param {string[]} paperIds - 论文 ID 列表（safeId）
 * @param {string[]} categoryIds - 分类 ID 列表
 * @returns {Promise<UnassignPapersResponse>} 操作结果消息
 */
export async function unassignPapers(paperIds: string[], categoryIds: string[]): Promise<UnassignPapersResponse> {
  for (const categoryId of categoryIds) {
    for (const paperId of paperIds) {
      await request<{ removed?: boolean }>(`/collections/${encodeURIComponent(categoryId)}/articles/${encodeIdSegment(fromSafeId(paperId))}`, {
        method: 'DELETE',
      });
    }
  }
  invalidateCache('/collections');
  return { message: '已移除' };
}


/**
 * 获取某篇论文所属的所有分类
 *
 * 当前无线上调用方（仅遗留导入）。Article 不携带集合成员关系，这里从
 * `/collections` 的 article_ids 倒排 —— 与 getPapers 的做法一致。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @returns {Promise<GetPaperCategoriesResponse>} 论文的分类 ID 列表
 */
export async function getPaperCategories(paperId: string): Promise<GetPaperCategoriesResponse> {
  const realId = fromSafeId(paperId);
  const data = await request<{ collections?: BibCollection[] }>('/collections');
  const membership = collectionMembershipOf(data?.collections);
  // jayread 的类型把这两个字段标成 number，autonomics 全是 string id —— 类型定义
  // 本身已过期（Phase 5 清扫），这里用 cast 保持导出签名不变
  return {
    paper_id: paperId as unknown as GetPaperCategoriesResponse['paper_id'],
    category_ids: (membership.get(realId) ?? []) as unknown as GetPaperCategoriesResponse['category_ids'],
  };
}


/**
 * 移动分类（拖拽排序）
 *
 * 通过拖拽操作改变分类的父级关系和排序位置。
 *
 * sortOrder 是「目标位置在新兄弟列表中的下标（0 起）」—— CategorySidebar 计算
 * dropPosition 时已扣除虚拟节点数。autonomics 的 PUT 只接受绝对 sort_order，
 * 不做兄弟重排，所以这里由前端负责把新父下的所有兄弟重新编号为 0..n-1。
 *
 * 顺序：先 PUT 目标节点（换父 + 落位），再按最终顺序 PUT 其余兄弟。
 * sort_order 只是普通整数列，中间态允许重复，无需事务。
 *
 * @param {string} id - 要移动的分类 ID
 * @param {string|null} parentId - 新的父分类 ID，null 表示移动到根级别
 * @param {number} sortOrder - 目标排序位置（0 为第一个位置）
 * @returns {Promise<MoveCategoryResponse>} 移动后的分类信息
 */
export async function moveCategory(id: string, parentId: string | null, sortOrder: number): Promise<MoveCategoryResponse> {
  const flat = await fetchFlatCollections();
  const target = flat.find((c) => c.id === id);

  // 目标新父下的现有兄弟（不含自己），按 sort_order 排好
  const siblings = flat
    .filter((c) => c.id !== id && (c.parent_id ?? null) === (parentId ?? null))
    .sort((a, b) => a.sort_order - b.sort_order);

  // 插入目标，得到最终顺序
  const insertAt = Math.max(0, Math.min(sortOrder, siblings.length));
  const finalOrder = [...siblings.slice(0, insertAt), { id, name: target?.name ?? '' }, ...siblings.slice(insertAt)];

  // 1) 换父 + 落位
  await request<BibCollection>(`/collections/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: {
      ...(parentId ? { parent_id: parentId } : { parent_id: null }),
      sort_order: insertAt,
    },
  });

  // 2) 重排其余兄弟（跳过目标，它已落位）
  for (let i = 0; i < finalOrder.length; i++) {
    const node = finalOrder[i];
    if (node.id === id) continue;
    if (siblings.find((s) => s.id === node.id)?.sort_order === i) continue; // 未变，跳过
    await request(`/collections/${encodeURIComponent(node.id)}`, {
      method: 'PUT',
      body: { sort_order: i },
    });
  }

  invalidateCache('/collections');

  return {
    id,
    name: target?.name ?? '',
    parent_id: parentId,
    sort_order: insertAt,
    paper_count: 0,
    created_at: target?.created_at ?? new Date().toISOString(),
    updated_at: new Date().toISOString(),
  } as MoveCategoryResponse;
}
