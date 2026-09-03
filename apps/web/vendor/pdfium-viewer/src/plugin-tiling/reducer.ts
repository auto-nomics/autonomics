/**
 * @file 瓦片渲染插件 — Reducer
 *
 * 本文件定义了瓦片渲染插件的状态归约器（reducer）和初始状态。
 * reducer 负责处理四种 action 类型，维护按文档 ID 组织的瓦片状态树。
 *
 * 核心设计决策：
 *
 * 1. 降级瓦片（Fallback）机制：
 *    当用户缩放时，旧缩放级别的已渲染瓦片会被标记为 fallback，
 *    保留显示直到新缩放级别的瓦片全部渲染完成，从而避免缩放过程中的闪烁。
 *
 * 2. 状态合并策略：
 *    - 缩放变化：将旧瓦片降级为 fallback，加入新瓦片，一起渲染
 *    - 平移/滚动（同缩放）：保留已有的 fallback 瓦片和与新瓦片 ID 匹配的旧瓦片（保留其渲染状态），
 *      仅追加真正新增的瓦片
 *    - 瓦片全部就绪：自动移除 fallback 瓦片
 */

import { Reducer } from '../core';

import {
  UPDATE_VISIBLE_TILES,
  MARK_TILE_STATUS,
  TilingAction,
  INIT_TILING_STATE,
  CLEANUP_TILING_STATE,
} from './actions';
import { Tile, TilingDocumentState, TilingState } from './types';

/** 单个文档的初始瓦片状态，visibleTiles 为空对象 */
export const initialTilingDocumentState: TilingDocumentState = {
  visibleTiles: {},
};

/** 插件的顶层初始状态，documents 为空对象 */
export const initialState: TilingState = {
  documents: {},
};

/**
 * 瓦片插件的状态归约器。
 *
 * 处理四种 action：
 * - INIT_TILING_STATE     → 为新加载的文档初始化瓦片状态
 * - CLEANUP_TILING_STATE  → 清理已关闭文档的瓦片状态
 * - UPDATE_VISIBLE_TILES  → 更新可见瓦片列表（核心逻辑，含 fallback 机制）
 * - MARK_TILE_STATUS      → 标记单个瓦片的渲染状态，并在所有瓦片就绪时自动清理 fallback
 *
 * @param state - 当前瓦片状态树
 * @param action - 要处理的 action
 * @returns 更新后的新状态（不可变更新）
 */
export const tilingReducer: Reducer<TilingState, TilingAction> = (state, action) => {
  switch (action.type) {
    /* ---- 初始化：为新文档创建瓦片状态 ---- */
    case INIT_TILING_STATE: {
      const { documentId, state: docState } = action.payload;
      return {
        ...state,
        documents: {
          ...state.documents,
          [documentId]: docState,
        },
      };
    }

    /* ---- 清理：移除已关闭文档的瓦片状态 ---- */
    case CLEANUP_TILING_STATE: {
      const documentId = action.payload;
      // 利用解构赋值移除指定文档 ID 的条目
      const { [documentId]: removed, ...remaining } = state.documents;
      return {
        ...state,
        documents: remaining,
      };
    }

    /* ---- 更新可见瓦片：核心合并逻辑 ---- */
    case UPDATE_VISIBLE_TILES: {
      const { documentId, tiles: incoming } = action.payload; // incoming: 新计算的可见瓦片
      const docState = state.documents[documentId];
      if (!docState) return state; // 文档尚未初始化，忽略

      // 浅拷贝当前页面的瓦片映射，用于增量更新
      const nextPages = { ...docState.visibleTiles };

      for (const key in incoming) {
        const pageIndex = Number(key);
        const newTiles = incoming[pageIndex]; // 新计算的瓦片（isFallback 均为 false）
        const prevTiles = nextPages[pageIndex] ?? []; // 该页之前的瓦片列表

        // 检测缩放是否发生变化：比较旧瓦片和新瓦片的 srcScale
        const prevScale = prevTiles.find((t) => !t.isFallback)?.srcScale;
        const newScale = newTiles.length > 0 ? newTiles[0].srcScale : prevScale;
        const zoomChanged = prevScale !== undefined && prevScale !== newScale;

        if (zoomChanged) {
          /* === 缩放变化场景 === */

          // 步骤 1：将旧缩放级别中已渲染完成的瓦片提升为 fallback
          const promoted = prevTiles
            .filter((t) => !t.isFallback && t.status === 'ready')
            .map((t) => ({ ...t, isFallback: true }));

          // 步骤 2：决定是否保留之前的 fallback 瓦片
          // 如果有新的 fallback（promoted 非空），则丢弃旧 fallback（避免冗余）
          // 否则保留旧 fallback（保持降级显示）
          const fallbackToCarry = promoted.length > 0 ? [] : prevTiles.filter((t) => t.isFallback);

          // 步骤 3：最终列表 = 保留的旧 fallback + 新 fallback + 新瓦片
          nextPages[pageIndex] = [...fallbackToCarry, ...promoted, ...newTiles];
        } else {
          /* === 同缩放场景（平移/滚动） === */

          // 收集新瓦片的 ID 集合，用于快速查找
          const newIds = new Set(newTiles.map((t) => t.id));
          const keepers: Tile[] = []; // 存活下来的瓦片
          const seenIds = new Set<string>(); // 已处理的瓦片 ID，避免重复

          // 步骤 1：遍历旧瓦片，决定哪些需要保留
          for (const t of prevTiles) {
            if (t.isFallback) {
              // fallback 瓦片始终保留
              keepers.push(t);
              seenIds.add(t.id);
            } else if (newIds.has(t.id)) {
              // 如果旧瓦片仍在新可见列表中，保留旧实例（保持其渲染状态）
              keepers.push(t);
              seenIds.add(t.id);
            }
            // 否则：旧瓦片已不在可见范围内，自然淘汰
          }

          // 步骤 2：追加真正新增的瓦片（不在旧列表中的）
          for (const t of newTiles) {
            if (!seenIds.has(t.id)) keepers.push(t);
          }

          nextPages[pageIndex] = keepers;
        }
      }

      return {
        ...state,
        documents: {
          ...state.documents,
          [documentId]: {
            ...docState,
            visibleTiles: nextPages,
          },
        },
      };
    }

    /* ---- 标记瓦片渲染状态 ---- */
    case MARK_TILE_STATUS: {
      const { documentId, pageIndex, tileId, status } = action.payload;
      const docState = state.documents[documentId];
      if (!docState) return state;

      // 更新目标瓦片的状态，其余瓦片保持不变
      const tiles =
        docState.visibleTiles[pageIndex]?.map((t) =>
          t.id === tileId ? ({ ...t, status } as Tile) : t,
        ) ?? [];

      // 检查该页所有非 fallback 瓦片是否都已渲染完成
      const newTiles = tiles.filter((t) => !t.isFallback);
      const allReady = newTiles.length > 0 && newTiles.every((t) => t.status === 'ready');
      // 如果全部就绪，则移除 fallback 瓦片（不再需要降级显示）
      const finalTiles = allReady ? newTiles : tiles;

      return {
        ...state,
        documents: {
          ...state.documents,
          [documentId]: {
            ...docState,
            visibleTiles: { ...docState.visibleTiles, [pageIndex]: finalTiles },
          },
        },
      };
    }

    default:
      return state;
  }
};
