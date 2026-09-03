/**
 * @file 通用 Worker 端引擎运行器（EngineRunner）
 *
 * 在 Web Worker 线程中运行，接收主线程发来的引擎方法调用请求，
 * 转发给实际的 PdfEngine 实例执行，并将结果通过 postMessage 返回。
 *
 * 通信协议：
 *   主线程 → Worker：Request（ExecuteRequest | AbortRequest）
 *   Worker → 主线程：Response（ExecuteResponse | ReadyResponse | ExecuteProgress）
 *
 * 核心流程：
 *   1. 子类在 Worker 线程中初始化 PdfEngine 实例并赋值给 this.engine
 *   2. 调用 ready() 方法开始监听消息并发送 ReadyResponse
 *   3. 收到 ExecuteRequest 后，根据方法名分发到对应的引擎方法
 *   4. 将引擎方法的 TaskReturn 结果绑定到 handleTask，
 *      通过 postMessage 回传进度和最终结果
 *
 * 设计决策：
 *   - 方法分发使用 switch-case 而非动态调用，确保类型安全
 *   - TaskReturn 的 onProgress/wait 回调机制实现了异步进度上报
 *   - 引擎未初始化时返回 NotReady 错误，避免空指针异常
 */

import {
  Logger,
  NoopLogger,
  PdfEngine,
  PdfEngineError,
  PdfEngineMethodArgs,
  PdfEngineMethodName,
  PdfEngineMethodReturnType,
  PdfErrorCode,
  TaskReturn,
} from '../../models';

// ======================== 通信协议类型定义 ========================

/**
 * 引擎方法调用请求体
 *
 * 将 PdfEngine 的所有方法名和参数编码为一个联合类型，
 * 确保请求中的方法名和参数类型一一对应。
 */
export type PdfEngineMethodRequestBody = {
  [P in PdfEngineMethodName]: {
    name: P;
    args: PdfEngineMethodArgs<P>;
  };
}[PdfEngineMethodName];

/**
 * 类型化的执行请求（泛型版本）
 *
 * 用于在编译时约束特定的方法名和参数类型。
 */
export type SpecificExecuteRequest<M extends PdfEngineMethodName> = {
  id: string;
  type: 'ExecuteRequest';
  data: {
    name: M;
    args: PdfEngineMethodArgs<M>;
  };
};

/**
 * 引擎方法返回值类型
 *
 * 从所有引擎方法的 TaskReturn 中提取 'result' 类型的 value，
 * 用于构造 ExecuteResponse 的成功响应。
 */
type PdfEngineMethodResultValue = Extract<
  {
    [P in PdfEngineMethodName]: TaskReturn<PdfEngineMethodReturnType<P>>;
  }[PdfEngineMethodName],
  { type: 'result' }
>['value'];

/** 引擎方法响应体：成功（result）或失败（error） */
export type PdfEngineMethodResponseBody =
  | { type: 'result'; value: PdfEngineMethodResultValue }
  | { type: 'error'; value: PdfEngineError };

// ======================== 消息类型定义 ========================

/**
 * 中止任务请求
 *
 * 主线程发送此消息以取消指定的执行中任务。
 */
export interface AbortRequest {
  /** 消息唯一标识 */
  id: string;
  /** 消息类型 */
  type: 'AbortRequest';
}

/**
 * 执行引擎方法请求
 *
 * 主线程发送此消息以在 Worker 中调用指定的 PdfEngine 方法。
 */
export interface ExecuteRequest {
  /** 消息唯一标识 */
  id: string;
  /** 消息类型 */
  type: 'ExecuteRequest';
  /** 方法调用详情（方法名 + 参数） */
  data: PdfEngineMethodRequestBody;
}

/**
 * 执行引擎方法的响应
 *
 * Worker 将方法执行的结果或错误通过此消息返回主线程。
 */
export type ExecuteResponse = {
  /** 对应请求的唯一标识 */
  id: string;
  /** 消息类型 */
  type: 'ExecuteResponse';
  /** 响应体（成功结果或错误信息） */
  data: PdfEngineMethodResponseBody;
};

/**
 * 任务进度通知
 *
 * Worker 在任务执行过程中发送此消息以报告进度，
 * 例如批量渲染时每完成一页就发送一次进度。
 */
export interface ExecuteProgress<T = unknown> {
  /** 对应请求的唯一标识 */
  id: string;
  /** 消息类型 */
  type: 'ExecuteProgress';
  /** 进度数据（类型取决于具体的引擎方法） */
  data: T;
}

/**
 * 引擎就绪响应
 *
 * Worker 初始化引擎完成后发送此消息，通知主线程可以开始发送请求。
 */
export interface ReadyResponse {
  /** 消息标识（固定为 '0'） */
  id: string;
  /** 消息类型 */
  type: 'ReadyResponse';
}

/** 主线程 → Worker 的请求联合类型 */
export type Request = ExecuteRequest | AbortRequest;
/** Worker → 主线程的响应联合类型 */
export type Response = ExecuteResponse | ReadyResponse | ExecuteProgress;

// ======================== 引擎运行器 ========================

const LOG_SOURCE = 'WebWorkerEngineRunner';
const LOG_CATEGORY = 'Engine';

/**
 * Worker 端引擎运行器
 *
 * 在 Web Worker 线程中运行，负责：
 *   1. 持有 PdfEngine 实例
 *   2. 监听主线程消息
 *   3. 分发请求到对应的引擎方法
 *   4. 将结果通过 postMessage 返回主线程
 *
 * 子类需要：
 *   - 在适当的时候创建 PdfEngine 实例并赋值给 this.engine
 *   - 调用 ready() 方法通知主线程引擎已就绪
 */
export class EngineRunner {
  /** PdfEngine 实例，子类在初始化完成后赋值 */
  engine: PdfEngine | undefined;

  /**
   * 创建运行器实例
   * @param logger - 日志记录器，默认为 NoopLogger（不输出日志）
   */
  constructor(public logger: Logger = new NoopLogger()) {}

  /**
   * 开始监听主线程消息
   *
   * 绑定 self.onmessage 处理函数，接收主线程发来的请求。
   */
  listen() {
    self.onmessage = (evt: MessageEvent<Request>) => {
      return this.handle(evt);
    };
  }

  /**
   * 处理主线程消息
   *
   * 根据消息类型分发到对应的处理逻辑。
   * 目前仅处理 ExecuteRequest，AbortRequest 暂未实现。
   */
  handle(evt: MessageEvent<Request>) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'webworker receive message event: ', evt.data);
    try {
      const request = evt.data as Request;
      switch (request.type) {
        case 'ExecuteRequest':
          this.execute(request);
          break;
      }
    } catch (e) {
      this.logger.info(
        LOG_SOURCE,
        LOG_CATEGORY,
        'webworker met error when processing message event:',
        e,
      );
    }
  }

  /**
   * 通知主线程引擎已就绪
   *
   * 调用此方法后：
   *   1. 开始监听主线程消息
   *   2. 发送 ReadyResponse 通知主线程
   *
   * 子类在引擎初始化完成后必须调用此方法。
   *
   * @protected
   */
  ready() {
    this.listen();

    this.respond({
      id: '0',
      type: 'ReadyResponse',
    });
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'runner is ready');
  }

  /**
   * 将引擎方法的 TaskReturn 绑定到 Worker 消息响应
   *
   * TaskReturn 提供两个回调通道：
   *   - onProgress：进度回调，转发为 ExecuteProgress 消息
   *   - wait：完成回调，转发为 ExecuteResponse（result 或 error）
   *
   * @param requestId - 原始请求的 ID，用于关联请求和响应
   * @param task - 引擎方法返回的 TaskReturn 对象
   */
  private handleTask<T extends PdfEngineMethodReturnType<PdfEngineMethodName>>(
    requestId: string,
    task: T,
  ) {
    // 绑定进度回调：每次进度更新都发送 ExecuteProgress
    task.onProgress((progress) => {
      const response: ExecuteProgress = {
        id: requestId,
        type: 'ExecuteProgress',
        data: progress,
      };
      this.respond(response);
    });

    // 绑定完成回调：成功或失败都发送 ExecuteResponse
    task.wait(
      (result) => {
        const response: ExecuteResponse = {
          id: requestId,
          type: 'ExecuteResponse',
          data: {
            type: 'result',
            value: result as PdfEngineMethodResultValue,
          },
        };
        this.respond(response);
      },
      (error) => {
        const response: ExecuteResponse = {
          id: requestId,
          type: 'ExecuteResponse',
          data: {
            type: 'error',
            value: error,
          },
        };
        this.respond(response);
      },
    );
  }

  /**
   * 执行引擎方法
   *
   * 根据请求中的方法名，调用引擎对应的方法，
   * 并将返回的 TaskReturn 交给 handleTask 处理。
   *
   * 错误处理：
   *   - 引擎未初始化：返回 NotReady 错误
   *   - 方法不存在：返回 NotSupport 错误
   *
   * @param request - 执行请求
   *
   * @protected
   */
  execute = async (request: ExecuteRequest) => {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'runner start exeucte request');

    // 检查引擎是否已初始化
    if (!this.engine) {
      const error: PdfEngineError = {
        type: 'reject',
        reason: {
          code: PdfErrorCode.NotReady,
          message: 'engine has not started yet',
        },
      };
      const response: ExecuteResponse = {
        id: request.id,
        type: 'ExecuteResponse',
        data: {
          type: 'error',
          value: error,
        },
      };
      this.respond(response);
      return;
    }

    const engine = this.engine;
    const { name, args } = request.data;

    // 检查方法是否存在
    if (!engine[name]) {
      const error: PdfEngineError = {
        type: 'reject',
        reason: {
          code: PdfErrorCode.NotSupport,
          message: `engine method ${name} is not supported yet`,
        },
      };
      const response: ExecuteResponse = {
        id: request.id,
        type: 'ExecuteResponse',
        data: {
          type: 'error',
          value: error,
        },
      };
      this.respond(response);
      return;
    }

    // 根据方法名分发到对应的引擎方法
    // 使用 switch-case 确保类型安全和穷举检查
    switch (name) {
      case 'isSupport':
        this.handleTask(request.id, engine.isSupport!(...args));
        return;
      case 'destroy':
        this.handleTask(request.id, engine.destroy!(...args));
        return;
      case 'openDocumentUrl':
        this.handleTask(request.id, engine.openDocumentUrl!(...args));
        return;
      case 'openDocumentBuffer':
        this.handleTask(request.id, engine.openDocumentBuffer!(...args));
        return;
      case 'getDocPermissions':
        this.handleTask(request.id, engine.getDocPermissions!(...args));
        return;
      case 'getDocUserPermissions':
        this.handleTask(request.id, engine.getDocUserPermissions!(...args));
        return;
      case 'getMetadata':
        this.handleTask(request.id, engine.getMetadata!(...args));
        return;
      case 'setMetadata':
        this.handleTask(request.id, engine.setMetadata!(...args));
        return;
      case 'getBookmarks':
        this.handleTask(request.id, engine.getBookmarks!(...args));
        return;
      case 'setBookmarks':
        this.handleTask(request.id, engine.setBookmarks!(...args));
        return;
      case 'deleteBookmarks':
        this.handleTask(request.id, engine.deleteBookmarks!(...args));
        return;
      case 'getSignatures':
        this.handleTask(request.id, engine.getSignatures!(...args));
        return;
      case 'renderPage':
        this.handleTask(request.id, engine.renderPage!(...args));
        return;
      case 'renderPageRect':
        this.handleTask(request.id, engine.renderPageRect!(...args));
        return;
      case 'renderPageRaw':
        this.handleTask(request.id, engine.renderPageRaw!(...args));
        return;
      case 'renderPageRectRaw':
        this.handleTask(request.id, engine.renderPageRectRaw!(...args));
        return;
      case 'renderPageAnnotation':
        this.handleTask(request.id, engine.renderPageAnnotation!(...args));
        return;
      case 'renderPageAnnotations':
        this.handleTask(request.id, engine.renderPageAnnotations!(...args));
        return;
      case 'renderPageAnnotationsRaw':
        this.handleTask(request.id, engine.renderPageAnnotationsRaw!(...args));
        return;
      case 'renderThumbnail':
        this.handleTask(request.id, engine.renderThumbnail!(...args));
        return;
      case 'getAllAnnotations':
        this.handleTask(request.id, engine.getAllAnnotations!(...args));
        return;
      case 'getPageAnnotations':
        this.handleTask(request.id, engine.getPageAnnotations!(...args));
        return;
      case 'getDocumentJavaScriptActions':
        this.handleTask(request.id, engine.getDocumentJavaScriptActions!(...args));
        return;
      case 'getPageAnnoWidgets':
        this.handleTask(request.id, engine.getPageAnnoWidgets!(...args));
        return;
      case 'getPageWidgetJavaScriptActions':
        this.handleTask(request.id, engine.getPageWidgetJavaScriptActions!(...args));
        return;
      case 'createPageAnnotation':
        this.handleTask(request.id, engine.createPageAnnotation!(...args));
        return;
      case 'updatePageAnnotation':
        this.handleTask(request.id, engine.updatePageAnnotation!(...args));
        return;
      case 'removePageAnnotation':
        this.handleTask(request.id, engine.removePageAnnotation!(...args));
        return;
      case 'getPageTextRects':
        this.handleTask(request.id, engine.getPageTextRects!(...args));
        return;
      case 'searchAllPages':
        this.handleTask(request.id, engine.searchAllPages!(...args));
        return;
      case 'closeDocument':
        this.handleTask(request.id, engine.closeDocument!(...args));
        return;
      case 'closeAllDocuments':
        this.handleTask(request.id, engine.closeAllDocuments!(...args));
        return;
      case 'saveAsCopy':
        this.handleTask(request.id, engine.saveAsCopy!(...args));
        return;
      case 'getAttachments':
        this.handleTask(request.id, engine.getAttachments!(...args));
        return;
      case 'addAttachment':
        this.handleTask(request.id, engine.addAttachment!(...args));
        return;
      case 'removeAttachment':
        this.handleTask(request.id, engine.removeAttachment!(...args));
        return;
      case 'readAttachmentContent':
        this.handleTask(request.id, engine.readAttachmentContent!(...args));
        return;
      case 'setFormFieldValue':
        this.handleTask(request.id, engine.setFormFieldValue!(...args));
        return;
      case 'setFormFieldState':
        this.handleTask(request.id, engine.setFormFieldState!(...args));
        return;
      case 'renameWidgetField':
        this.handleTask(request.id, engine.renameWidgetField!(...args));
        return;
      case 'shareWidgetField':
        this.handleTask(request.id, engine.shareWidgetField!(...args));
        return;
      case 'flattenPage':
        this.handleTask(request.id, engine.flattenPage!(...args));
        return;
      case 'extractPages':
        this.handleTask(request.id, engine.extractPages!(...args));
        return;
      case 'createDocument':
        this.handleTask(request.id, engine.createDocument!(...args));
        return;
      case 'importPages':
        this.handleTask(request.id, engine.importPages!(...args));
        return;
      case 'deletePage':
        this.handleTask(request.id, engine.deletePage!(...args));
        return;
      case 'extractText':
        this.handleTask(request.id, engine.extractText!(...args));
        return;
      case 'redactTextInRects':
        this.handleTask(request.id, engine.redactTextInRects!(...args));
        return;
      case 'applyRedaction':
        this.handleTask(request.id, engine.applyRedaction!(...args));
        return;
      case 'applyAllRedactions':
        this.handleTask(request.id, engine.applyAllRedactions!(...args));
        return;
      case 'flattenAnnotation':
        this.handleTask(request.id, engine.flattenAnnotation!(...args));
        return;
      case 'exportAnnotationAppearanceAsPdf':
        this.handleTask(request.id, engine.exportAnnotationAppearanceAsPdf!(...args));
        return;
      case 'exportAnnotationsAppearanceAsPdf':
        this.handleTask(request.id, engine.exportAnnotationsAppearanceAsPdf!(...args));
        return;
      case 'getTextSlices':
        this.handleTask(request.id, engine.getTextSlices!(...args));
        return;
      case 'getPageGlyphs':
        this.handleTask(request.id, engine.getPageGlyphs!(...args));
        return;
      case 'getPageGeometry':
        this.handleTask(request.id, engine.getPageGeometry!(...args));
        return;
      case 'getPageTextRuns':
        this.handleTask(request.id, engine.getPageTextRuns!(...args));
        return;
      case 'merge':
        this.handleTask(request.id, engine.merge!(...args));
        return;
      case 'mergePages':
        this.handleTask(request.id, engine.mergePages!(...args));
        return;
      case 'preparePrintDocument':
        this.handleTask(request.id, engine.preparePrintDocument!(...args));
        return;
      case 'setDocumentEncryption':
        this.handleTask(request.id, engine.setDocumentEncryption(...args));
        return;
      case 'removeEncryption':
        this.handleTask(request.id, engine.removeEncryption(...args));
        return;
      case 'unlockOwnerPermissions':
        this.handleTask(request.id, engine.unlockOwnerPermissions(...args));
        return;
      case 'isEncrypted':
        this.handleTask(request.id, engine.isEncrypted(...args));
        return;
      case 'isOwnerUnlocked':
        this.handleTask(request.id, engine.isOwnerUnlocked(...args));
        return;
      default:
        // 理论上不会到达此处（前面已检查方法是否存在），作为兜底保护
        const error: PdfEngineError = {
          type: 'reject',
          reason: {
            code: PdfErrorCode.NotSupport,
            message: `engine method ${name} is not supported`,
          },
        };
        const response: ExecuteResponse = {
          id: request.id,
          type: 'ExecuteResponse',
          data: {
            type: 'error',
            value: error,
          },
        };
        this.respond(response);
        return;
    }
  };

  /**
   * 向主线程发送响应消息
   *
   * @param response - 要发送的响应消息
   *
   * @protected
   */
  respond(response: Response) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'runner respond: ', response);
    self.postMessage(response);
  }
}
