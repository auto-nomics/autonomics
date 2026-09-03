/**
 * @file 瓦片渲染插件 — Action 定义
 *
 * 本文件定义了瓦片渲染插件使用的所有 Redux 风格 action 类型及其创建函数。
 * 这些 action 驱动了瓦片状态机的完整生命周期：
 *
 * 生命周期流程：
 *   文档加载 → INIT_TILING_STATE（初始化瓦片状态）
 *            → UPDATE_VISIBLE_TILES（计算并更新可见瓦片）
 *            → MARK_TILE_STATUS（标记瓦片渲染状态 queued → rendering → ready）
 *   文档关闭 → CLEANUP_TILING_STATE（清理瓦片状态）
 */

import { Action } from '../core';
import { Tile, TileStatus, TilingDocumentState } from './types';

/* ======================== Action 类型常量 ======================== */

/** 初始化指定文档的瓦片状态（文档加载时触发） */
export const INIT_TILING_STATE = 'TILING/INIT_STATE';
/** 清理指定文档的瓦片状态（文档关闭时触发） */
export const CLEANUP_TILING_STATE = 'TILING/CLEANUP_STATE';
/** 更新指定文档的可见瓦片列表（滚动或缩放时触发） */
export const UPDATE_VISIBLE_TILES = 'TILING/UPDATE_VISIBLE_TILES';
/** 标记某个瓦片的渲染状态变化（渲染开始/完成时触发） */
export const MARK_TILE_STATUS = 'TILING/MARK_TILE_STATUS';

/* ======================== Action 接口定义 ======================== */

/** 初始化瓦片状态的 action，携带文档 ID 和初始状态 */
export interface InitTilingStateAction extends Action {
  type: typeof INIT_TILING_STATE;
  payload: {
    /** 文档唯一标识 */
    documentId: string;
    /** 该文档的初始瓦片状态 */
    state: TilingDocumentState;
  };
}

/** 清理瓦片状态的 action，payload 为要清理的文档 ID */
export interface CleanupTilingStateAction extends Action {
  type: typeof CLEANUP_TILING_STATE;
  payload: string; // documentId
}

/** 更新可见瓦片列表的 action，携带按页码索引的新瓦片集合 */
export type UpdateVisibleTilesAction = {
  type: typeof UPDATE_VISIBLE_TILES;
  payload: {
    /** 文档唯一标识 */
    documentId: string;
    /** 键为 0-based 页码，值为该页的新瓦片列表（isFallback 均为 false） */
    tiles: Record<number, Tile[]>;
  };
};

/** 标记单个瓦片状态变化的 action */
export type MarkTileStatusAction = {
  type: typeof MARK_TILE_STATUS;
  payload: {
    /** 文档唯一标识 */
    documentId: string;
    /** 页码索引（0-based） */
    pageIndex: number;
    /** 瓦片唯一标识符 */
    tileId: string;
    /** 要设置的新状态 */
    status: TileStatus;
  };
};

/** 瓦片插件所有 action 类型的联合类型 */
export type TilingAction =
  | InitTilingStateAction
  | CleanupTilingStateAction
  | UpdateVisibleTilesAction
  | MarkTileStatusAction;

/* ======================== Action 创建函数 ======================== */

/**
 * 创建初始化瓦片状态的 action。
 * @param documentId - 文档唯一标识
 * @param state - 该文档的初始瓦片状态
 */
export const initTilingState = (
  documentId: string,
  state: TilingDocumentState,
): InitTilingStateAction => ({
  type: INIT_TILING_STATE,
  payload: { documentId, state },
});

/**
 * 创建清理瓦片状态的 action。
 * @param documentId - 要清理的文档 ID
 */
export const cleanupTilingState = (documentId: string): CleanupTilingStateAction => ({
  type: CLEANUP_TILING_STATE,
  payload: documentId,
});

/**
 * 创建更新可见瓦片列表的 action。
 * @param documentId - 文档唯一标识
 * @param tiles - 按页码索引的新瓦片集合
 */
export const updateVisibleTiles = (
  documentId: string,
  tiles: Record<number, Tile[]>,
): UpdateVisibleTilesAction => ({
  type: UPDATE_VISIBLE_TILES,
  payload: { documentId, tiles },
});

/**
 * 创建标记瓦片状态的 action。
 * @param documentId - 文档唯一标识
 * @param pageIndex - 页码索引（0-based）
 * @param tileId - 瓦片唯一标识符
 * @param status - 要设置的新渲染状态
 */
export const markTileStatus = (
  documentId: string,
  pageIndex: number,
  tileId: string,
  status: TileStatus,
): MarkTileStatusAction => ({
  type: MARK_TILE_STATUS,
  payload: { documentId, pageIndex, tileId, status },
});
