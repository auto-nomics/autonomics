/**
 * @file 主线程端 Worker 引擎代理（WebWorkerEngine）
 *
 * 在主线程中运行，作为真实 PdfEngine 的远程代理。
 * 所有 PdfEngine 接口方法的调用都被转换为消息，
 * 发送给 Worker 线程中的 EngineRunner 执行，
 * 并通过 Promise（Task）模式异步返回结果。
 *
 * 架构角色：
 *   ┌───────────────┐     postMessage      ┌───────────────────┐
 *   │ WebWorkerEngine│ ──────────────────→ │   EngineRunner    │
 *   │  （主线程代理） │ ←────────────────── │  （Worker 端执行） │
 *   └───────────────┘     onmessage        └───────────────────┘
 *
 * 核心机制：
 *   1. readyTask 门控：所有请求在 Worker 就绪后才发送，
 *      避免在引擎初始化期间丢失请求
 *   2. WorkerTask 管理：每个请求创建一个 WorkerTask，
 *      通过唯一 ID 关联请求和响应
 *   3. 进度支持：通过 Task 的 onProgress 回调接收中间进度
 *
 * 设计决策：
 *   - 每个方法都是完整的代理实现（而非动态调用），
 *     确保 TypeScript 类型检查覆盖所有方法
 *   - 使用 Transferable Objects 优化 ArrayBuffer 传输性能
 */

import {
  FormFieldValue,
  PdfWidgetAnnoField,
  PdfAttachmentObject,
  PdfDocumentJavaScriptActionObject,
  PdfFile,
  PdfMetadataObject,
  PdfSignatureObject,
  PdfTextRectObject,
  PdfWidgetAnnoObject,
  Task,
  Logger,
  NoopLogger,
  PdfAnnotationObject,
  PdfBookmarksObject,
  PdfDocumentObject,
  PdfEngine,
  PdfPageObject,
  Rect,
  PdfErrorCode,
  PdfErrorReason,
  PdfPageFlattenResult,
  SearchAllPagesResult,
  PdfFileUrl,
  PdfGlyphObject,
  PdfPageGeometry,
  PdfPageTextRuns,
  PageTextSlice,
  AnnotationCreateContext,
  PdfEngineMethodArgs,
  PdfEngineMethodName,
  PdfPageSearchProgress,
  PdfRenderThumbnailOptions,
  PdfSearchAllPagesOptions,
  PdfFlattenPageOptions,
  PdfRedactTextOptions,
  PdfRenderPageAnnotationOptions,
  PdfRenderPageOptions,
  PdfOpenDocumentUrlOptions,
  PdfOpenDocumentBufferOptions,
  PdfAnnotationsProgress,
  PdfPrintOptions,
  PdfBookmarkObject,
  PdfWidgetJavaScriptActionObject,
  PdfAddAttachmentParams,
  AnnotationAppearanceMap,
  ImageDataLike,
} from '../../models';
import { ExecuteRequest, Response, SpecificExecuteRequest } from './runner';

const LOG_SOURCE = 'WebWorkerEngine';
const LOG_CATEGORY = 'Engine';

// ======================== 辅助工具 ========================

/**
 * 创建发送给 Worker 的执行请求
 *
 * 将方法名和参数打包为标准的 ExecuteRequest 格式。
 * 使用泛型确保方法名和参数类型一一对应。
 *
 * @param id - 请求唯一标识
 * @param name - PdfEngine 方法名
 * @param args - 方法参数
 * @returns 类型化的执行请求
 */
function createRequest<M extends PdfEngineMethodName>(
  id: string,
  name: M,
  args: PdfEngineMethodArgs<M>,
): SpecificExecuteRequest<M> {
  return {
    id,
    type: 'ExecuteRequest',
    data: {
      name,
      args,
    },
  };
}

// ======================== Worker 端任务封装 ========================

/**
 * Worker 任务封装
 *
 * 继承自 Task，增加了与 Worker 的关联能力：
 *   - abort() 时向 Worker 发送 AbortRequest 取消任务
 *   - progress() 供 handle 回调触发进度通知
 *   - 通过 messageId 与 Worker 端的请求一一对应
 *
 * @typeParam R - 任务结果类型
 * @typeParam P - 进度数据类型
 */
export class WorkerTask<R, P = unknown> extends Task<R, PdfErrorReason, P> {
  /**
   * 创建绑定到指定 Worker 消息的任务
   * @param worker - Web Worker 实例
   * @param messageId - 与 Worker 消息关联的唯一 ID
   */
  constructor(
    public worker: Worker,
    private messageId: string,
  ) {
    super();
  }

  /**
   * 中止任务
   *
   * 调用父类 abort 标记任务为已取消，
   * 同时向 Worker 发送 AbortRequest 通知其停止处理。
   *
   * @override
   */
  abort(e: PdfErrorReason) {
    super.abort(e);

    this.worker.postMessage({
      id: this.messageId,
      type: 'AbortRequest',
    });
  }

  /**
   * 报告进度
   *
   * 由 WebWorkerEngine 的 handle 方法调用，
   * 当收到 Worker 的 ExecuteProgress 消息时触发。
   *
   * @override
   */
  progress(p: P) {
    super.progress(p);
  }
}

// ======================== 主线程 Worker 引擎代理 ========================

/**
 * 主线程端的 Worker 引擎代理
 *
 * 实现 PdfEngine 接口，但所有方法调用都被转发到 Worker 线程执行。
 * 上层代码透明地使用此代理，无需关心 Worker 通信细节。
 *
 * 生命周期：
 *   1. 构造时注册 Worker 的 message 监听器
 *   2. 创建 readyTask 等待 Worker 初始化完成
 *   3. 所有方法调用通过 proxy() 发送请求，等待 Worker 响应
 *   4. destroy() 终止 Worker 线程
 */
export class WebWorkerEngine implements PdfEngine {
  /** Worker 就绪响应的固定 ID */
  static readyTaskId = '0';

  /**
   * Worker 就绪任务
   *
   * 在 Worker 初始化期间，所有通过 proxy() 发送的请求
   * 都会等待此任务完成后才真正发送给 Worker。
   * Worker 发送 ReadyResponse 后此任务 resolve。
   */
  readyTask: WorkerTask<boolean>;

  /**
   * 正在执行的任务映射表
   *
   * key 为请求 ID，value 为对应的 WorkerTask。
   * 当收到 ExecuteResponse 时，根据 ID 找到对应的 task 并 resolve/reject。
   */
  tasks: Map<string, WorkerTask<any, any>> = new Map();

  /**
   * 创建 WebWorkerEngine 实例
   *
   * @param worker - 已创建的 Web Worker 实例（内部运行 EngineRunner）
   * @param logger - 日志记录器
   */
  constructor(
    private worker: Worker,
    private logger: Logger = new NoopLogger(),
  ) {
    // 注册 Worker 消息监听器
    this.worker.addEventListener('message', this.handle);

    // 创建就绪任务并注册到 tasks 映射表
    this.readyTask = new WorkerTask<boolean>(this.worker, WebWorkerEngine.readyTaskId);
    this.tasks.set(WebWorkerEngine.readyTaskId, this.readyTask);
  }

  /**
   * 处理 Worker 发来的消息
   *
   * 根据响应类型分发处理：
   *   - ReadyResponse：Worker 已就绪，resolve readyTask
   *   - ExecuteProgress：任务进度更新，调用 task.progress()
   *   - ExecuteResponse：任务完成，resolve/reject task 并从映射表移除
   *
   * @param evt - Worker 的 MessageEvent
   */
  handle = (evt: MessageEvent<any>) => {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'webworker engine start handling message: ',
      evt.data,
    );
    try {
      const response = evt.data as Response;
      const task = this.tasks.get(response.id);
      if (!task) {
        return;
      }

      switch (response.type) {
        case 'ReadyResponse':
          // Worker 引擎初始化完成
          this.readyTask.resolve(true);
          break;
        case 'ExecuteProgress':
          // 任务进度更新
          task.progress(response.data);
          break;
        case 'ExecuteResponse':
          {
            // 任务执行完成（成功或失败）
            switch (response.data.type) {
              case 'result':
                task.resolve(response.data.value);
                break;
              case 'error':
                task.reject(response.data.value.reason);
                break;
            }
            // 从映射表中移除已完成的任务
            this.tasks.delete(response.id);
          }
          break;
      }
    } catch (e) {
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, 'webworker met error when handling message: ', e);
    }
  };

  /**
   * 生成唯一的请求 ID
   *
   * 格式：`{前缀}.{时间戳}.{随机数}`，确保全局唯一。
   *
   * @param id - 前缀标识（通常为文档 ID）
   * @returns 唯一请求 ID
   */
  generateRequestId(id: string) {
    return `${id}.${Date.now()}.${Math.random()}`;
  }

  // ======================== PdfEngine 接口实现 ========================
  // 每个方法的实现模式相同：
  //   1. 生成唯一请求 ID
  //   2. 创建 WorkerTask
  //   3. 构造 ExecuteRequest
  //   4. 通过 proxy() 发送请求

  /** 销毁引擎，终止 Worker 线程 */
  destroy() {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'destroy');
    const requestId = this.generateRequestId('General');
    const task = new WorkerTask<boolean>(this.worker, requestId);

    // 无论成功或失败，都移除监听器并终止 Worker
    const finish = () => {
      this.worker.removeEventListener('message', this.handle);
      this.worker.terminate();
    };

    task.wait(finish, finish);

    const request: ExecuteRequest = createRequest(requestId, 'destroy', []);
    this.proxy(task, request);

    return task;
  }

  /** 通过 URL 打开 PDF 文档 */
  openDocumentUrl(file: PdfFileUrl, options?: PdfOpenDocumentUrlOptions) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'openDocumentUrl', file.url, options);
    const requestId = this.generateRequestId(file.id);
    const task = new WorkerTask<PdfDocumentObject>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'openDocumentUrl', [file, options]);
    this.proxy(task, request);

    return task;
  }

  /** 通过内存缓冲区打开 PDF 文档 */
  openDocumentBuffer(file: PdfFile, options?: PdfOpenDocumentBufferOptions) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'openDocumentBuffer', file, options);
    const requestId = this.generateRequestId(file.id);
    const task = new WorkerTask<PdfDocumentObject>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'openDocumentBuffer', [file, options]);
    this.proxy(task, request);

    return task;
  }

  /** 获取文档元数据（标题、作者等） */
  getMetadata(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getMetadata', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfMetadataObject>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getMetadata', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 设置文档元数据 */
  setMetadata(doc: PdfDocumentObject, metadata: Partial<PdfMetadataObject>) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'setMetadata', doc, metadata);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'setMetadata', [doc, metadata]);
    this.proxy(task, request);

    return task;
  }

  /** 获取文档权限标志位 */
  getDocPermissions(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getDocPermissions', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<number>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getDocPermissions', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 获取用户权限标志位 */
  getDocUserPermissions(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getDocUserPermissions', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<number>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getDocUserPermissions', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 获取书签树 */
  getBookmarks(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getBookmarks', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfBookmarksObject>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getBookmarks', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 设置书签树 */
  setBookmarks(doc: PdfDocumentObject, payload: PdfBookmarkObject[]) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'setBookmarks', doc, payload);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'setBookmarks', [doc, payload]);
    this.proxy(task, request);

    return task;
  }

  /** 删除所有书签 */
  deleteBookmarks(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'deleteBookmarks', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'deleteBookmarks', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 获取文档中的数字签名信息 */
  getSignatures(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getSignatures', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfSignatureObject[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getSignatures', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 渲染整页为 Blob（PNG/WebP） */
  renderPage(doc: PdfDocumentObject, page: PdfPageObject, options?: PdfRenderPageOptions) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderPage', doc, page, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<Blob>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'renderPage', [doc, page, options]);
    this.proxy(task, request);

    return task;
  }

  /** 渲染页面指定区域为 Blob */
  renderPageRect(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rect: Rect,
    options?: PdfRenderPageOptions,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderPageRect', doc, page, rect, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<Blob>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'renderPageRect', [
      doc,
      page,
      rect,
      options,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 渲染整页为原始像素数据（ImageData） */
  renderPageRaw(doc: PdfDocumentObject, page: PdfPageObject, options?: PdfRenderPageOptions) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderPageRaw', doc, page, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<ImageDataLike>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'renderPageRaw', [doc, page, options]);
    this.proxy(task, request);

    return task;
  }

  /** 渲染页面指定区域为原始像素数据 */
  renderPageRectRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rect: Rect,
    options?: PdfRenderPageOptions,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderPageRectRaw', doc, page, rect, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<ImageDataLike>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'renderPageRectRaw', [
      doc,
      page,
      rect,
      options,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 渲染单个注解的外观为 Blob */
  renderPageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
    options?: PdfRenderPageAnnotationOptions,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderAnnotation', doc, page, annotation, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<Blob>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'renderPageAnnotation', [
      doc,
      page,
      annotation,
      options,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 批量渲染页面注解外观为 Blob 映射 */
  renderPageAnnotations(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageAnnotationOptions,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderPageAnnotations', doc, page, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<AnnotationAppearanceMap<Blob>>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'renderPageAnnotations', [
      doc,
      page,
      options,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 批量渲染页面注解外观为原始像素数据 */
  renderPageAnnotationsRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageAnnotationOptions,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderPageAnnotationsRaw', doc, page, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<AnnotationAppearanceMap<ImageDataLike>>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'renderPageAnnotationsRaw', [
      doc,
      page,
      options,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 获取所有页面的注解（带进度回调） */
  getAllAnnotations(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getAllAnnotations', doc);
    const requestId = this.generateRequestId(doc.id);

    const task = new WorkerTask<Record<number, PdfAnnotationObject[]>, PdfAnnotationsProgress>(
      this.worker,
      requestId,
    );

    const request: ExecuteRequest = createRequest(requestId, 'getAllAnnotations', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 获取指定页面的注解列表 */
  getPageAnnotations(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageAnnotations', doc, page);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfAnnotationObject[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getPageAnnotations', [doc, page]);
    this.proxy(task, request);

    return task;
  }

  /** 获取指定页面的表单控件（Widget）注解 */
  getPageAnnoWidgets(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageAnnoWidgets', doc, page);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfWidgetAnnoObject[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getPageAnnoWidgets', [doc, page]);
    this.proxy(task, request);

    return task;
  }

  /** 获取文档级 JavaScript 动作 */
  getDocumentJavaScriptActions(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getDocumentJavaScriptActions', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfDocumentJavaScriptActionObject[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getDocumentJavaScriptActions', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 获取页面控件的 JavaScript 动作 */
  getPageWidgetJavaScriptActions(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageWidgetJavaScriptActions', doc, page);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfWidgetJavaScriptActionObject[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getPageWidgetJavaScriptActions', [
      doc,
      page,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 创建页面注解 */
  createPageAnnotation<A extends PdfAnnotationObject>(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: A,
    context?: AnnotationCreateContext<A>,
  ) {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'createPageAnnotations',
      doc,
      page,
      annotation,
      context,
    );
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<string>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'createPageAnnotation', [
      doc,
      page,
      annotation,
      context,
    ] as PdfEngineMethodArgs<'createPageAnnotation'>);
    this.proxy(task, request);

    return task;
  }

  /** 更新页面注解 */
  updatePageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
    options?: { regenerateAppearance?: boolean },
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'updatePageAnnotation', doc, page, annotation);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'updatePageAnnotation', [
      doc,
      page,
      annotation,
      options,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 删除页面注解 */
  removePageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'removePageAnnotations', doc, page, annotation);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'removePageAnnotation', [
      doc,
      page,
      annotation,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 获取页面文本矩形区域 */
  getPageTextRects(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageTextRects', doc, page);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfTextRectObject[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getPageTextRects', [doc, page]);
    this.proxy(task, request);

    return task;
  }

  /** 渲染缩略图 */
  renderThumbnail(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderThumbnailOptions,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderThumbnail', doc, page, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<Blob>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'renderThumbnail', [
      doc,
      page,
      options,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 在所有页面中搜索文本（带进度回调） */
  searchAllPages(doc: PdfDocumentObject, keyword: string, options?: PdfSearchAllPagesOptions) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'searchAllPages', doc, keyword, options);

    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<SearchAllPagesResult, PdfPageSearchProgress>(
      this.worker,
      requestId,
    );

    const request: ExecuteRequest = createRequest(requestId, 'searchAllPages', [
      doc,
      keyword,
      options,
    ]);

    this.proxy(task, request);
    return task;
  }

  /** 保存文档副本为 ArrayBuffer */
  saveAsCopy(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'saveAsCopy', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<ArrayBuffer>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'saveAsCopy', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 获取文档附件列表 */
  getAttachments(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getAttachments', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfAttachmentObject[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getAttachments', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 添加文档附件 */
  addAttachment(doc: PdfDocumentObject, params: PdfAddAttachmentParams) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'addAttachment', doc, params);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'addAttachment', [doc, params]);
    this.proxy(task, request);

    return task;
  }

  /** 移除文档附件 */
  removeAttachment(doc: PdfDocumentObject, attachment: PdfAttachmentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'removeAttachment', doc, attachment);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'removeAttachment', [doc, attachment]);
    this.proxy(task, request);

    return task;
  }

  /** 读取附件内容 */
  readAttachmentContent(doc: PdfDocumentObject, attachment: PdfAttachmentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'readAttachmentContent', doc, attachment);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<ArrayBuffer>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'readAttachmentContent', [
      doc,
      attachment,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 设置表单字段值 */
  setFormFieldValue(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    value: FormFieldValue,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'setFormFieldValue', doc, annotation, value);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'setFormFieldValue', [
      doc,
      page,
      annotation,
      value,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 设置表单字段状态（复选框、单选按钮等） */
  setFormFieldState(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    field: PdfWidgetAnnoField,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'setFormFieldState', doc, annotation, field);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'setFormFieldState', [
      doc,
      page,
      annotation,
      field,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 重命名表单控件字段 */
  renameWidgetField(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    name: string,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renameWidgetField', doc, annotation, name);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'renameWidgetField', [
      doc,
      page,
      annotation,
      name,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 共享表单控件字段（使两个控件关联同一字段名） */
  shareWidgetField(
    doc: PdfDocumentObject,
    sourcePage: PdfPageObject,
    sourceAnnotation: PdfWidgetAnnoObject,
    targetPage: PdfPageObject,
    targetAnnotation: PdfWidgetAnnoObject,
  ) {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'shareWidgetField',
      doc,
      sourceAnnotation,
      targetAnnotation,
    );
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'shareWidgetField', [
      doc,
      sourcePage,
      sourceAnnotation,
      targetPage,
      targetAnnotation,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 重新生成控件外观流 */
  regenerateWidgetAppearances(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationIds: string[],
  ) {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'regenerateWidgetAppearances',
      doc,
      page,
      annotationIds,
    );
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'regenerateWidgetAppearances', [
      doc,
      page,
      annotationIds,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 扁平化页面（将注解合并到页面内容中） */
  flattenPage(doc: PdfDocumentObject, page: PdfPageObject, options?: PdfFlattenPageOptions) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'flattenPage', doc, page, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfPageFlattenResult>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'flattenPage', [doc, page, options]);
    this.proxy(task, request);

    return task;
  }

  /** 提取指定页面为新文档 */
  extractPages(doc: PdfDocumentObject, pageIndexes: number[]) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'extractPages', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<ArrayBuffer>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'extractPages', [doc, pageIndexes]);
    this.proxy(task, request);

    return task;
  }

  /** 创建空白文档 */
  createDocument(id: string) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'createDocument', id);
    const requestId = this.generateRequestId(id);
    const task = new WorkerTask<PdfDocumentObject>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'createDocument', [id]);
    this.proxy(task, request);

    return task;
  }

  /** 将源文档的页面导入到目标文档 */
  importPages(
    destDoc: PdfDocumentObject,
    srcDoc: PdfDocumentObject,
    srcPageIndices: number[],
    insertIndex?: number,
  ) {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'importPages',
      destDoc.id,
      srcDoc.id,
      srcPageIndices,
    );
    const requestId = this.generateRequestId(destDoc.id);
    const task = new WorkerTask<PdfPageObject[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'importPages', [
      destDoc,
      srcDoc,
      srcPageIndices,
      insertIndex,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 删除指定页面 */
  deletePage(doc: PdfDocumentObject, pageIndex: number) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'deletePage', doc.id, pageIndex);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'deletePage', [doc, pageIndex]);
    this.proxy(task, request);

    return task;
  }

  /** 在指定矩形区域内涂黑文本（添加密文标记） */
  redactTextInRects(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rects: Rect[],
    options?: PdfRedactTextOptions,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'redactTextInRects', doc, page, rects, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'redactTextInRects', [
      doc,
      page,
      rects,
      options,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 应用单个密文注解 */
  applyRedaction(doc: PdfDocumentObject, page: PdfPageObject, annotation: PdfAnnotationObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'applyRedaction', doc, page, annotation);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'applyRedaction', [
      doc,
      page,
      annotation,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 应用页面上的所有密文注解 */
  applyAllRedactions(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'applyAllRedactions', doc, page);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'applyAllRedactions', [doc, page]);
    this.proxy(task, request);

    return task;
  }

  /** 扁平化单个注解（合并到页面内容） */
  flattenAnnotation(doc: PdfDocumentObject, page: PdfPageObject, annotation: PdfAnnotationObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'flattenAnnotation', doc, page, annotation);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'flattenAnnotation', [
      doc,
      page,
      annotation,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 导出单个注解外观为 PDF */
  exportAnnotationAppearanceAsPdf(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ) {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'exportAnnotationAppearanceAsPdf',
      doc,
      page,
      annotation,
    );
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<ArrayBuffer>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'exportAnnotationAppearanceAsPdf', [
      doc,
      page,
      annotation,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 批量导出注解外观为 PDF */
  exportAnnotationsAppearanceAsPdf(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotations: PdfAnnotationObject[],
  ) {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'exportAnnotationsAppearanceAsPdf',
      doc,
      page,
      annotations,
    );
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<ArrayBuffer>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'exportAnnotationsAppearanceAsPdf', [
      doc,
      page,
      annotations,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 提取指定页面的纯文本 */
  extractText(doc: PdfDocumentObject, pageIndexes: number[]) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'extractText', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<string>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'extractText', [doc, pageIndexes]);
    this.proxy(task, request);

    return task;
  }

  /** 获取指定文本切片的内容 */
  getTextSlices(doc: PdfDocumentObject, slices: PageTextSlice[]) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getTextSlices', doc, slices);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<string[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getTextSlices', [doc, slices]);
    this.proxy(task, request);

    return task;
  }

  /** 获取页面字形（Glyph）信息 */
  getPageGlyphs(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageGlyphs', doc, page);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfGlyphObject[]>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getPageGlyphs', [doc, page]);
    this.proxy(task, request);

    return task;
  }

  /** 获取页面几何信息（文本块、段落等布局数据） */
  getPageGeometry(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageGeometry', doc, page);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfPageGeometry>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getPageGeometry', [doc, page]);
    this.proxy(task, request);

    return task;
  }

  /** 获取页面文本排版信息（字体、字号、位置等） */
  getPageTextRuns(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageTextRuns', doc, page);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<PdfPageTextRuns>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'getPageTextRuns', [doc, page]);
    this.proxy(task, request);

    return task;
  }

  /** 合并多个 PDF 文件 */
  merge(files: PdfFile[]) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'merge', files);
    const fileIds = files.map((file) => file.id).join('.');
    const requestId = this.generateRequestId(fileIds);
    const task = new WorkerTask<PdfFile>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'merge', [files]);
    this.proxy(task, request);

    return task;
  }

  /** 按页面粒度合并多个文档 */
  mergePages(mergeConfigs: Array<{ docId: string; pageIndices: number[] }>) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'mergePages', mergeConfigs);
    const requestId = this.generateRequestId(mergeConfigs.map((config) => config.docId).join('.'));
    const task = new WorkerTask<PdfFile>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'mergePages', [mergeConfigs]);
    this.proxy(task, request);

    return task;
  }

  /** 准备打印文档（生成打印优化的 PDF） */
  preparePrintDocument(doc: PdfDocumentObject, options?: PdfPrintOptions) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'preparePrintDocument', doc, options);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<ArrayBuffer>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'preparePrintDocument', [
      doc,
      options,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 关闭文档 */
  closeDocument(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'closeDocument', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'closeDocument', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 关闭所有已打开的文档 */
  closeAllDocuments() {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'closeAllDocuments');
    const requestId = this.generateRequestId('closeAllDocuments');
    const task = new WorkerTask<boolean>(this.worker, requestId);
    const request: ExecuteRequest = createRequest(requestId, 'closeAllDocuments', []);
    this.proxy(task, request);

    return task;
  }

  /** 设置文档加密 */
  setDocumentEncryption(
    doc: PdfDocumentObject,
    userPassword: string,
    ownerPassword: string,
    allowedFlags: number,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'setDocumentEncryption', doc, allowedFlags);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'setDocumentEncryption', [
      doc,
      userPassword,
      ownerPassword,
      allowedFlags,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 移除文档加密 */
  removeEncryption(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'removeEncryption', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'removeEncryption', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 使用所有者密码解锁权限 */
  unlockOwnerPermissions(doc: PdfDocumentObject, ownerPassword: string) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'unlockOwnerPermissions', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'unlockOwnerPermissions', [
      doc,
      ownerPassword,
    ]);
    this.proxy(task, request);

    return task;
  }

  /** 检查文档是否加密 */
  isEncrypted(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'isEncrypted', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'isEncrypted', [doc]);
    this.proxy(task, request);

    return task;
  }

  /** 检查所有者权限是否已解锁 */
  isOwnerUnlocked(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'isOwnerUnlocked', doc);
    const requestId = this.generateRequestId(doc.id);
    const task = new WorkerTask<boolean>(this.worker, requestId);

    const request: ExecuteRequest = createRequest(requestId, 'isOwnerUnlocked', [doc]);
    this.proxy(task, request);

    return task;
  }

  /**
   * 将请求代理到 Worker 执行（核心方法）
   *
   * 执行流程：
   *   1. 等待 readyTask 完成（Worker 已就绪）
   *   2. 通过 postMessage 将请求发送给 Worker
   *   3. 将 WorkerTask 注册到 tasks 映射表
   *   4. Worker 处理完成后通过 handle 回调 resolve/reject task
   *
   * 如果 Worker 初始化失败，直接 reject task 并返回 Initialization 错误。
   *
   * @param task - 等待响应的任务
   * @param request - 发送给 Worker 的请求
   * @param transferables - 需要转移所有权的 ArrayBuffer 对象列表
   *
   * @internal
   */
  proxy<R, P = unknown>(task: WorkerTask<R, P>, request: ExecuteRequest, transferables: any[] = []) {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'send request to worker',
      task,
      request,
      transferables,
    );
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `${request.data.name}`, 'Begin', request.id);

    // 等待 Worker 就绪后再发送请求
    this.readyTask.wait(
      () => {
        // Worker 已就绪，发送请求
        this.worker.postMessage(request, transferables);

        // 记录性能数据：任务完成时标记 End
        task.wait(
          () => {
            this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `${request.data.name}`, 'End', request.id);
          },
          () => {
            this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `${request.data.name}`, 'End', request.id);
          },
        );

        // 将任务注册到映射表，等待 Worker 响应
        this.tasks.set(request.id, task);
      },
      () => {
        // Worker 初始化失败，直接 reject
        this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `${request.data.name}`, 'End', request.id);
        task.reject({
          code: PdfErrorCode.Initialization,
          message: 'worker initialization failed',
        });
      },
    );
  }
}
