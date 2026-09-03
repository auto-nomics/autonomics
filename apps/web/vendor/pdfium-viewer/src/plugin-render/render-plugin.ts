/**
 * @file 渲染插件核心实现
 *
 * 本文件实现了 RenderPlugin 类，它是 PDF 渲染插件的核心，负责：
 * 1. 将 PDF 页面渲染为图像（Blob 或原始像素数据）
 * 2. 支持整页渲染和矩形区域渲染两种模式
 * 3. 提供文档级作用域绑定（forDocument），简化多文档场景调用
 *
 * 设计决策：
 * - 页面刷新追踪（refresh tracking）不在本插件中管理，而是由核心状态
 *   DocumentState.pageRefreshVersions 统一维护。这样任何插件都可以观测到
 *   页面刷新事件，而不仅仅是渲染插件本身，实现了关注点分离。
 * - 配置项合并采用三级优先级：调用方传入 > 插件全局配置 > 硬编码默认值。
 * - 本插件不重写 onDocumentLoadingStarted / onDocumentClosed 生命周期钩子，
 *   因为渲染操作是无状态的——每次调用直接读取当前核心状态。
 */

import { BasePlugin, PluginRegistry } from '../core';
import {
  RenderCapability,
  RenderPageOptions,
  RenderPageRectOptions,
  RenderPluginConfig,
  RenderScope,
} from './types';

/**
 * RenderPlugin – PDF 渲染插件
 *
 * 继承自 BasePlugin，通过 buildCapability() 方法将渲染能力暴露给插件系统。
 * 支持四种渲染模式：
 *   - renderPage       → 整页渲染，返回编码后的 Blob
 *   - renderPageRect   → 矩形区域渲染，返回编码后的 Blob
 *   - renderPageRaw    → 整页渲染，返回原始像素数据（跳过图像编码）
 *   - renderPageRectRaw → 矩形区域渲染，返回原始像素数据
 *
 * @template RenderPluginConfig  插件配置类型
 * @template RenderCapability    插件对外暴露的能力类型
 */
export class RenderPlugin extends BasePlugin<RenderPluginConfig, RenderCapability> {
  /** 插件静态标识，用于在插件注册表中查找和类型推断 */
  static readonly id = 'render' as const;

  /** 插件运行时配置（注册时传入的配置项） */
  private config: RenderPluginConfig;

  /**
   * 构造函数。
   * @param id       插件实例标识（通常为 'render'）
   * @param registry 插件注册表，用于与其他插件交互
   * @param config   插件配置项
   */
  constructor(id: string, registry: PluginRegistry, config: RenderPluginConfig) {
    super(id, registry);
    this.config = config;
  }

  // 注：本插件不需要实现 onDocumentLoadingStarted 或 onDocumentClosed，
  // 因为渲染操作直接从 coreState 读取文档数据，无需额外的加载/卸载逻辑。

  /**
   * 构建并返回渲染能力对象。
   * 该对象将被插件系统缓存，并通过 useCapability() 等 hook 暴露给 UI 层。
   *
   * @returns RenderCapability 实例，包含所有渲染方法和文档作用域工厂
   */
  protected buildCapability(): RenderCapability {
    return {
      // 直接操作——基于当前活动文档执行渲染
      renderPage: (options: RenderPageOptions) => this.renderPage(options),
      renderPageRect: (options: RenderPageRectOptions) => this.renderPageRect(options),
      renderPageRaw: (options: RenderPageOptions) => this.renderPageRaw(options),
      renderPageRectRaw: (options: RenderPageRectOptions) => this.renderPageRectRaw(options),

      // 文档作用域工厂——绑定到指定文档后执行渲染
      forDocument: (documentId: string) => this.createRenderScope(documentId),
    };
  }

  // ─────────────────────────────────────────────────────────
  // 文档作用域
  // ─────────────────────────────────────────────────────────

  /**
   * 创建一个绑定到指定文档的渲染作用域。
   *
   * 返回的 RenderScope 对象中，所有方法调用都自动携带指定的 documentId，
   * 调用方无需在每个方法中重复传递。
   *
   * @param documentId 目标文档的唯一标识
   * @returns 绑定到指定文档的 RenderScope 实例
   */
  private createRenderScope(documentId: string): RenderScope {
    return {
      renderPage: (options: RenderPageOptions) => this.renderPage(options, documentId),
      renderPageRect: (options: RenderPageRectOptions) => this.renderPageRect(options, documentId),
      renderPageRaw: (options: RenderPageOptions) => this.renderPageRaw(options, documentId),
      renderPageRectRaw: (options: RenderPageRectOptions) =>
        this.renderPageRectRaw(options, documentId),
    };
  }

  // ─────────────────────────────────────────────────────────
  // 核心渲染操作
  // ─────────────────────────────────────────────────────────

  /**
   * 渲染指定文档的完整页面，返回编码后的图像 Blob。
   *
   * 合并配置的优先级：调用方传入 > 插件全局配置 > 硬编码默认值。
   *
   * @param param0      包含 pageIndex 和可选渲染 options 的对象
   * @param documentId  可选的文档 ID，若未提供则使用当前活动文档
   * @returns 异步任务，成功时解析为图像 Blob
   * @throws 若文档未加载或页面不存在则抛出错误
   */
  private renderPage({ pageIndex, options }: RenderPageOptions, documentId?: string) {
    // 确定目标文档：优先使用显式传入的 documentId，否则回退到当前活动文档
    const id = documentId ?? this.getActiveDocumentId();
    const coreDoc = this.coreState.core.documents[id];

    // 前置检查：文档必须已加载
    if (!coreDoc?.document) {
      throw new Error(`Document ${id} not loaded`);
    }

    // 在文档的页面列表中查找目标页面
    const page = coreDoc.document.pages.find((p) => p.index === pageIndex);
    if (!page) {
      throw new Error(`Page ${pageIndex} not found in document ${id}`);
    }

    // 合并渲染选项：调用方 > 插件配置 > 默认值（三级优先级）
    const mergedOptions = {
      ...(options ?? {}),
      withForms: options?.withForms ?? this.config.withForms ?? false,
      withAnnotations: options?.withAnnotations ?? this.config.withAnnotations ?? false,
      imageType: options?.imageType ?? this.config.defaultImageType ?? 'image/png',
      imageQuality: options?.imageQuality ?? this.config.defaultImageQuality ?? 0.92,
    };

    // 委托给底层渲染引擎执行实际渲染
    return this.engine.renderPage(coreDoc.document, page, mergedOptions);
  }

  /**
   * 渲染指定文档中某页面的矩形区域，返回编码后的图像 Blob。
   *
   * @param param0      包含 pageIndex、rect 和可选渲染 options 的对象
   * @param documentId  可选的文档 ID，若未提供则使用当前活动文档
   * @returns 异步任务，成功时解析为图像 Blob
   * @throws 若文档未加载或页面不存在则抛出错误
   */
  private renderPageRect({ pageIndex, rect, options }: RenderPageRectOptions, documentId?: string) {
    const id = documentId ?? this.getActiveDocumentId();
    const coreDoc = this.coreState.core.documents[id];

    if (!coreDoc?.document) {
      throw new Error(`Document ${id} not loaded`);
    }

    const page = coreDoc.document.pages.find((p) => p.index === pageIndex);
    if (!page) {
      throw new Error(`Page ${pageIndex} not found in document ${id}`);
    }

    // 合并渲染选项（同 renderPage）
    const mergedOptions = {
      ...(options ?? {}),
      withForms: options?.withForms ?? this.config.withForms ?? false,
      withAnnotations: options?.withAnnotations ?? this.config.withAnnotations ?? false,
      imageType: options?.imageType ?? this.config.defaultImageType ?? 'image/png',
      imageQuality: options?.imageQuality ?? this.config.defaultImageQuality ?? 0.92,
    };

    // 委托给底层渲染引擎执行矩形区域渲染
    return this.engine.renderPageRect(coreDoc.document, page, rect, mergedOptions);
  }

  // ─────────────────────────────────────────────────────────
  // 原始渲染（返回 ImageDataLike，跳过图像编码）
  // ─────────────────────────────────────────────────────────

  /**
   * 渲染指定文档的完整页面，返回原始像素数据（未经图像编码）。
   *
   * 适用场景：需要对像素数据进行二次处理（如文字选择高亮、图像分析等），
   * 避免编码/解码的性能开销。
   *
   * 注意：Raw 模式不包含 imageType / imageQuality 选项，因为不进行编码。
   *
   * @param param0      包含 pageIndex 和可选渲染 options 的对象
   * @param documentId  可选的文档 ID
   * @returns 异步任务，成功时解析为 ImageDataLike（原始像素数据）
   */
  private renderPageRaw({ pageIndex, options }: RenderPageOptions, documentId?: string) {
    const id = documentId ?? this.getActiveDocumentId();
    const coreDoc = this.coreState.core.documents[id];

    if (!coreDoc?.document) {
      throw new Error(`Document ${id} not loaded`);
    }

    const page = coreDoc.document.pages.find((p) => p.index === pageIndex);
    if (!page) {
      throw new Error(`Page ${pageIndex} not found in document ${id}`);
    }

    // Raw 渲染只需合并表单和注解选项，无需图像编码参数
    const mergedOptions = {
      ...(options ?? {}),
      withForms: options?.withForms ?? this.config.withForms ?? false,
      withAnnotations: options?.withAnnotations ?? this.config.withAnnotations ?? false,
    };

    return this.engine.renderPageRaw(coreDoc.document, page, mergedOptions);
  }

  /**
   * 渲染指定文档中某页面的矩形区域，返回原始像素数据（未经图像编码）。
   *
   * @param param0      包含 pageIndex、rect 和可选渲染 options 的对象
   * @param documentId  可选的文档 ID
   * @returns 异步任务，成功时解析为 ImageDataLike（原始像素数据）
   */
  private renderPageRectRaw(
    { pageIndex, rect, options }: RenderPageRectOptions,
    documentId?: string,
  ) {
    const id = documentId ?? this.getActiveDocumentId();
    const coreDoc = this.coreState.core.documents[id];

    if (!coreDoc?.document) {
      throw new Error(`Document ${id} not loaded`);
    }

    const page = coreDoc.document.pages.find((p) => p.index === pageIndex);
    if (!page) {
      throw new Error(`Page ${pageIndex} not found in document ${id}`);
    }

    const mergedOptions = {
      ...(options ?? {}),
      withForms: options?.withForms ?? this.config.withForms ?? false,
      withAnnotations: options?.withAnnotations ?? this.config.withAnnotations ?? false,
    };

    return this.engine.renderPageRectRaw(coreDoc.document, page, rect, mergedOptions);
  }

  // ─────────────────────────────────────────────────────────
  // 生命周期
  // ─────────────────────────────────────────────────────────

  /**
   * 插件初始化钩子。
   * 当前为空实现，仅记录初始化日志。
   *
   * @param _config 插件配置（与构造函数中传入的相同）
   */
  async initialize(_config: RenderPluginConfig): Promise<void> {
    this.logger.info('RenderPlugin', 'Initialize', 'Render plugin initialized');
  }

  /**
   * 插件销毁钩子。
   * 调用父类销毁逻辑以清理注册表引用等资源。
   */
  async destroy(): Promise<void> {
    super.destroy();
  }
}
