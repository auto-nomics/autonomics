/**
 * @file RemoteExecutor — 主线程端的 Worker 代理执行器
 *
 * 整体作用：
 *   RemoteExecutor 是 IPdfiumExecutor 接口的 Worker 代理实现。
 *   它在主线程中运行，将所有 PDF 操作调用通过 postMessage 转发到
 *   Web Worker 中的 PdfiumNativeRunner，由后者实际执行。
 *
 * 为什么要用 Worker？
 *   PDFium WASM 的渲染操作是 CPU 密集型的，如果在主线程执行会导致
 *   UI 卡顿。将 PDFium 放在 Worker 线程中可以保持 UI 响应性。
 *
 * 通信模型：
 *   主线程 (RemoteExecutor) ←→ postMessage ←→ Worker (PdfiumNativeRunner)
 *
 *   - send() 发送请求 → pendingRequests Map 中等待响应
 *   - handleMessage() 接收响应 → 根据 id 匹配并 resolve/reject Task
 *   - Worker 初始化前，所有请求排队等待 readyTask resolve
 *
 * 关键设计：
 *   - readyTask 机制：构造时创建，Worker 发送 'ready' 消息后 resolve，
 *     确保不会在 WASM 未初始化时发送执行请求。
 *   - Task 桥接：将 Worker 的 postMessage 响应转换为 TypeScript Task
 *     的 resolve/reject/progress 事件。
 */

import {
  BatchProgress,
  Logger,
  NoopLogger,
  PdfDocumentObject,
  PdfPageObject,
  PdfTask,
  PdfErrorReason,
  PdfFile,
  PdfOpenDocumentBufferOptions,
  PdfMetadataObject,
  PdfBookmarksObject,
  PdfBookmarkObject,
  PdfRenderPageOptions,
  PdfRenderThumbnailOptions,
  PdfRenderPageAnnotationOptions,
  PdfAnnotationObject,
  PdfTextRectObject,
  PdfAttachmentObject,
  PdfAddAttachmentParams,
  PdfWidgetAnnoObject,
  PdfWidgetAnnoField,
  PdfDocumentJavaScriptActionObject,
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
  PdfSignatureObject,
  AnnotationCreateContext,
  Task,
  TaskError,
  PdfErrorCode,
  SearchResult,
  serializeLogger,
  IPdfiumExecutor,
  ImageDataLike,
  AnnotationAppearanceMap,
} from '../../models';
import type { WorkerRequest, WorkerResponse } from './pdfium-native-runner';
import type { FontFallbackConfig } from '../pdfium/font-fallback';

/**
 * RemoteExecutor 的创建选项
 *
 * @property wasmUrl     - PDFium WASM 文件的 URL（必填，Worker 会通过 fetch 下载）
 * @property logger      - 日志记录器（会被序列化传给 Worker）
 * @property fontFallback - 字体回退配置（传给 Worker 中的 FontFallbackManager）
 */
export interface RemoteExecutorOptions {
  wasmUrl: string;
  logger?: Logger;
  fontFallback?: FontFallbackConfig;
}

const LOG_SOURCE = 'RemoteExecutor';
const LOG_CATEGORY = 'Worker';

/**
 * Message types for worker communication
 *
 * 消息类型白名单 — 与 Worker 端 PdfiumNative 的方法名一一对应。
 * TypeScript 会在编译时检查 send() 调用的 method 参数，
 * 确保不会发送不存在的方法名到 Worker。
 */
type MessageType =
  | 'destroy'
  | 'openDocumentBuffer'
  | 'getMetadata'
  | 'setMetadata'
  | 'getDocPermissions'
  | 'getDocUserPermissions'
  | 'getSignatures'
  | 'getBookmarks'
  | 'setBookmarks'
  | 'deleteBookmarks'
  | 'renderPageRaw'
  | 'renderPageRect'
  | 'renderThumbnailRaw'
  | 'renderPageAnnotationRaw'
  | 'renderPageAnnotationsRaw'
  | 'getPageAnnotations'
  | 'getPageAnnotationsRaw'
  | 'createPageAnnotation'
  | 'updatePageAnnotation'
  | 'removePageAnnotation'
  | 'getPageTextRects'
  | 'searchInPage'
  | 'getAnnotationsBatch'
  | 'searchBatch'
  | 'getAttachments'
  | 'addAttachment'
  | 'removeAttachment'
  | 'readAttachmentContent'
  | 'getDocumentJavaScriptActions'
  | 'getPageAnnoWidgets'
  | 'getPageWidgetJavaScriptActions'
  | 'setFormFieldValue'
  | 'setFormFieldState'
  | 'renameWidgetField'
  | 'shareWidgetField'
  | 'regenerateWidgetAppearances'
  | 'flattenPage'
  | 'extractPages'
  | 'createDocument'
  | 'importPages'
  | 'deletePage'
  | 'extractText'
  | 'redactTextInRects'
  | 'applyRedaction'
  | 'applyAllRedactions'
  | 'flattenAnnotation'
  | 'exportAnnotationAppearanceAsPdf'
  | 'exportAnnotationsAppearanceAsPdf'
  | 'getTextSlices'
  | 'getPageGlyphs'
  | 'getPageGeometry'
  | 'getPageTextRuns'
  | 'merge'
  | 'mergePages'
  | 'preparePrintDocument'
  | 'saveAsCopy'
  | 'closeDocument'
  | 'closeAllDocuments'
  | 'setDocumentEncryption'
  | 'removeEncryption'
  | 'unlockOwnerPermissions'
  | 'isEncrypted'
  | 'isOwnerUnlocked';

/**
 * RemoteExecutor — 主线程端的 Worker 代理执行器
 *
 * 实现 IPdfiumExecutor 接口，但将所有方法调用通过 postMessage 转发到 Worker。
 * 职责：
 *   - 消息序列化/反序列化
 *   - Promise/Task 转换（Worker 的 postMessage 响应 → TypeScript Task）
 *   - 错误处理与转发
 *   - 进度追踪与转发
 */
export class RemoteExecutor implements IPdfiumExecutor {
  /** Worker 初始化完成消息的特殊 ID */
  private static READY_TASK_ID = '0';
  /** 等待响应的请求映射：请求 ID → 代理 Task */
  private pendingRequests = new Map<string, Task<any, any, any>>();
  /** 请求 ID 递增计数器（配合时间戳生成唯一 ID） */
  private requestCounter = 0;
  private logger: Logger;
  /** Worker 就绪 Task — 在 Worker 发送 'ready' 前阻塞所有执行请求 */
  private readyTask: Task<boolean, PdfErrorReason>;

  /**
   * 构造 RemoteExecutor 并启动 Worker 初始化流程
   *
   * @param worker  - 已创建的 Web Worker 实例
   * @param options - 配置选项（WASM URL、日志器、字体回退配置）
   */
  constructor(
    private worker: Worker,
    options: RemoteExecutorOptions,
  ) {
    this.logger = options.logger ?? new NoopLogger();
    // 注册消息监听器，接收 Worker 的响应
    this.worker.addEventListener('message', this.handleMessage);

    // 创建就绪 Task — Worker 发送 'ready' 后 resolve
    this.readyTask = new Task<boolean, PdfErrorReason>();
    this.pendingRequests.set(RemoteExecutor.READY_TASK_ID, this.readyTask);

    // 向 Worker 发送初始化消息，携带 WASM URL、序列化的日志器和字体回退配置
    this.worker.postMessage({
      id: RemoteExecutor.READY_TASK_ID,
      type: 'wasmInit',
      wasmUrl: options.wasmUrl,
      logger: options.logger ? serializeLogger(options.logger) : undefined,
      fontFallback: options.fontFallback,
    });

    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'RemoteExecutor created');
  }

  /** 生成唯一请求 ID（时间戳 + 递增计数器） */
  private generateId(): string {
    return `req-${Date.now()}-${this.requestCounter++}`;
  }

  /**
   * 向 Worker 发送执行请求并返回代理 Task
   *
   * 关键机制：等待 Worker 就绪后才发送请求。
   * 如果 Worker 初始化失败，所有排队请求会被统一 reject。
   *
   * @param method - 要调用的 PdfiumNative 方法名
   * @param args   - 方法参数数组
   * @returns 代理 Task，Worker 返回结果后会自动 resolve/reject
   */
  private send<T, P = unknown>(method: MessageType, args: any[]): Task<T, PdfErrorReason, P> {
    const id = this.generateId();
    const task = new Task<T, PdfErrorReason, P>();

    const request: WorkerRequest = {
      id,
      type: 'execute',
      method,
      args,
    };

    // ★ 关键：等待 Worker 初始化完成后才发送执行请求
    // readyTask 在 Worker 发送 'ready' 消息后 resolve
    this.readyTask.wait(
      () => {
        // Worker 已就绪，将 Task 加入待响应映射并发送请求
        this.pendingRequests.set(id, task);
        this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Sending ${method} request:`, id);
        this.worker.postMessage(request);
      },
      (error) => {
        // Worker 初始化失败，直接 reject 该请求
        this.logger.error(
          LOG_SOURCE,
          LOG_CATEGORY,
          `Worker init failed, rejecting ${method}:`,
          error,
        );
        task.reject({
          code: PdfErrorCode.Initialization,
          message: 'Worker initialization failed',
        });
      },
    );

    return task;
  }

  /**
   * 处理来自 Worker 的响应消息
   *
   * 根据响应类型分发：
   *   - 'ready'    → resolve readyTask（Worker 初始化完成）
   *   - 'result'   → resolve 对应请求的 Task
   *   - 'error'    → fail 对应请求的 Task
   *   - 'progress' → 触发对应 Task 的进度事件
   */
  private handleMessage = (event: MessageEvent<WorkerResponse>) => {
    const response = event.data;

    // Worker 初始化完成，resolve readyTask
    if (response.type === 'ready') {
      this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'Worker is ready');
      this.readyTask.resolve(true);
      return;
    }

    // 根据 ID 查找对应的待响应 Task
    const task = this.pendingRequests.get(response.id);

    if (!task) {
      // 收到未知请求的响应（可能是已取消的请求）
      this.logger.warn(
        LOG_SOURCE,
        LOG_CATEGORY,
        `Received response for unknown request: ${response.id}`,
      );
      return;
    }

    switch (response.type) {
      case 'result':
        // 方法执行成功，resolve Task 并清理映射
        this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Received result for ${response.id}`);
        task.resolve(response.data);
        this.pendingRequests.delete(response.id);
        break;

      case 'error':
        // 方法执行失败，fail Task 并清理映射
        this.logger.debug(
          LOG_SOURCE,
          LOG_CATEGORY,
          `Received error for ${response.id}:`,
          response.error,
        );
        if (response.error) {
          task.fail(response.error);
        } else {
          task.reject({ code: PdfErrorCode.Unknown, message: 'Unknown error' });
        }
        this.pendingRequests.delete(response.id);
        break;

      case 'progress':
        this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Received progress for ${response.id}`);
        task.progress(response.progress);
        break;
    }
  };

  /**
   * Cleanup and terminate worker
   */
  destroy(): void {
    this.worker.removeEventListener('message', this.handleMessage);

    // Reject all pending requests (except readyTask)
    this.pendingRequests.forEach((task, id) => {
      if (id !== RemoteExecutor.READY_TASK_ID) {
        task.abort('Worker destroyed');
        this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Aborted pending request: ${id}`);
      }
    });
    this.pendingRequests.clear();

    this.worker.terminate();
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'RemoteExecutor destroyed');
  }

  // ========== IPdfExecutor Implementation ==========

  openDocumentBuffer(
    file: PdfFile,
    options?: PdfOpenDocumentBufferOptions,
  ): PdfTask<PdfDocumentObject> {
    return this.send<PdfDocumentObject>('openDocumentBuffer', [file, options]);
  }

  getMetadata(doc: PdfDocumentObject): PdfTask<PdfMetadataObject> {
    return this.send<PdfMetadataObject>('getMetadata', [doc]);
  }

  setMetadata(doc: PdfDocumentObject, metadata: Partial<PdfMetadataObject>): PdfTask<boolean> {
    return this.send<boolean>('setMetadata', [doc, metadata]);
  }

  getDocPermissions(doc: PdfDocumentObject): PdfTask<number> {
    return this.send<number>('getDocPermissions', [doc]);
  }

  getDocUserPermissions(doc: PdfDocumentObject): PdfTask<number> {
    return this.send<number>('getDocUserPermissions', [doc]);
  }

  getSignatures(doc: PdfDocumentObject): PdfTask<PdfSignatureObject[]> {
    return this.send<PdfSignatureObject[]>('getSignatures', [doc]);
  }

  getBookmarks(doc: PdfDocumentObject): PdfTask<PdfBookmarksObject> {
    return this.send<PdfBookmarksObject>('getBookmarks', [doc]);
  }

  setBookmarks(doc: PdfDocumentObject, bookmarks: PdfBookmarkObject[]): PdfTask<boolean> {
    return this.send<boolean>('setBookmarks', [doc, bookmarks]);
  }

  deleteBookmarks(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.send<boolean>('deleteBookmarks', [doc]);
  }

  renderPageRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageOptions,
  ): PdfTask<ImageDataLike> {
    return this.send<ImageDataLike>('renderPageRaw', [doc, page, options]);
  }

  renderPageRect(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rect: Rect,
    options?: PdfRenderPageOptions,
  ): PdfTask<ImageDataLike> {
    return this.send<ImageDataLike>('renderPageRect', [doc, page, rect, options]);
  }

  renderThumbnailRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderThumbnailOptions,
  ): PdfTask<ImageDataLike> {
    return this.send<ImageDataLike>('renderThumbnailRaw', [doc, page, options]);
  }

  renderPageAnnotationRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
    options?: PdfRenderPageAnnotationOptions,
  ): PdfTask<ImageDataLike> {
    return this.send<ImageDataLike>('renderPageAnnotationRaw', [doc, page, annotation, options]);
  }

  renderPageAnnotationsRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageAnnotationOptions,
  ): PdfTask<AnnotationAppearanceMap> {
    return this.send<AnnotationAppearanceMap>('renderPageAnnotationsRaw', [doc, page, options]);
  }

  getPageAnnotationsRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
  ): PdfTask<PdfAnnotationObject[]> {
    return this.send<PdfAnnotationObject[]>('getPageAnnotationsRaw', [doc, page]);
  }

  getPageAnnotations(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfAnnotationObject[]> {
    return this.send<PdfAnnotationObject[]>('getPageAnnotations', [doc, page]);
  }

  createPageAnnotation<A extends PdfAnnotationObject>(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: A,
    context?: AnnotationCreateContext<A>,
  ): PdfTask<string> {
    return this.send<string>('createPageAnnotation', [doc, page, annotation, context]);
  }

  updatePageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
    options?: { regenerateAppearance?: boolean },
  ): PdfTask<boolean> {
    return this.send<boolean>('updatePageAnnotation', [doc, page, annotation, options]);
  }

  removePageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<boolean> {
    return this.send<boolean>('removePageAnnotation', [doc, page, annotation]);
  }

  getPageTextRects(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfTextRectObject[]> {
    return this.send<PdfTextRectObject[]>('getPageTextRects', [doc, page]);
  }

  searchInPage(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    keyword: string,
    flags: number,
  ): PdfTask<SearchResult[]> {
    return this.send<SearchResult[]>('searchInPage', [doc, page, keyword, flags]);
  }

  getAnnotationsBatch(
    doc: PdfDocumentObject,
    pages: PdfPageObject[],
  ): PdfTask<Record<number, PdfAnnotationObject[]>, BatchProgress<PdfAnnotationObject[]>> {
    return this.send<Record<number, PdfAnnotationObject[]>, BatchProgress<PdfAnnotationObject[]>>(
      'getAnnotationsBatch',
      [doc, pages],
    );
  }

  searchBatch(
    doc: PdfDocumentObject,
    pages: PdfPageObject[],
    keyword: string,
    flags: number,
  ): PdfTask<Record<number, SearchResult[]>, BatchProgress<SearchResult[]>> {
    return this.send<Record<number, SearchResult[]>, BatchProgress<SearchResult[]>>('searchBatch', [
      doc,
      pages,
      keyword,
      flags,
    ]);
  }

  getAttachments(doc: PdfDocumentObject): PdfTask<PdfAttachmentObject[]> {
    return this.send<PdfAttachmentObject[]>('getAttachments', [doc]);
  }

  addAttachment(doc: PdfDocumentObject, params: PdfAddAttachmentParams): PdfTask<boolean> {
    return this.send<boolean>('addAttachment', [doc, params]);
  }

  removeAttachment(doc: PdfDocumentObject, attachment: PdfAttachmentObject): PdfTask<boolean> {
    return this.send<boolean>('removeAttachment', [doc, attachment]);
  }

  readAttachmentContent(
    doc: PdfDocumentObject,
    attachment: PdfAttachmentObject,
  ): PdfTask<ArrayBuffer> {
    return this.send<ArrayBuffer>('readAttachmentContent', [doc, attachment]);
  }

  getDocumentJavaScriptActions(
    doc: PdfDocumentObject,
  ): PdfTask<PdfDocumentJavaScriptActionObject[]> {
    return this.send<PdfDocumentJavaScriptActionObject[]>('getDocumentJavaScriptActions', [doc]);
  }

  getPageAnnoWidgets(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfWidgetAnnoObject[]> {
    return this.send<PdfWidgetAnnoObject[]>('getPageAnnoWidgets', [doc, page]);
  }

  getPageWidgetJavaScriptActions(
    doc: PdfDocumentObject,
    page: PdfPageObject,
  ): PdfTask<PdfWidgetJavaScriptActionObject[]> {
    return this.send<PdfWidgetJavaScriptActionObject[]>('getPageWidgetJavaScriptActions', [
      doc,
      page,
    ]);
  }

  setFormFieldValue(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    value: FormFieldValue,
  ): PdfTask<boolean> {
    return this.send<boolean>('setFormFieldValue', [doc, page, annotation, value]);
  }

  setFormFieldState(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    field: PdfWidgetAnnoField,
  ): PdfTask<boolean> {
    return this.send<boolean>('setFormFieldState', [doc, page, annotation, field]);
  }

  renameWidgetField(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    name: string,
  ): PdfTask<boolean> {
    return this.send<boolean>('renameWidgetField', [doc, page, annotation, name]);
  }

  shareWidgetField(
    doc: PdfDocumentObject,
    sourcePage: PdfPageObject,
    sourceAnnotation: PdfWidgetAnnoObject,
    targetPage: PdfPageObject,
    targetAnnotation: PdfWidgetAnnoObject,
  ): PdfTask<boolean> {
    return this.send<boolean>('shareWidgetField', [
      doc,
      sourcePage,
      sourceAnnotation,
      targetPage,
      targetAnnotation,
    ]);
  }

  regenerateWidgetAppearances(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationIds: string[],
  ): PdfTask<boolean> {
    return this.send<boolean>('regenerateWidgetAppearances', [doc, page, annotationIds]);
  }

  flattenPage(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfFlattenPageOptions,
  ): PdfTask<PdfPageFlattenResult> {
    return this.send<PdfPageFlattenResult>('flattenPage', [doc, page, options]);
  }

  extractPages(doc: PdfDocumentObject, pageIndexes: number[]): PdfTask<ArrayBuffer> {
    return this.send<ArrayBuffer>('extractPages', [doc, pageIndexes]);
  }

  createDocument(id: string): PdfTask<PdfDocumentObject> {
    return this.send<PdfDocumentObject>('createDocument', [id]);
  }

  importPages(
    destDoc: PdfDocumentObject,
    srcDoc: PdfDocumentObject,
    srcPageIndices: number[],
    insertIndex?: number,
  ): PdfTask<PdfPageObject[]> {
    return this.send<PdfPageObject[]>('importPages', [
      destDoc,
      srcDoc,
      srcPageIndices,
      insertIndex,
    ]);
  }

  deletePage(doc: PdfDocumentObject, pageIndex: number): PdfTask<boolean> {
    return this.send<boolean>('deletePage', [doc, pageIndex]);
  }

  extractText(doc: PdfDocumentObject, pageIndexes: number[]): PdfTask<string> {
    return this.send<string>('extractText', [doc, pageIndexes]);
  }

  redactTextInRects(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rects: Rect[],
    options?: PdfRedactTextOptions,
  ): PdfTask<boolean> {
    return this.send<boolean>('redactTextInRects', [doc, page, rects, options]);
  }

  applyRedaction(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<boolean> {
    return this.send<boolean>('applyRedaction', [doc, page, annotation]);
  }

  applyAllRedactions(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<boolean> {
    return this.send<boolean>('applyAllRedactions', [doc, page]);
  }

  flattenAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<boolean> {
    return this.send<boolean>('flattenAnnotation', [doc, page, annotation]);
  }

  exportAnnotationAppearanceAsPdf(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<ArrayBuffer> {
    return this.send<ArrayBuffer>('exportAnnotationAppearanceAsPdf', [doc, page, annotation]);
  }

  exportAnnotationsAppearanceAsPdf(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotations: PdfAnnotationObject[],
  ): PdfTask<ArrayBuffer> {
    return this.send<ArrayBuffer>('exportAnnotationsAppearanceAsPdf', [doc, page, annotations]);
  }

  getTextSlices(doc: PdfDocumentObject, slices: PageTextSlice[]): PdfTask<string[]> {
    return this.send<string[]>('getTextSlices', [doc, slices]);
  }

  getPageGlyphs(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfGlyphObject[]> {
    return this.send<PdfGlyphObject[]>('getPageGlyphs', [doc, page]);
  }

  getPageGeometry(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfPageGeometry> {
    return this.send<PdfPageGeometry>('getPageGeometry', [doc, page]);
  }

  getPageTextRuns(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfPageTextRuns> {
    return this.send<PdfPageTextRuns>('getPageTextRuns', [doc, page]);
  }

  merge(files: PdfFile[]): PdfTask<PdfFile> {
    return this.send<PdfFile>('merge', [files]);
  }

  mergePages(mergeConfigs: Array<{ docId: string; pageIndices: number[] }>): PdfTask<PdfFile> {
    return this.send<PdfFile>('mergePages', [mergeConfigs]);
  }

  preparePrintDocument(doc: PdfDocumentObject, options?: PdfPrintOptions): PdfTask<ArrayBuffer> {
    return this.send<ArrayBuffer>('preparePrintDocument', [doc, options]);
  }

  saveAsCopy(doc: PdfDocumentObject): PdfTask<ArrayBuffer> {
    return this.send<ArrayBuffer>('saveAsCopy', [doc]);
  }

  closeDocument(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.send<boolean>('closeDocument', [doc]);
  }

  closeAllDocuments(): PdfTask<boolean> {
    return this.send<boolean>('closeAllDocuments', []);
  }

  setDocumentEncryption(
    doc: PdfDocumentObject,
    userPassword: string,
    ownerPassword: string,
    allowedFlags: number,
  ): PdfTask<boolean> {
    return this.send<boolean>('setDocumentEncryption', [
      doc,
      userPassword,
      ownerPassword,
      allowedFlags,
    ]);
  }

  removeEncryption(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.send<boolean>('removeEncryption', [doc]);
  }

  unlockOwnerPermissions(doc: PdfDocumentObject, ownerPassword: string): PdfTask<boolean> {
    return this.send<boolean>('unlockOwnerPermissions', [doc, ownerPassword]);
  }

  isEncrypted(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.send<boolean>('isEncrypted', [doc]);
  }

  isOwnerUnlocked(doc: PdfDocumentObject): PdfTask<boolean> {
    return this.send<boolean>('isOwnerUnlocked', [doc]);
  }
}
