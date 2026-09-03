/**
 * @file PdfEngine 编排器 — PDF 引擎的"智能"调度层
 *
 * 整体作用：
 *   PdfEngine 是 PDF 引擎架构中的中间编排层，位于上层业务逻辑
 *   和底层执行器（IPdfiumExecutor）之间。它提供：
 *
 *   1. **优先级任务调度**：通过 WorkerTaskQueue 确保渲染等实时操作
 *      优先于搜索等后台操作执行。
 *   2. **两阶段渲染管线**：将页面渲染分为"原始像素渲染"和"图像编码"
 *      两个阶段，渲染在引擎中执行，编码在独立的 Worker 池中并行完成。
 *   3. **批量操作编排**：将跨页操作（如搜索所有页面、获取所有注解）
 *      拆分为多个批次任务，使用 CompoundTask 聚合结果。
 *   4. **图像编码池**：利用 ImageEncoderWorkerPool 将像素数据编码为
 *      PNG/BMP 等格式，与渲染操作并行执行。
 *
 * 架构：
 *   上层 → PdfEngine（编排层）→ IPdfiumExecutor（执行器）
 *                                ↙            ↘
 *                       PdfiumNative      RemoteExecutor
 *                       （主线程/Worker）  （Worker 代理）
 */

import {
  BatchProgress,
  Logger,
  NoopLogger,
  PdfEngine as IPdfEngine,
  PdfDocumentObject,
  PdfPageObject,
  PdfTask,
  PdfErrorReason,
  PdfFileUrl,
  PdfFile,
  PdfOpenDocumentUrlOptions,
  PdfOpenDocumentBufferOptions,
  PdfMetadataObject,
  PdfBookmarksObject,
  PdfBookmarkObject,
  PdfRenderPageOptions,
  PdfRenderThumbnailOptions,
  PdfRenderPageAnnotationOptions,
  PdfAnnotationObject,
  PdfTextRectObject,
  PdfSearchAllPagesOptions,
  SearchAllPagesResult,
  PdfPageSearchProgress,
  PdfAnnotationsProgress,
  PdfAttachmentObject,
  PdfAddAttachmentParams,
  PdfDocumentJavaScriptActionObject,
  PdfWidgetAnnoObject,
  PdfWidgetAnnoField,
  PdfWidgetJavaScriptActionObject,
  FormFieldValue,
  PdfFlattenPageOptions,
  PdfPageFlattenResult,
  PdfRedactTextOptions,
  Rect,
  PageTextSlice,
  PdfGlyphObject,
  PdfPageGeometry,
  PdfPageTextRuns,
  PdfPrintOptions,
  PdfEngineFeature,
  PdfEngineOperation,
  PdfSignatureObject,
  AnnotationCreateContext,
  Task,
  PdfErrorCode,
  SearchResult,
  CompoundTask,
  ImageDataLike,
  IPdfiumExecutor,
  AnnotationAppearanceMap,
} from '../../models';
import { WorkerTaskQueue, Priority } from './task-queue';
import type { ImageDataConverter } from '../converters/types';

// 便捷重导出
export type { ImageDataConverter } from '../converters/types';
export type { ImageDataLike, IPdfiumExecutor, BatchProgress } from '../../models';

const LOG_SOURCE = 'PdfEngine';
const LOG_CATEGORY = 'Orchestrator';

/**
 * PdfEngine 配置选项
 *
 * @template T - 输出图像类型（通常为 Blob）
 * @property imageConverter - 图像编码器（将原始像素数据编码为目标格式）
 * @property fetcher       - 自定义 fetch 函数（用于下载远程 PDF 文件）
 * @property logger        - 日志记录器
 */
export interface PdfEngineOptions<T> {
  imageConverter: ImageDataConverter<T>;
  fetcher?: typeof fetch;
  logger?: Logger;
}

/**
 * PdfEngine 编排器
 *
 * 核心设计模式：
 *   - **两阶段渲染**：renderPage() = renderPageRaw()（引擎侧）+ encodeImage()（编码侧）
 *     渲染和编码可以流水线化，提高吞吐量。
 *   - **批量聚合**：getAllAnnotations() / searchAllPages() 将操作分块，
 *     每块创建一个子任务，通过 CompoundTask 聚合结果并转发进度。
 *   - **可中止性**：返回的 Task 支持 abort()，可取消排队中的任务。
 */
export class PdfEngine<T = Blob> implements IPdfEngine<T> {
  /** 底层执行器（PdfiumNative 或 RemoteExecutor） */
  private executor: IPdfiumExecutor;
  /** 优先级任务队列 */
  private workerQueue: WorkerTaskQueue;
  private logger: Logger;
  private options: PdfEngineOptions<T>;

  constructor(executor: IPdfiumExecutor, options: PdfEngineOptions<T>) {
    this.executor = executor;
    this.logger = options.logger ?? new NoopLogger();
    this.options = {
      imageConverter: options.imageConverter,
      // 默认使用全局 fetch（浏览器环境）或 undefined（Node.js 环境）
      fetcher:
        options.fetcher ??
        (typeof fetch !== 'undefined' ? (url, init) => fetch(url, init) : undefined),
      logger: this.logger,
    };

    // 创建任务队列，并发数为 3（提升吞吐，PDFium 单线程由 Worker 内部锁保证）
    this.workerQueue = new WorkerTaskQueue({
      concurrency: 3,
      autoStart: true,
      logger: this.logger,
    });

    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'PdfEngine orchestrator created');
  }

  /**
   * 将数组按指定大小分块（用于批量操作的拆分）
   *
   * @param items     - 要分块的数组
   * @param chunkSize - 每块的最大大小
   * @returns 二维数组，每个子数组长度不超过 chunkSize
   */
  private chunkArray<U>(items: U[], chunkSize: number): U[][] {
    const chunks: U[][] = [];
    for (let i = 0; i < items.length; i += chunkSize) {
      chunks.push(items.slice(i, i + chunkSize));
    }
    return chunks;
  }

  // ========== IPdfEngine Implementation ==========

  isSupport(feature: PdfEngineFeature): PdfTask<PdfEngineOperation[]> {
    const task = new Task<PdfEngineOperation[], PdfErrorReason>();
    // PDFium supports all features
    task.resolve([
      PdfEngineOperation.Create,
      PdfEngineOperation.Read,
      PdfEngineOperation.Update,
      PdfEngineOperation.Delete,
    ]);
    return task;
  }

  destroy(): PdfTask<boolean> {
    const task = new Task<boolean, PdfErrorReason>();
    try {
      this.executor.destroy();
      // Clean up image converter resources (e.g., encoder worker pool)
      this.options.imageConverter.destroy?.();
      task.resolve(true);
    } catch (error) {
      task.reject({ code: PdfErrorCode.Unknown, message: String(error) });
    }
    return task;
  }

  openDocumentUrl(
    file: PdfFileUrl,
    options?: PdfOpenDocumentUrlOptions,
  ): PdfTask<PdfDocumentObject> {
    const task = new Task<PdfDocumentObject, PdfErrorReason>();

    // Handle fetch in main thread (not worker!)
    (async () => {
      try {
        if (!this.options.fetcher) {
          throw new Error('Fetcher is not set');
        }

        const response = await this.options.fetcher(file.url, options?.requestOptions);
        const arrayBuf = await response.arrayBuffer();

        const pdfFile: PdfFile = {
          id: file.id,
          content: arrayBuf,
        };

        // Then open in worker - use wait() to properly propagate task errors
        this.openDocumentBuffer(pdfFile, {
          password: options?.password,
          normalizeRotation: options?.normalizeRotation,
        }).wait(
          (doc) => task.resolve(doc),
          (error) => task.fail(error),
        );
      } catch (error) {
        // This only catches fetch errors (network issues, etc.)
        task.reject({ code: PdfErrorCode.Unknown, message: String(error) });
      }
    })();

    return task;
  }

  openDocumentBuffer(
    file: PdfFile,
    options?: PdfOpenDocumentBufferOptions,
  ): PdfTask<PdfDocumentObject> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.openDocumentBuffer(file, options),
        meta: { docId: file.id, operation: 'openDocumentBuffer' },
      },
      { priority: Priority.CRITICAL },
    );
  }

  getMetadata(doc: PdfDocumentObject): PdfTask<PdfMetadataObject> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getMetadata(doc),
        meta: { docId: doc.id, operation: 'getMetadata' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  setMetadata(doc: PdfDocumentObject, metadata: Partial<PdfMetadataObject>): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.setMetadata(doc, metadata),
        meta: { docId: doc.id, operation: 'setMetadata' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getDocPermissions(doc: PdfDocumentObject): PdfTask<number> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getDocPermissions(doc),
        meta: { docId: doc.id, operation: 'getDocPermissions' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getDocUserPermissions(doc: PdfDocumentObject): PdfTask<number> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getDocUserPermissions(doc),
        meta: { docId: doc.id, operation: 'getDocUserPermissions' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getSignatures(doc: PdfDocumentObject): PdfTask<PdfSignatureObject[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getSignatures(doc),
        meta: { docId: doc.id, operation: 'getSignatures' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getBookmarks(doc: PdfDocumentObject): PdfTask<PdfBookmarksObject> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getBookmarks(doc),
        meta: { docId: doc.id, operation: 'getBookmarks' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  setBookmarks(doc: PdfDocumentObject, bookmarks: PdfBookmarkObject[]): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.setBookmarks(doc, bookmarks),
        meta: { docId: doc.id, operation: 'setBookmarks' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  deleteBookmarks(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.deleteBookmarks(doc),
        meta: { docId: doc.id, operation: 'deleteBookmarks' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  // ========== Rendering with Encoding ==========

  renderPage(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageOptions,
  ): PdfTask<T> {
    return this.renderWithEncoding(
      () => this.executor.renderPageRaw(doc, page, options),
      options,
      doc.id,
      page.index,
      Priority.CRITICAL,
    );
  }

  renderPageRect(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rect: Rect,
    options?: PdfRenderPageOptions,
  ): PdfTask<T> {
    return this.renderWithEncoding(
      () => this.executor.renderPageRect(doc, page, rect, options),
      options,
      doc.id,
      page.index,
      Priority.HIGH,
    );
  }

  // ========== Raw Rendering (no encoding) ==========

  renderPageRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageOptions,
  ): PdfTask<ImageDataLike> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.renderPageRaw(doc, page, options),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'renderPageRaw' },
      },
      { priority: Priority.HIGH },
    );
  }

  renderPageRectRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rect: Rect,
    options?: PdfRenderPageOptions,
  ): PdfTask<ImageDataLike> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.renderPageRect(doc, page, rect, options),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'renderPageRectRaw' },
      },
      { priority: Priority.HIGH },
    );
  }

  renderThumbnail(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderThumbnailOptions,
  ): PdfTask<T> {
    return this.renderWithEncoding(
      () => this.executor.renderThumbnailRaw(doc, page, options),
      options,
      doc.id,
      page.index,
      Priority.MEDIUM,
    );
  }

  renderPageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
    options?: PdfRenderPageAnnotationOptions,
  ): PdfTask<T> {
    return this.renderWithEncoding(
      () => this.executor.renderPageAnnotationRaw(doc, page, annotation, options),
      options,
      doc.id,
      page.index,
      Priority.MEDIUM,
    );
  }

  renderPageAnnotations(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageAnnotationOptions,
  ): PdfTask<AnnotationAppearanceMap<T>> {
    const resultTask = new Task<AnnotationAppearanceMap<T>, PdfErrorReason>();

    const renderHandle = this.workerQueue.enqueue(
      {
        execute: () => this.executor.renderPageAnnotationsRaw(doc, page, options),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'renderPageAnnotationsRaw' },
      },
      { priority: Priority.MEDIUM },
    );

    // Wire up abort: when resultTask is aborted, also abort the queue task
    const originalAbort = resultTask.abort.bind(resultTask);
    resultTask.abort = (reason) => {
      renderHandle.abort(reason);
      originalAbort(reason);
    };

    renderHandle.wait(
      (rawMap) => {
        if (resultTask.state.stage !== 0 /* Pending */) {
          return;
        }
        this.encodeAppearanceMap(rawMap, options, resultTask);
      },
      (error) => {
        if (resultTask.state.stage === 0 /* Pending */) {
          resultTask.fail(error);
        }
      },
    );

    return resultTask;
  }

  renderPageAnnotationsRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageAnnotationOptions,
  ): PdfTask<AnnotationAppearanceMap<ImageDataLike>> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.renderPageAnnotationsRaw(doc, page, options),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'renderPageAnnotationsRaw' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  /**
   * ★ 核心方法：两阶段渲染 + 编码管线
   *
   * 将渲染过程分为两个阶段，解耦执行和编码：
   *   阶段 1（渲染）：通过 WorkerTaskQueue 调度执行器渲染原始像素数据（ImageDataLike）
   *   阶段 2（编码）：通过 imageConverter 将像素数据编码为目标格式（如 PNG Blob）
   *
   * 这种两阶段设计的优势：
   *   - 渲染在引擎线程（Worker）中执行，编码在编码 Worker 池中并行执行
   *   - 两个阶段可以流水线化：渲染 A 页的同时编码 B 页
   *   - 调用者只看到最终编码结果，无需关心中间格式
   *
   * @param renderFn  - 渲染工厂函数，返回原始像素数据的 Task
   * @param options   - 渲染/编码选项（imageType、quality 等）
   * @param docId     - 文档 ID（用于日志和优先级排序）
   * @param pageIndex - 页面索引（用于日志和优先级排序）
   * @param priority  - 任务优先级（默认 CRITICAL）
   * @returns 最终编码结果的 Task
   */
  private renderWithEncoding(
    renderFn: () => PdfTask<ImageDataLike>,
    options?: PdfRenderPageOptions | PdfRenderThumbnailOptions | PdfRenderPageAnnotationOptions,
    docId?: string,
    pageIndex?: number,
    priority: Priority = Priority.CRITICAL,
  ): PdfTask<T> {
    const resultTask = new Task<T, PdfErrorReason>();

    // 阶段 1：将渲染任务加入优先级队列
    const renderHandle = this.workerQueue.enqueue(
      {
        execute: () => renderFn(),
        meta: { docId, pageIndex, operation: 'render' },
      },
      { priority },
    );

    // 连接中止逻辑：resultTask 被中止时，同步取消队列中的渲染任务
    const originalAbort = resultTask.abort.bind(resultTask);
    resultTask.abort = (reason) => {
      renderHandle.abort(reason); // 取消队列中的渲染任务
      originalAbort(reason);
    };

    // 渲染完成后进入阶段 2
    renderHandle.wait(
      (rawImageData) => {
        // 检查 resultTask 是否已被中止（避免编码已取消的结果）
        if (resultTask.state.stage !== 0 /* Pending */) {
          return;
        }
        // 阶段 2：将原始像素数据编码为目标格式
        this.encodeImage(rawImageData, options, resultTask);
      },
      (error) => {
        // 渲染失败时，将错误转发到最终 Task
        if (resultTask.state.stage === 0 /* Pending */) {
          resultTask.fail(error);
        }
      },
    );

    return resultTask;
  }

  /**
   * 将原始像素数据（ImageDataLike）编码为目标格式（Blob 等）
   *
   * 使用 imageConverter（通常由 ImageEncoderWorkerPool 提供）进行编码。
   * 编码在独立的 Worker 线程中执行，不阻塞渲染线程。
   *
   * @param rawImageData - 来自渲染阶段的原始像素数据
   * @param options      - 编码选项（imageType、quality）
   * @param resultTask   - 要 resolve/reject 的最终 Task
   */
  private encodeImage(
    rawImageData: ImageDataLike,
    options: any,
    resultTask: Task<T, PdfErrorReason>,
  ): void {
    const imageType = options?.imageType ?? 'image/png';
    const quality = options?.quality;

    // 将 ImageDataLike 转换为普通对象（避免 Transferable 传输问题）
    const plainImageData = {
      data: new Uint8ClampedArray(rawImageData.data),
      width: rawImageData.width,
      height: rawImageData.height,
    };

    // 通过 imageConverter 编码（可能在线程池中并行执行）
    this.options
      .imageConverter(() => plainImageData, imageType, quality)
      .then((result) => resultTask.resolve(result))
      .catch((error) => resultTask.reject({ code: PdfErrorCode.Unknown, message: String(error) }));
  }

  /**
   * 编码注解外观映射（Annotation Appearance Map）
   *
   * 与 encodeImage 类似，但处理的是多个注解的多个外观模式（normal/rollover/down）。
   * 所有外观图像并行编码，全部完成后 resolve。
   *
   * @param rawMap     - 原始外观映射：注解ID → { normal?, rollover?, down? } → { data, rect }
   * @param options    - 编码选项
   * @param resultTask - 要 resolve/reject 的最终 Task
   */
  private encodeAppearanceMap(
    rawMap: AnnotationAppearanceMap<ImageDataLike>,
    options: PdfRenderPageAnnotationOptions | undefined,
    resultTask: Task<AnnotationAppearanceMap<T>, PdfErrorReason>,
  ): void {
    const imageType = options?.imageType ?? 'image/png';
    const quality = options?.imageQuality;

    // 单个图像编码函数
    const convertImage = (rawImageData: ImageDataLike): Promise<T> => {
      const plainImageData = {
        data: new Uint8ClampedArray(rawImageData.data),
        width: rawImageData.width,
        height: rawImageData.height,
      };
      return this.options.imageConverter(() => plainImageData, imageType, quality);
    };

    const jobs: Promise<void>[] = [];
    const encodedMap: AnnotationAppearanceMap<T> = {};
    // PDF 注解的三种外观模式
    const modes: Array<'normal' | 'rollover' | 'down'> = ['normal', 'rollover', 'down'];

    // 遍历所有注解，为每个外观模式创建并行编码任务
    for (const [annotationId, appearances] of Object.entries(rawMap)) {
      const encodedAppearances: NonNullable<AnnotationAppearanceMap<T>[string]> = {};
      encodedMap[annotationId] = encodedAppearances;

      for (const mode of modes) {
        const appearance = appearances[mode];
        if (!appearance) continue;

        // 并行编码每个外观
        jobs.push(
          convertImage(appearance.data).then((encodedData) => {
            encodedAppearances[mode] = {
              data: encodedData,
              rect: appearance.rect,
            };
          }),
        );
      }
    }

    Promise.all(jobs)
      .then(() => {
        if (resultTask.state.stage === 0 /* Pending */) {
          resultTask.resolve(encodedMap);
        }
      })
      .catch((error) => {
        if (resultTask.state.stage === 0 /* Pending */) {
          resultTask.reject({ code: PdfErrorCode.Unknown, message: String(error) });
        }
      });
  }

  // ========== Annotations ==========

  getPageAnnotations(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfAnnotationObject[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getPageAnnotations(doc, page),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'getPageAnnotations' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  createPageAnnotation<A extends PdfAnnotationObject>(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: A,
    context?: AnnotationCreateContext<A>,
  ): PdfTask<string> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.createPageAnnotation(doc, page, annotation, context),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'createPageAnnotation' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  updatePageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
    options?: { regenerateAppearance?: boolean },
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.updatePageAnnotation(doc, page, annotation, options),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'updatePageAnnotation' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  removePageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.removePageAnnotation(doc, page, annotation),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'removePageAnnotation' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  /**
   * Get all annotations across all pages
   * Uses batched operations to reduce queue overhead
   */
  getAllAnnotations(
    doc: PdfDocumentObject,
  ): CompoundTask<Record<number, PdfAnnotationObject[]>, PdfErrorReason, PdfAnnotationsProgress> {
    // Chunk pages for batched processing
    const chunks = this.chunkArray(doc.pages, 500);

    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      `getAllAnnotations: ${doc.pages.length} pages in ${chunks.length} chunks`,
    );

    // Create compound task for result aggregation
    const compound = new CompoundTask<
      Record<number, PdfAnnotationObject[]>,
      PdfErrorReason,
      PdfAnnotationsProgress
    >({
      aggregate: (results) => Object.assign({}, ...results),
    });

    // Create one task per chunk and wire up progress forwarding
    chunks.forEach((chunkPages, chunkIndex) => {
      const batchTask = this.workerQueue.enqueue(
        {
          execute: () => this.executor.getAnnotationsBatch(doc, chunkPages),
          meta: { docId: doc.id, operation: 'getAnnotationsBatch', chunkSize: chunkPages.length },
        },
        { priority: Priority.LOW },
      );

      // Forward batch progress (per-page) to compound task
      batchTask.onProgress((batchProgress: BatchProgress<PdfAnnotationObject[]>) => {
        compound.progress({
          page: batchProgress.pageIndex,
          result: batchProgress.result,
        });
      });

      compound.addChild(batchTask, chunkIndex);
    });

    compound.finalize();
    return compound;
  }

  getPageTextRects(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfTextRectObject[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getPageTextRects(doc, page),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'getPageTextRects' },
      },
      {
        priority: Priority.MEDIUM,
      },
    );
  }

  // ========== Search ==========

  /**
   * Search across all pages
   * Uses batched operations to reduce queue overhead
   */
  searchAllPages(
    doc: PdfDocumentObject,
    keyword: string,
    options?: PdfSearchAllPagesOptions,
  ): PdfTask<SearchAllPagesResult, PdfPageSearchProgress> {
    const flags = Array.isArray(options?.flags)
      ? options.flags.reduce((acc, flag) => acc | flag, 0)
      : (options?.flags ?? 0);

    // Chunk pages for batched processing
    const chunks = this.chunkArray(doc.pages, 25);

    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      `searchAllPages: ${doc.pages.length} pages in ${chunks.length} chunks`,
    );

    // Create compound task for result aggregation
    const compound = new CompoundTask<SearchAllPagesResult, PdfErrorReason, PdfPageSearchProgress>({
      aggregate: (results) => {
        // Merge all batch results into a flat array
        const allResults = results.flatMap((batchResult: Record<number, SearchResult[]>) =>
          Object.values(batchResult).flat(),
        );
        return { results: allResults, total: allResults.length };
      },
    });

    // Create one task per chunk and wire up progress forwarding
    chunks.forEach((chunkPages, chunkIndex) => {
      const batchTask = this.workerQueue.enqueue(
        {
          execute: () => this.executor.searchBatch(doc, chunkPages, keyword, flags),
          meta: { docId: doc.id, operation: 'searchBatch', chunkSize: chunkPages.length },
        },
        { priority: Priority.LOW },
      );

      // Forward batch progress (per-page) to compound task
      batchTask.onProgress((batchProgress: BatchProgress<SearchResult[]>) => {
        compound.progress({
          page: batchProgress.pageIndex,
          results: batchProgress.result,
        });
      });

      compound.addChild(batchTask, chunkIndex);
    });

    compound.finalize();
    return compound;
  }

  // ========== Attachments ==========

  getAttachments(doc: PdfDocumentObject): PdfTask<PdfAttachmentObject[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getAttachments(doc),
        meta: { docId: doc.id, operation: 'getAttachments' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  addAttachment(doc: PdfDocumentObject, params: PdfAddAttachmentParams): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.addAttachment(doc, params),
        meta: { docId: doc.id, operation: 'addAttachment' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  removeAttachment(doc: PdfDocumentObject, attachment: PdfAttachmentObject): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.removeAttachment(doc, attachment),
        meta: { docId: doc.id, operation: 'removeAttachment' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  readAttachmentContent(
    doc: PdfDocumentObject,
    attachment: PdfAttachmentObject,
  ): PdfTask<ArrayBuffer> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.readAttachmentContent(doc, attachment),
        meta: { docId: doc.id, operation: 'readAttachmentContent' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  // ========== Forms ==========

  getDocumentJavaScriptActions(
    doc: PdfDocumentObject,
  ): PdfTask<PdfDocumentJavaScriptActionObject[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getDocumentJavaScriptActions(doc),
        meta: { docId: doc.id, operation: 'getDocumentJavaScriptActions' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getPageAnnoWidgets(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfWidgetAnnoObject[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getPageAnnoWidgets(doc, page),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'getPageAnnoWidgets' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getPageWidgetJavaScriptActions(
    doc: PdfDocumentObject,
    page: PdfPageObject,
  ): PdfTask<PdfWidgetJavaScriptActionObject[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getPageWidgetJavaScriptActions(doc, page),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'getPageWidgetJavaScriptActions' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  setFormFieldValue(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    value: FormFieldValue,
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.setFormFieldValue(doc, page, annotation, value),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'setFormFieldValue' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  setFormFieldState(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    field: PdfWidgetAnnoField,
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.setFormFieldState(doc, page, annotation, field),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'setFormFieldState' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  renameWidgetField(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    name: string,
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.renameWidgetField(doc, page, annotation, name),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'renameWidgetField' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  shareWidgetField(
    doc: PdfDocumentObject,
    sourcePage: PdfPageObject,
    sourceAnnotation: PdfWidgetAnnoObject,
    targetPage: PdfPageObject,
    targetAnnotation: PdfWidgetAnnoObject,
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () =>
          this.executor.shareWidgetField(
            doc,
            sourcePage,
            sourceAnnotation,
            targetPage,
            targetAnnotation,
          ),
        meta: {
          docId: doc.id,
          pageIndex: sourcePage.index,
          operation: 'shareWidgetField',
        },
      },
      { priority: Priority.MEDIUM },
    );
  }

  regenerateWidgetAppearances(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationIds: string[],
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.regenerateWidgetAppearances(doc, page, annotationIds),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'regenerateWidgetAppearances' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  flattenPage(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfFlattenPageOptions,
  ): PdfTask<PdfPageFlattenResult> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.flattenPage(doc, page, options),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'flattenPage' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  // ========== Text Operations ==========

  extractPages(doc: PdfDocumentObject, pageIndexes: number[]): PdfTask<ArrayBuffer> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.extractPages(doc, pageIndexes),
        meta: { docId: doc.id, pageIndexes: pageIndexes, operation: 'extractPages' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  createDocument(id: string): PdfTask<PdfDocumentObject> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.createDocument(id),
        meta: { docId: id, operation: 'createDocument' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  importPages(
    destDoc: PdfDocumentObject,
    srcDoc: PdfDocumentObject,
    srcPageIndices: number[],
    insertIndex?: number,
  ): PdfTask<PdfPageObject[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.importPages(destDoc, srcDoc, srcPageIndices, insertIndex),
        meta: { docId: destDoc.id, operation: 'importPages' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  deletePage(doc: PdfDocumentObject, pageIndex: number): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.deletePage(doc, pageIndex),
        meta: { docId: doc.id, operation: 'deletePage' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  extractText(doc: PdfDocumentObject, pageIndexes: number[]): PdfTask<string> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.extractText(doc, pageIndexes),
        meta: { docId: doc.id, pageIndexes: pageIndexes, operation: 'extractText' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  redactTextInRects(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rects: Rect[],
    options?: PdfRedactTextOptions,
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.redactTextInRects(doc, page, rects, options),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'redactTextInRects' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  applyRedaction(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.applyRedaction(doc, page, annotation),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'applyRedaction' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  applyAllRedactions(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.applyAllRedactions(doc, page),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'applyAllRedactions' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  flattenAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.flattenAnnotation(doc, page, annotation),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'flattenAnnotation' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  exportAnnotationAppearanceAsPdf(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<ArrayBuffer> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.exportAnnotationAppearanceAsPdf(doc, page, annotation),
        meta: {
          docId: doc.id,
          pageIndex: page.index,
          operation: 'exportAnnotationAppearanceAsPdf',
        },
      },
      { priority: Priority.MEDIUM },
    );
  }

  exportAnnotationsAppearanceAsPdf(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotations: PdfAnnotationObject[],
  ): PdfTask<ArrayBuffer> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.exportAnnotationsAppearanceAsPdf(doc, page, annotations),
        meta: {
          docId: doc.id,
          pageIndex: page.index,
          operation: 'exportAnnotationsAppearanceAsPdf',
        },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getTextSlices(doc: PdfDocumentObject, slices: PageTextSlice[]): PdfTask<string[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getTextSlices(doc, slices),
        meta: { docId: doc.id, slices: slices, operation: 'getTextSlices' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getPageGlyphs(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfGlyphObject[]> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getPageGlyphs(doc, page),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'getPageGlyphs' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getPageGeometry(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfPageGeometry> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getPageGeometry(doc, page),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'getPageGeometry' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  getPageTextRuns(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfPageTextRuns> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.getPageTextRuns(doc, page),
        meta: { docId: doc.id, pageIndex: page.index, operation: 'getPageTextRuns' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  // ========== Document Operations ==========

  merge(files: PdfFile[]): PdfTask<PdfFile> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.merge(files),
        meta: { docId: files.map((file) => file.id).join(','), operation: 'merge' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  mergePages(mergeConfigs: Array<{ docId: string; pageIndices: number[] }>): PdfTask<PdfFile> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.mergePages(mergeConfigs),
        meta: {
          docId: mergeConfigs.map((config) => config.docId).join(','),
          operation: 'mergePages',
        },
      },
      { priority: Priority.MEDIUM },
    );
  }

  preparePrintDocument(doc: PdfDocumentObject, options?: PdfPrintOptions): PdfTask<ArrayBuffer> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.preparePrintDocument(doc, options),
        meta: { docId: doc.id, operation: 'preparePrintDocument' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  saveAsCopy(doc: PdfDocumentObject): PdfTask<ArrayBuffer> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.saveAsCopy(doc),
        meta: { docId: doc.id, operation: 'saveAsCopy' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  closeDocument(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.closeDocument(doc),
        meta: { docId: doc.id, operation: 'closeDocument' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  closeAllDocuments(): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.closeAllDocuments(),
        meta: { operation: 'closeAllDocuments' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.setDocumentEncryption}
   */
  setDocumentEncryption(
    doc: PdfDocumentObject,
    userPassword: string,
    ownerPassword: string,
    allowedFlags: number,
  ): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () =>
          this.executor.setDocumentEncryption(doc, userPassword, ownerPassword, allowedFlags),
        meta: { docId: doc.id, operation: 'setDocumentEncryption' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.removeEncryption}
   */
  removeEncryption(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.removeEncryption(doc),
        meta: { docId: doc.id, operation: 'removeEncryption' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.unlockOwnerPermissions}
   */
  unlockOwnerPermissions(doc: PdfDocumentObject, ownerPassword: string): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.unlockOwnerPermissions(doc, ownerPassword),
        meta: { docId: doc.id, operation: 'unlockOwnerPermissions' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.isEncrypted}
   */
  isEncrypted(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.isEncrypted(doc),
        meta: { docId: doc.id, operation: 'isEncrypted' },
      },
      { priority: Priority.MEDIUM },
    );
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.isOwnerUnlocked}
   */
  isOwnerUnlocked(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.workerQueue.enqueue(
      {
        execute: () => this.executor.isOwnerUnlocked(doc),
        meta: { docId: doc.id, operation: 'isOwnerUnlocked' },
      },
      { priority: Priority.MEDIUM },
    );
  }
}
