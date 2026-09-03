/**
 * @file 瓦片渲染插件 — 插件主类
 *
 * TilingPlugin 是瓦片渲染系统的核心控制器。它继承自 BasePlugin，协调以下职责：
 *
 * 1. 监听滚动事件，计算当前视口内需要显示的瓦片
 * 2. 响应缩放/旋转变化，重新计算瓦片网格
 * 3. 通过 RenderCapability 委托实际的 PDF 页面渲染
 * 4. 管理瓦片的生命周期状态（queued → rendering → ready）
 * 5. 通过事件发射器通知 React 层瓦片列表的更新
 *
 * 依赖关系：
 * - render 插件：提供页面区域渲染能力
 * - scroll 插件：提供滚动事件和视口可见性度量
 * - viewport 插件：提供视口尺寸信息
 *
 * 性能设计：
 * - 滚动事件以 50ms 节流（trailing 模式）处理，避免高频计算
 * - 仅计算可见区域内的瓦片，不渲染屏幕外的内容
 */

import {
  BasePlugin,
  createBehaviorEmitter,
  Listener,
  PluginRegistry,
  REFRESH_PAGES,
  RefreshPagesAction,
} from '../core';
import { ignore } from '../models';
import { RenderCapability, RenderPlugin } from '../plugin-render';
import {
  ScrollCapability,
  ScrollMetrics,
  ScrollPlugin,
  ScrollEvent,
} from '../plugin-scroll';
import { ViewportCapability, ViewportPlugin } from '../plugin-viewport';

import { initTilingState, cleanupTilingState, markTileStatus, updateVisibleTiles } from './actions';
import { initialTilingDocumentState } from './reducer';
import {
  TilingPluginConfig,
  TilingCapability,
  Tile,
  RenderTileOptions,
  TilingState,
  TilingEvent,
  TilingScope,
} from './types';
import { calculateTilesForPage } from './utils';

/**
 * 瓦片渲染插件主类。
 *
 * 继承自 BasePlugin，通过插件注册表获取 render/scroll/viewport 三个依赖插件的能力，
 * 并根据滚动和视口变化动态计算和更新可见瓦片。
 *
 * 对外通过 TilingCapability 接口暴露 renderTile 和 onTileRendering 两个核心能力。
 */
export class TilingPlugin extends BasePlugin<TilingPluginConfig, TilingCapability, TilingState> {
  /** 插件静态 ID，用于在插件系统中标识 */
  static readonly id = 'tiling' as const;

  /**
   * 瓦片渲染事件发射器。
   * 当瓦片列表发生变化时（通过 onStoreUpdated 检测），向所有订阅者广播事件。
   * React 组件通过此事件获取最新的瓦片列表。
   */
  private readonly tileRendering$ = createBehaviorEmitter<TilingEvent>();

  /** 插件配置（瓦片大小、重叠像素、额外圈数等） */
  private config: TilingPluginConfig;
  /** 渲染插件能力，用于委托 PDF 页面区域的实际渲染 */
  private renderCapability: RenderCapability;
  /** 滚动插件能力，用于获取滚动事件和可见性度量 */
  private scrollCapability: ScrollCapability;
  /** 视口插件能力，用于获取视口尺寸信息 */
  private viewportCapability: ViewportCapability;

  /**
   * 构造函数。
   *
   * @param id - 插件实例 ID
   * @param registry - 插件注册表，用于获取依赖插件
   * @param config - 插件配置参数
   */
  constructor(id: string, registry: PluginRegistry, config: TilingPluginConfig) {
    super(id, registry);

    this.config = config;

    // 获取三个依赖插件的能力接口
    this.renderCapability = this.registry.getPlugin<RenderPlugin>('render')!.provides();
    this.scrollCapability = this.registry.getPlugin<ScrollPlugin>('scroll')!.provides();
    this.viewportCapability = this.registry.getPlugin<ViewportPlugin>('viewport')!.provides();

    // 订阅滚动事件，节流计算可见瓦片
    // throttle trailing 模式：在 50ms 的静默期后触发一次，确保最终位置一定被处理
    this.scrollCapability.onScroll(
      (event: ScrollEvent) => this.calculateVisibleTiles(event.documentId, event.metrics),
      {
        mode: 'throttle',
        wait: 50,
        throttleMode: 'trailing',
      },
    );

    // 监听页面刷新 action（如标注更新后需要重新渲染）
    this.coreStore.onAction(REFRESH_PAGES, (action) => this.recalculateTiles(action.payload));
  }

  /* ======================== 文档生命周期钩子 ======================== */

  /**
   * 文档开始加载时的回调。
   * 为新文档初始化空的瓦片状态。
   * @param documentId - 正在加载的文档 ID
   */
  protected override onDocumentLoadingStarted(documentId: string): void {
    this.dispatch(initTilingState(documentId, initialTilingDocumentState));
  }

  /**
   * 文档关闭时的回调。
   * 清理该文档的瓦片状态，释放资源。
   * @param documentId - 已关闭的文档 ID
   */
  protected override onDocumentClosed(documentId: string): void {
    this.dispatch(cleanupTilingState(documentId));
  }

  /**
   * 文档缩放比例变化时的回调。
   * 需要重新计算瓦片网格，因为缩放改变了瓦片的屏幕尺寸和可见范围。
   * @param documentId - 发生缩放变化的文档 ID
   */
  protected override onScaleChanged(documentId: string): void {
    this.recalculateTilesForDocument(documentId);
  }

  /**
   * 文档旋转角度变化时的回调。
   * 需要重新计算瓦片网格，因为旋转改变了页面的宽高和可见区域的映射。
   * @param documentId - 发生旋转变化的文档 ID
   */
  protected override onRotationChanged(documentId: string): void {
    this.recalculateTilesForDocument(documentId);
  }

  /* ======================== 瓦片计算核心逻辑 ======================== */

  /**
   * 根据当前滚动和视口状态重新计算指定文档的可见瓦片。
   * 用于缩放/旋转变化等需要完整重算的场景。
   *
   * @param documentId - 文档 ID
   */
  private recalculateTilesForDocument(documentId: string): void {
    const scrollScope = this.scrollCapability.forDocument(documentId);
    const viewportScope = this.viewportCapability.forDocument(documentId);
    // 合并滚动和视口度量，得到完整的可见性信息
    const metrics = scrollScope.getMetrics(viewportScope.getMetrics());
    this.calculateVisibleTiles(documentId, metrics);
  }

  /**
   * 重新计算指定页面的瓦片（用于页面刷新场景，如标注更新）。
   *
   * 与 calculateVisibleTiles 不同，此方法会为瓦片 ID 添加时间戳后缀，
   * 强制触发重新渲染（即使瓦片位置和参数未变化）。
   *
   * @param payload - 包含文档 ID 和需要刷新的页面索引列表
   */
  async recalculateTiles(payload: RefreshPagesAction['payload']): Promise<void> {
    const { documentId, pageIndexes } = payload;
    const coreDoc = this.getCoreDocument(documentId);
    if (!coreDoc || !coreDoc.document) return;

    const scrollScope = this.scrollCapability.forDocument(documentId);
    const viewportScope = this.viewportCapability.forDocument(documentId);
    const currentMetrics = scrollScope.getMetrics(viewportScope.getMetrics());

    // 为刷新的页面重新计算瓦片，并附加时间戳以强制重新渲染
    const refreshedTiles: Record<number, Tile[]> = {};
    const refreshTimestamp = Date.now();
    const scale = coreDoc.scale;

    for (const pageIndex of pageIndexes) {
      // 从可见性度量中查找该页面的滚动信息
      const metric = currentMetrics.pageVisibilityMetrics.find(
        (m) => m.pageNumber === pageIndex + 1,
      );
      if (!metric) continue;

      const page = coreDoc.document.pages[pageIndex];
      if (!page) continue;

      // 计算有效旋转：页面自身旋转 + 文档全局旋转
      const effectiveRotation = ((page.rotation ?? 0) + coreDoc.rotation) % 4;

      refreshedTiles[pageIndex] = calculateTilesForPage({
        page,
        metric,
        scale,
        rotation: effectiveRotation,
        tileSize: this.config.tileSize,
        overlapPx: this.config.overlapPx,
        extraRings: this.config.extraRings,
      }).map((tile) => ({
        ...tile,
        // 在瓦片 ID 后追加刷新时间戳，使其成为新的唯一 ID
        // 这样即使位置和参数相同，也会被视为新瓦片并触发重新渲染
        id: `${tile.id}-r${refreshTimestamp}`,
      }));
    }

    if (Object.keys(refreshedTiles).length > 0) {
      this.dispatch(updateVisibleTiles(documentId, refreshedTiles));
    }
  }

  /**
   * 插件异步初始化钩子（当前无额外初始化逻辑）。
   * 依赖插件的能力已在构造函数中获取。
   */
  async initialize(): Promise<void> {
    // Fetch dependencies from the registry if needed
  }

  /**
   * 根据滚动度量计算当前可见区域的瓦片。
   * 这是滚动事件处理的核心方法。
   *
   * 算法流程：
   * 1. 获取文档的当前缩放比例
   * 2. 遍历所有可见页面的滚动度量
   * 3. 为每个可见页面计算瓦片网格
   * 4. 通过 dispatch 更新 store 中的瓦片列表
   *
   * @param documentId - 文档 ID
   * @param scrollMetrics - 滚动度量信息，包含每个页面的可见性数据
   */
  private calculateVisibleTiles(documentId: string, scrollMetrics: ScrollMetrics): void {
    const coreDoc = this.getCoreDocument(documentId);
    if (!coreDoc || !coreDoc.document) return;

    const scale = coreDoc.scale;
    const visibleTiles: { [pageIndex: number]: Tile[] } = {};

    for (const scrollMetric of scrollMetrics.pageVisibilityMetrics) {
      // 滚动度量中的 pageNumber 是 1-based，转换为 0-based 索引
      const pageIndex = scrollMetric.pageNumber - 1;
      const page = coreDoc.document.pages[pageIndex];
      if (!page) continue;

      // 计算有效旋转：页面自身旋转 + 文档全局旋转
      const effectiveRotation = ((page.rotation ?? 0) + coreDoc.rotation) % 4;

      // 调用工具函数计算该页面的瓦片网格
      const tiles = calculateTilesForPage({
        page,
        metric: scrollMetric,
        scale,
        rotation: effectiveRotation,
        tileSize: this.config.tileSize,
        overlapPx: this.config.overlapPx,
        extraRings: this.config.extraRings,
      });

      visibleTiles[pageIndex] = tiles;
    }

    this.dispatch(updateVisibleTiles(documentId, visibleTiles));
  }

  /* ======================== 状态变化通知 ======================== */

  /**
   * Store 更新回调。当瓦片状态发生变化时，通过事件发射器通知订阅者。
   *
   * 此方法在每次 reducer 处理完 action 后被调用，用于将状态变化
   * 从 Redux 风格的 store 桥接到事件驱动模式，供 React 层订阅。
   *
   * @param prevState - 更新前的状态
   * @param newState - 更新后的状态
   */
  override onStoreUpdated(prevState: TilingState, newState: TilingState): void {
    for (const documentId in newState.documents) {
      const prevDoc = prevState.documents[documentId];
      const newDoc = newState.documents[documentId];
      // 仅在实际发生变化时才发射事件（引用比较）
      if (prevDoc !== newDoc) {
        this.tileRendering$.emit({ documentId, tiles: newDoc.visibleTiles });
      }
    }
  }

  /* ======================== 能力接口构建 ======================== */

  /**
   * 构建插件对外暴露的能力接口。
   *
   * @returns TilingCapability 对象，包含：
   *   - renderTile: 渲染单个瓦片
   *   - forDocument: 获取文档级别的瓦片操作 scope
   *   - onTileRendering: 监听瓦片渲染状态变化的全局事件
   */
  protected buildCapability(): TilingCapability {
    return {
      renderTile: this.renderTile.bind(this),
      forDocument: this.createTilingScope.bind(this),
      onTileRendering: this.tileRendering$.on,
    };
  }

  /**
   * 创建指定文档的瓦片操作 scope。
   *
   * scope 将全局操作限定到特定文档，使消费者无需每次传递 documentId。
   *
   * @param documentId - 文档 ID
   * @returns TilingScope 对象，包含文档级别的 renderTile 和 onTileRendering
   */
  private createTilingScope(documentId: string): TilingScope {
    return {
      // 绑定文档 ID 的 renderTile 方法
      renderTile: (options) => this.renderTile(options, documentId),
      // 过滤仅属于该文档的事件
      onTileRendering: (listener: Listener<Record<number, Tile[]>>) =>
        this.tileRendering$.on((event) => {
          if (event.documentId === documentId) listener(event.tiles);
        }),
    };
  }

  /**
   * 渲染单个瓦片。
   *
   * 工作流程：
   * 1. 将瓦片状态标记为 'rendering'
   * 2. 通过 render 插件的 renderPageRect 方法渲染指定区域
   * 3. 渲染完成后将状态标记为 'ready'
   *
   * @param options - 渲染参数（页码、瓦片、DPR、图片类型等）
   * @param documentId - 可选的文档 ID，不指定则使用当前活动文档
   * @returns Task<Blob> 异步渲染任务，完成后返回图片 Blob
   * @throws 如果 render capability 不可用
   */
  private renderTile(options: RenderTileOptions, documentId?: string) {
    const id = documentId ?? this.getActiveDocumentId();
    if (!this.renderCapability) {
      throw new Error('Render capability not available.');
    }

    // 标记瓦片状态为渲染中
    this.dispatch(markTileStatus(id, options.pageIndex, options.tile.id, 'rendering'));

    // 委托 render 插件渲染页面指定矩形区域
    // 渲染分辨率下限 1.0：fit-width 等场景下 scale 可能 < 1，此时按原 scale 渲染会让
    // 物理像素不足，文字看起来发虚。强制至少以原生密度渲染，显示时由浏览器下采样到 CSS 尺寸。
    const task = this.renderCapability.forDocument(id).renderPageRect({
      pageIndex: options.pageIndex,
      rect: options.tile.pageRect,
      options: {
        scaleFactor: Math.max(1, options.tile.srcScale),
        dpr: options.dpr,
        // 图片类型优先级：调用者指定 > 插件配置默认值 > render 插件默认值
        ...(options.imageType || this.config.defaultImageType
          ? { imageType: options.imageType ?? this.config.defaultImageType }
          : {}),
      },
    });

    // 渲染成功后标记为 ready，失败时忽略（ignore）
    task.wait(() => {
      this.dispatch(markTileStatus(id, options.pageIndex, options.tile.id, 'ready'));
    }, ignore);

    return task;
  }
}
