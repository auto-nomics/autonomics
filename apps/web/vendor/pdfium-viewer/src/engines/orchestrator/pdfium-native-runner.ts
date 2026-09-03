/**
 * @file PdfiumNativeRunner — Web Worker 内的 PDFium 原生引擎运行器
 *
 * 整体作用：
 *   本文件实现了在 Web Worker 线程中运行 PDFium WASM 引擎的运行器。
 *   它是 RemoteExecutor（主线程代理）的 Worker 侧对应组件。
 *
 * 通信协议：
 *   主线程通过 postMessage 发送 WorkerRequest，Worker 侧处理后
 *   通过 postMessage 返回 WorkerResponse。消息格式如下：
 *
 *   Request（主线程 → Worker）：
 *     { id, type: 'execute' | 'init', method?, args?, wasmUrl? }
 *
 *   Response（Worker → 主线程）：
 *     { id, type: 'result' | 'error' | 'progress' | 'ready', data?, error?, progress? }
 *
 * 生命周期：
 *   1. Worker 启动 → 监听 'wasmInit' 消息
 *   2. 收到 'wasmInit' → 下载 WASM → 初始化 PdfiumNative → 发送 'ready'
 *   3. 收到 'execute' → 调用 PdfiumNative 的方法 → 返回 result/error/progress
 */

import { Logger, NoopLogger, Task, TaskError, PdfErrorCode } from '../../models';
import { PdfiumNative } from '../pdfium/engine';
import { init } from '../../pdfium';

const LOG_SOURCE = 'PdfiumNativeRunner';
const LOG_CATEGORY = 'Worker';

/**
 * 主线程发来的请求消息格式
 *
 * @property id       - 请求唯一标识，用于匹配响应
 * @property type     - 消息类型：'init'（初始化）| 'execute'（执行方法）
 * @property method   - 要调用的 PdfiumNative 方法名（type='execute' 时必填）
 * @property args     - 方法参数数组（将被展开传递给方法）
 * @property wasmUrl  - WASM 文件 URL（type='init' 时必填）
 */
export interface WorkerRequest {
  id: string;
  type: 'execute' | 'init';
  method?: string;
  args?: any[];
  wasmUrl?: string;
}

/**
 * Worker 返回给主线程的响应消息格式
 *
 * @property id       - 对应请求的唯一标识
 * @property type     - 响应类型：
 *   - 'result'   — 方法执行成功，data 包含返回值
 *   - 'error'    — 方法执行失败，error 包含错误信息
 *   - 'progress' — 进度更新（用于批量操作）
 *   - 'ready'    — Worker 初始化完成，可以接受执行请求
 */
export interface WorkerResponse {
  id: string;
  type: 'result' | 'error' | 'progress' | 'ready';
  data?: any;
  error?: TaskError<any>;
  progress?: any;
}

/**
 * PdfiumNativeRunner — Worker 线程内的 PDFium 运行器
 *
 * 职责：
 *   - 在 Worker 线程中初始化 PDFium WASM 模块
 *   - 监听主线程消息，分发到对应的 PdfiumNative 方法
 *   - 将 PdfiumNative 方法的返回值（包括 Task 的进度）回传给主线程
 *   - 管理活跃 Task 的生命周期（支持进度回传和中止）
 */
export class PdfiumNativeRunner {
  native: PdfiumNative | null = null;
  logger: Logger;
  private activeTasks = new Map<string, Task<any, any>>();

  constructor(logger?: Logger) {
    this.logger = logger ?? new NoopLogger();
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'PdfiumNativeRunner created');
  }

  /**
   * Initialize PDFium with WASM binary
   */
  async prepare(wasmBinary: ArrayBuffer, logger?: Logger): Promise<void> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'Preparing PDFium...');

    try {
      const module = await init({ wasmBinary });

      // PdfiumNative initializes PDFium in its constructor
      this.native = new PdfiumNative(module, { logger: logger ?? this.logger });

      this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'PDFium initialized successfully');
    } catch (error) {
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, 'Failed to initialize PDFium:', error);
      throw error;
    }
  }

  /**
   * Start listening for messages
   */
  listen(): void {
    self.onmessage = (evt: MessageEvent<WorkerRequest>) => {
      this.handle(evt);
    };
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'Listening for messages');
  }

  /**
   * Handle incoming messages
   */
  handle(evt: MessageEvent<WorkerRequest>): void {
    const request = evt.data;
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'Received message:', request.type);

    try {
      switch (request.type) {
        case 'init':
          this.handleInit(request);
          break;
        case 'execute':
          this.handleExecute(request);
          break;
        default:
          this.logger.warn(LOG_SOURCE, LOG_CATEGORY, 'Unknown message type:', request.type);
      }
    } catch (error) {
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, 'Error handling message:', error);
      this.respond({
        id: request.id,
        type: 'error',
        error: {
          type: 'reject',
          reason: { code: PdfErrorCode.Unknown, message: String(error) },
        },
      });
    }
  }

  /**
   * Handle initialization request
   */
  private async handleInit(request: WorkerRequest): Promise<void> {
    if (!request.wasmUrl) {
      this.respond({
        id: request.id,
        type: 'error',
        error: {
          type: 'reject',
          reason: { code: PdfErrorCode.Unknown, message: 'Missing wasmUrl' },
        },
      });
      return;
    }

    try {
      // Fetch WASM binary
      const response = await fetch(request.wasmUrl);
      const wasmBinary = await response.arrayBuffer();

      await this.prepare(wasmBinary);
      this.respond({
        id: request.id,
        type: 'ready',
      });
    } catch (error) {
      this.respond({
        id: request.id,
        type: 'error',
        error: {
          type: 'reject',
          reason: { code: PdfErrorCode.Unknown, message: String(error) },
        },
      });
    }
  }

  /**
   * Handle method execution request
   */
  private async handleExecute(request: WorkerRequest): Promise<void> {
    if (!this.native) {
      this.respond({
        id: request.id,
        type: 'error',
        error: {
          type: 'reject',
          reason: { code: PdfErrorCode.NotReady, message: 'PDFium not initialized' },
        },
      });
      return;
    }

    if (!request.method) {
      this.respond({
        id: request.id,
        type: 'error',
        error: {
          type: 'reject',
          reason: { code: PdfErrorCode.Unknown, message: 'Missing method name' },
        },
      });
      return;
    }

    const method = request.method;
    const args = request.args ?? [];

    // Check if method exists
    if (!(method in this.native) || typeof (this.native as any)[method] !== 'function') {
      this.respond({
        id: request.id,
        type: 'error',
        error: {
          type: 'reject',
          reason: { code: PdfErrorCode.NotSupport, message: `Method ${method} not supported` },
        },
      });
      return;
    }

    try {
      this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Executing method: ${method}`);

      // Execute the method
      const result = (this.native as any)[method](...args);

      // If result is a Task, handle it specially
      if (result && typeof result === 'object' && 'wait' in result) {
        const task = result as Task<any, any>;
        this.activeTasks.set(request.id, task);

        // Listen for progress
        task.onProgress((progress) => {
          this.respond({
            id: request.id,
            type: 'progress',
            progress,
          });
        });

        // Wait for result
        task.wait(
          (data) => {
            this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Method ${method} resolved`);
            this.respond({
              id: request.id,
              type: 'result',
              data,
            });
            this.activeTasks.delete(request.id);
          },
          (error) => {
            this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Method ${method} failed:`, error);
            this.respond({
              id: request.id,
              type: 'error',
              error,
            });
            this.activeTasks.delete(request.id);
          },
        );
      } else {
        // Synchronous result
        this.respond({
          id: request.id,
          type: 'result',
          data: result,
        });
      }
    } catch (error) {
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, `Error executing ${method}:`, error);
      this.respond({
        id: request.id,
        type: 'error',
        error: {
          type: 'reject',
          reason: { code: PdfErrorCode.Unknown, message: String(error) },
        },
      });
    }
  }

  /**
   * Send response back to main thread
   */
  private respond(response: WorkerResponse): void {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'Sending response:', response.type);
    self.postMessage(response);
  }

  /**
   * Ready notification
   */
  ready(): void {
    this.listen();
    this.respond({
      id: '0',
      type: 'ready',
    });
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'Runner is ready');
  }
}
