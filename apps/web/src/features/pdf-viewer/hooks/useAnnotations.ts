/**
 * JayRead PDF 高亮标注管理 Hook
 *
 * 职责：管理 PDF 高亮标注的数据和状态
 *
 * 功能：
 * - 加载论文的高亮列表（支持按页码过滤）
 * - 创建新的高亮标注
 * - 更新高亮的颜色和备注
 * - 删除指定的高亮标注
 * - 按页码分组高亮，便于渲染
 *
 * 数据流：
 * 1. 组件挂载时加载全部高亮（一次性获取）
 * 2. 高亮按页码分组存储（highlightsByPage: Map<page, highlights[]>）
 * 3. 渲染时根据当前页码获取对应高亮
 * 4. 创建/更新/删除操作后同步更新本地状态
 *
 * 为什么使用 Map 而不是数组？
 * - Map 的查找是 O(1)，数组查找是 O(n)
 * - 按页码分组后，获取当前页高亮非常高效
 * - 页码是整数，天然适合作为 Map 的键
 *
 * 为什么一次加载全部高亮？
 * - 大多数论文的高亮数量有限（通常 < 100）
 * - 避免翻页时频繁请求后端
 * - 本地缓存支持快速导航和搜索
 */

import { useState, useCallback, useEffect } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // 导入 App 组件（用于获取 message 实例）
import {
  fetchHighlights,
  createHighlight,
  updateHighlight,
  deleteHighlight,
} from '../../../services/highlightsApi'; // 导入高亮 API 函数
import { notifier } from '../../../bus';

/**
 * PDF 高亮标注管理 Hook
 *
 * @param {object} options - 配置选项
 * @param {number} options.paperId - 论文 ID
 * @param {boolean} [options.enabled=true] - 是否启用高亮功能
 *                                        为 false 时不加载高亮（性能优化）
 * @returns {object} 高亮管理接口对象
 */
export function useAnnotations({ paperId, enabled = true }: {
  paperId: string;
  enabled?: boolean;
}) {
  const { message } = App.useApp();
  // =========================================
  // 状态定义
  // =========================================

  /**
   * 全部高亮的按页码分组映射
   *
   * 数据结构：Map<number, Array<object>>
   * - 键：页码（从 1 开始）
   * - 值：该页的高亮数组
   *
   * 为什么使用 Map 而不是 Object？
   * - Map 的键可以是任意类型，Object 的键只能是字符串
   * - Map 有明确的 size 属性，Object 需要手动计算
   * - Map 的遍历顺序是插入顺序，更可预测
   */
  const [highlightsByPage, setHighlightsByPage] = useState(new Map());

  /**
   * 加载状态
   *
   * 三种状态：
   * - 'idle': 未开始加载（组件刚挂载或 paperId 为空）
   * - 'loading': 正在加载
   * - 'loaded': 加载完成
   * - 'error': 加载失败
   */
  const [loadingStatus, setLoadingStatus] = useState('idle');

  /**
   * 错误信息
   *
   * 存储加载或操作失败的错误描述
   */
  const [error, setError] = useState<string | null>(null);

  // =========================================
  // 加载高亮数据
  // =========================================

  /**
   * 从后端加载指定论文的全部高亮
   *
   * 为什么不按页码分批加载？
   * - 大多数论文的高亮数量有限（< 100），一次加载影响不大
   * - 用户可能快速翻页，分批加载会造成频繁请求
   * - 本地缓存支持高亮搜索和统计功能
   */
  const loadHighlights = useCallback(async () => {
    // 前置条件检查：论文 ID 必须存在且功能已启用
    if (!paperId || !enabled) {
      return; // 不满足条件，跳过加载
    }

    // 设置加载状态为"正在加载"
    setLoadingStatus('loading');
    setError(null); // 清除旧的错误信息

    try {
      // 调用后端 API 获取全部高亮（不传 page 参数）
      const response = await fetchHighlights(paperId);

      // 按页码分组高亮
      const grouped = new Map();

      // 遍历所有高亮，按页码分类
      if (response.highlights && Array.isArray(response.highlights)) {
        for (const highlight of response.highlights) {
          const page = highlight.page; // 获取高亮所在的页码

          // 如果该页码还没有数组，创建一个新数组
          if (!grouped.has(page)) {
            grouped.set(page, []);
          }

          // 将高亮添加到对应页码的数组中
          grouped.get(page).push(highlight);
        }
      }

      // 更新状态：设置分组后的高亮数据
      setHighlightsByPage(grouped);

      // 设置加载状态为"已完成"
      setLoadingStatus('loaded');
    } catch (err) {
      // 加载失败：记录错误信息
      console.error('加载高亮失败:', err);
      setError((err as Error).message);
      setLoadingStatus('error');

      // 显示用户友好的错误提示
      message.error(`加载高亮失败: ${(err as Error).message}`);
    }
  }, [paperId, enabled]); // 依赖于论文 ID 和启用状态

  // =========================================
  // 初始化加载：paperId 变化时重新加载
  // =========================================

  /**
   * 监听 paperId 变化，自动重新加载高亮
   *
   * 触发时机：
   * - 组件首次挂载（paperId 从 undefined 变为有效值）
   * - 用户切换到另一篇论文（paperId 改变）
   */
  useEffect(() => {
    loadHighlights();
  }, [loadHighlights]); // 依赖于 loadHighlights 函数

  // =========================================
  // 获取指定页码的高亮列表
  // =========================================

  /**
   * 获取指定页码的高亮列表
   *
   * 用途：渲染时传入当前页码，获取该页需要渲染的高亮。
   *
   * @param {number} pageNum - 页码（从 1 开始）
   * @returns {Array<object>} 该页的高亮数组，如果没有则返回空数组
   */
  const getPageHighlights = useCallback((pageNum: number) => {
    return highlightsByPage.get(pageNum) || [];
  }, [highlightsByPage]); // 依赖于分组数据

  // =========================================
  // 创建新高亮
  // =========================================

  /**
   * 创建新的高亮标注
   *
   * 流程：
   * 1. 调用后端 API 创建高亮
   * 2. 后端返回带 ID 的高亮对象
   * 3. 将新高亮添加到本地状态中
   * 4. 显示成功提示
   *
   * 为什么需要乐观更新？
   * - 用户期望立即看到高亮，不希望等待网络请求
   * - 但高亮需要后端生成的 ID，无法完全乐观更新
   * - 这里使用先创建后更新的策略，保证数据一致性
   *
   * @param {object} data - 高亮数据对象
   * @param {number} data.page - 所在页码
   * @param {string} data.text - 高亮的文本内容
   * @param {Array<object>} data.rects - 归一化矩形坐标数组
   * @param {string} [data.color='#ffe066'] - 高亮颜色
   * @returns {Promise<object>} 创建成功的高亮对象（包含后端生成的 id）
   */
  const create = useCallback(async (data: any) => {
    if (!paperId) {
      throw new Error('论文 ID 不存在');
    }

    // Optimistic update: add a temporary highlight immediately
    const tempId = `temp-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
    const optimisticHighlight = { ...data, id: tempId, _optimistic: true };

    setHighlightsByPage(prev => {
      const next = new Map(prev);
      const page = data.page;
      const pageHighlights = next.get(page) || [];
      pageHighlights.push(optimisticHighlight);
      next.set(page, pageHighlights);
      return next;
    });

    try {
      // Call API to create highlight on server
      const newHighlight = await createHighlight(paperId, data);
      notifier.trigger('add', 'annotation', [String(newHighlight.id)]);

      // Replace optimistic highlight with real one
      setHighlightsByPage(prev => {
        const next = new Map(prev);
        const page = data.page;
        const pageHighlights = next.get(page) || [];
        const index = pageHighlights.findIndex((h: any) => h.id === tempId);
        if (index !== -1) {
          pageHighlights[index] = newHighlight;
        } else {
          pageHighlights.push(newHighlight);
        }
        next.set(page, pageHighlights);
        return next;
      });

      return newHighlight;
    } catch (err) {
      // Rollback: remove the optimistic highlight
      setHighlightsByPage(prev => {
        const next = new Map(prev);
        const page = data.page;
        const pageHighlights = next.get(page) || [];
        const filtered = pageHighlights.filter((h: any) => h.id !== tempId);
        if (filtered.length === 0) {
          next.delete(page);
        } else {
          next.set(page, filtered);
        }
        return next;
      });

      console.error('创建高亮失败:', err);
      message.error(`创建高亮失败: ${(err as Error).message}`);
      throw err;
    }
  }, [paperId, message]); // 依赖于论文 ID 和 message 实例

  // =========================================
  // 更新高亮
  // =========================================

  /**
   * 更新指定高亮的颜色或备注
   *
   * 流程：
   * 1. 调用后端 API 更新高亮
   * 2. 后端返回更新后的高亮对象
   * 3. 在本地状态中找到并替换该高亮
   * 4. 显示成功提示
   *
   * 为什么需要在本地状态中查找高亮？
   * - 高亮按页码分组，需要先找到所属页
   * - 然后在页的数组中找到对应的高亮对象
   * - 使用高亮 ID 匹配，确保更新正确的对象
   *
   * @param {number} highlightId - 高亮 ID
   * @param {object} data - 需要更新的字段
   * @param {string} [data.color] - 新的高亮颜色
   */
  const update = useCallback(async (highlightId: string | number, data: any) => {
    if (!paperId) {
      throw new Error('论文 ID 不存在');
    }

    try {
      // 调用后端 API 更新高亮
      const updatedHighlight = await updateHighlight(paperId, String(highlightId), data);
      notifier.trigger('modify', 'annotation', [String(highlightId)]);

      // 更新本地状态：找到并替换高亮对象
      setHighlightsByPage(prev => {
        const next = new Map(prev); // 创建新 Map

        // 遍历所有页码，查找包含该高亮的页
        for (const [page, highlights] of next.entries()) {
          // 在该页的高亮数组中查找目标高亮
          const index = highlights.findIndex((h: any) => h.id === highlightId);

          if (index !== -1) {
            // 找到了！创建新数组，替换该高亮
            const newHighlights = [...highlights];
            newHighlights[index] = updatedHighlight;

            // 更新 Map 中的数组
            next.set(page, newHighlights);

            // 找到后就退出循环（高亮 ID 是唯一的）
            break;
          }
        }

        return next; // 返回新的 Map
      });

      // 显示成功提示
      message.success('高亮更新成功');
    } catch (err) {
      console.error('更新高亮失败:', err);
      message.error(`更新高亮失败: ${(err as Error).message}`);
      throw err; // 重新抛出错误，让调用方处理
    }
  }, [paperId, message]); // 依赖于论文 ID 和 message 实例

  // =========================================
  // 删除高亮
  // =========================================

  /**
   * 删除指定的高亮标注
   *
   * 流程：
   * 1. 调用后端 API 删除高亮
   * 2. 在本地状态中找到并移除该高亮
   * 3. 如果该页没有高亮了，清理该页的条目
   * 4. 显示成功提示
   *
   * 为什么需要清理空页？
   * - 删除后某页可能没有高亮了
   * - 清理空条目可以减少内存占用
   * - 遍历时不需要检查空数组
   *
   * @param {number} highlightId - 高亮 ID
   * @param {number} page - 高亮所在页码（用于快速定位）
   */
  const remove = useCallback(async (highlightId: string | number, page: number) => {
    if (!paperId) {
      throw new Error('论文 ID 不存在');
    }

    // Optimistic update: remove immediately, and capture snapshot for rollback
    let stateSnapshot: Map<number, any[]> | null = null;
    setHighlightsByPage(prev => {
      stateSnapshot = prev;
      const next = new Map(prev);
      const pageHighlights = next.get(page);

      if (pageHighlights) {
        const newHighlights = pageHighlights.filter((h: any) => h.id !== highlightId);
        if (newHighlights.length === 0) {
          next.delete(page);
        } else {
          next.set(page, newHighlights);
        }
      }

      return next;
    });

    try {
      // Call API to delete on server
      await deleteHighlight(paperId, String(highlightId));
      notifier.trigger('delete', 'annotation', [String(highlightId)]);
      message.success('高亮删除成功');
    } catch (err) {
      // Rollback: use functional updater to restore the highlight only if state matches snapshot
      if (stateSnapshot) {
        setHighlightsByPage(current => {
          // Only rollback if the state still matches our snapshot (no concurrent updates)
          const currentHighlights = current.get(page) || [];
          const highlightWasRemoved = !currentHighlights.some((h: any) => h.id === highlightId);

          if (!highlightWasRemoved) {
            // Highlight already restored, no action needed
            return current;
          }

          // Restore the highlight from snapshot
          const next = new Map(current);
          const pageHighlights = next.get(page) || [];
          const removedHighlight = stateSnapshot!.get(page)?.find((h: any) => h.id === highlightId);

          if (removedHighlight) {
            pageHighlights.push(removedHighlight);
            next.set(page, pageHighlights);
          }

          return next;
        });
      }

      console.error('删除高亮失败:', err);
      message.error(`删除高亮失败: ${(err as Error).message}`);
      throw err;
    }
  }, [paperId, message]); // 依赖于论文 ID 和 message 实例

  // =========================================
  // 返回接口对象
  // =========================================

  return {
    // 状态
    highlightsByPage,      // 按页码分组的高亮数据
    loadingStatus,         // 加载状态
    error,                 // 错误信息

    // 查询方法
    getPageHighlights,     // 获取指定页码的高亮列表

    // 操作方法
    create,                // 创建新高亮
    update,                // 更新高亮
    remove,                // 删除高亮
    loadHighlights,        // 重新加载高亮
  };
}

/**
 * 默认导出
 */
export default useAnnotations;
