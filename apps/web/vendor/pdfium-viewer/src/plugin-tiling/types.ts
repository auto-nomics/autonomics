/**
 * @file 瓦片渲染插件 — 类型定义
 *
 * 本文件定义了瓦片式渲染（Tiling）插件所需的全部类型接口。
 * 瓦片渲染的核心思想是将一页 PDF 按固定大小的网格切分为若干瓦片（Tile），
 * 仅渲染当前视口内可见（及邻近）的瓦片，从而降低单次渲染开销并支持增量更新。
 */

import { BasePluginConfig, EventHook } from '../core';
import {
  ImageConversionTypes,
  PdfErrorReason,
  PdfPageObject,
  Rect,
  Rotation,
  Task,
} from '../models';
import { PageVisibilityMetrics } from '../plugin-scroll';

/** 瓦片插件的配置项，可在插件初始化时传入 */
export interface TilingPluginConfig extends BasePluginConfig {
  /** 瓦片边长（像素）。每个瓦片的默认宽高为 tileSize × tileSize，边缘瓦片可能更小 */
  tileSize: number;
  /** 相邻瓦片之间重叠区域的大小（屏幕像素）。
   *  重叠用于消除瓦片拼接处的子像素缝隙（seam），典型值 2-3px */
  overlapPx: number;
  /** 在可见区域外围额外渲染的瓦片圈数。
   *  0 = 仅渲染可见瓦片；1 = 可见区域外再扩展一圈，用于预渲染 */
  extraRings: number;
  /**
   * 可选的瓦片渲染图片类型覆盖。
   * 若省略，瓦片渲染将回退到 render 插件的默认设置。
   */
  defaultImageType?: ImageConversionTypes;
}

/** 视口在某个页面上的可见矩形区域（屏幕坐标系） */
export interface VisibleRect {
  /** 可见区域左上角在页面坐标系中的 x 偏移（像素） */
  pageX: number;
  /** 可见区域左上角在页面坐标系中的 y 偏移（像素） */
  pageY: number;
  /** 可见区域宽度（像素） */
  visibleWidth: number;
  /** 可见区域高度（像素） */
  visibleHeight: number;
}

/**
 * 瓦片的生命周期状态
 *
 * - `queued`    — 已入队，等待渲染
 * - `rendering` — 正在由渲染插件处理中
 * - `ready`     — 渲染完成，图像可用
 */
export type TileStatus = 'queued' | 'rendering' | 'ready';

/**
 * 单个瓦片的描述
 *
 * 一个瓦片代表 PDF 页面上的一块矩形区域，既包含页面上以 PDF 点（pt）为单位的坐标，
 * 也包含以屏幕像素为单位的坐标。渲染时使用 pageRect 指定裁剪区域，
 * 显示时使用 screenRect 定位和缩放。
 */
export interface Tile {
  /** 当前瓦片的渲染状态 */
  status: TileStatus;
  /** 瓦片在屏幕上的矩形区域（像素坐标），用于 CSS 定位 */
  screenRect: Rect;
  /** 瓦片在 PDF 页面上的矩形区域（PDF 点坐标），用于渲染裁剪 */
  pageRect: Rect;
  /** 是否为旧缩放级别遗留的降级瓦片。
   *  缩放变化时，旧瓦片标记为 fallback 保留显示，直到新瓦片渲染完成 */
  isFallback: boolean;
  /** 渲染此瓦片时使用的缩放比例。
   *  用于在当前 scale 与 srcScale 不同时按比例缩放 CSS 尺寸 */
  srcScale: number;
  /** 瓦片在网格中的列索引 */
  col: number;
  /** 瓦片在网格中的行索引 */
  row: number;
  /** 唯一标识符，格式为 `p{pageIndex}-{scale}-x{...}-y{...}-w{...}-h{...}` */
  id: string;
}

/** 单个文档的瓦片状态。visibleTiles 按页码索引存储该页的所有可见瓦片 */
export interface TilingDocumentState {
  /** 键为 0-based 页码，值为该页当前可见的瓦片列表（含 fallback 瓦片） */
  visibleTiles: Record<number, Tile[]>;
}

/** 瓦片插件的完整 Redux 状态树，按文档 ID 组织 */
export interface TilingState {
  /** 键为文档 ID，值为该文档的瓦片状态 */
  documents: Record<string, TilingDocumentState>;
}

/** 瓦片渲染事件，用于通知 React 层瓦片列表已更新 */
export interface TilingEvent {
  /** 文档 ID */
  documentId: string;
  /** 按页码索引的新瓦片列表 */
  tiles: Record<number, Tile[]>;
}

/** 单文档级别的瓦片操作接口（scope） */
export interface TilingScope {
  /** 渲染指定瓦片，返回一个 Task<Blob> */
  renderTile: (options: RenderTileOptions) => Task<Blob, PdfErrorReason>;
  /** 监听该文档的瓦片渲染状态变化 */
  onTileRendering: EventHook<Record<number, Tile[]>>;
}

/** 瓦片插件对外暴露的能力接口（capability） */
export interface TilingCapability {
  /** 渲染指定瓦片，可指定文档 ID（不指定则使用当前活动文档） */
  renderTile: (options: RenderTileOptions, documentId?: string) => Task<Blob, PdfErrorReason>;
  /** 获取指定文档的瓦片操作 scope */
  forDocument(documentId: string): TilingScope;
  /** 监听瓦片渲染状态变化的全局事件钩子 */
  onTileRendering: EventHook<TilingEvent>;
}

/** calculateTilesForPage 工具函数的参数 */
export interface CalculateTilesForPageOptions {
  /** 瓦片边长（像素） */
  tileSize: number;
  /** 瓦片间重叠像素数 */
  overlapPx: number;
  /** 可见区域外额外渲染的瓦片圈数 */
  extraRings: number;
  /** 当前文档缩放比例 */
  scale: number;
  /** 当前旋转角度（0-3，对应 0°/90°/180°/270°） */
  rotation: Rotation;
  /** PDF 页面对象，包含页面尺寸等信息 */
  page: PdfPageObject;
  /** 该页面在滚动视口中的可见性度量信息 */
  metric: PageVisibilityMetrics;
}

/** 渲染单个瓦片时的参数 */
export interface RenderTileOptions {
  /** 页码索引（0-based） */
  pageIndex: number;
  /** 待渲染的瓦片描述 */
  tile: Tile;
  /** 设备像素比，用于高分辨率渲染 */
  dpr: number;
  /**
   * 可选的图片类型覆盖，用于此瓦片的渲染。
   * 回退顺序：此选项 → tiling 插件配置 → render 插件默认值。
   */
  imageType?: ImageConversionTypes;
}
