/**
 * @file engine.ts - PDFium WASM 渲染引擎核心实现
 *
 * 本文件是 PDF 渲染引擎的核心模块，定义了 PdfiumNative 类。
 * PdfiumNative 封装了所有与 PDFium WASM 模块的交互操作，包括：
 *   - PDF 文档的打开、创建、关闭
 *   - 页面渲染（整页渲染、区域渲染、缩略图渲染）
 *   - 注解（Annotation）的增删改查
 *   - 书签（Bookmark）的读写
 *   - 表单字段（Form Field）的读取与填写
 *   - 文本提取与搜索
 *   - 元数据（Metadata）的读写
 *   - 附件（Attachment）管理
 *   - 签名（Signature）查询
 *   - JavaScript 动作（JavaScript Action）读取
 *
 * 所有 WASM 指针操作和 PDFium C API 调用均在此类中完成，
 * 上层通过 IPdfiumExecutor 接口调用，无需关心底层 WASM 细节。
 */

// ======================== 类型与模型导入 ========================
// 从 models 模块导入所有 PDF 相关的类型定义和数据模型
import {
  BatchProgress,
  ImageDataLike,         // 图像数据接口，类似 ImageData 但可跨平台使用
  IPdfiumExecutor,       // PDF 引擎执行器接口，定义所有 PDF 操作的统一契约
  PdfActionObject,       // PDF 动作对象（如跳转、URI 等）
  PdfAnnotationObject,   // PDF 注解基础对象
  PdfTextRectObject,     // 文本矩形区域对象
  PdfAnnotationSubtype,  // 注解子类型枚举（如 Highlight、Ink、Stamp 等）
  PdfLinkAnnoObject,     // 链接注解对象
  PdfWidgetAnnoObject,   // 表单控件（Widget）注解对象
  PdfLinkTarget,         // 链接目标对象
  PdfZoomMode,           // 缩放模式枚举
  Logger,                // 日志接口
  NoopLogger,            // 空操作日志（默认不记录日志）
  SearchResult,          // 文本搜索结果对象
  PdfDestinationObject,  // PDF 目标位置对象（用于书签跳转等）
  PdfBookmarkObject,     // PDF 书签对象
  PdfDocumentObject,     // PDF 文档对象（包含页面列表、权限等）
  PdfPageObject,         // PDF 页面对象（包含尺寸、旋转等信息）
  PdfActionType,         // PDF 动作类型枚举
  Rotation,              // 页面旋转角度枚举（0/90/180/270）
  PDF_FORM_FIELD_FLAG,   // 表单字段标志位枚举
  PDF_FORM_FIELD_TYPE,   // 表单字段类型枚举（文本框、复选框、下拉框等）
  PdfWidgetAnnoOption,   // 表单控件选项（用于下拉框、列表框）
  PdfFileAttachmentAnnoObject, // 文件附件注解对象
  Rect,                  // 矩形区域
  Size,                  // 尺寸（宽高）
  PdfAttachmentObject,   // 文档级附件对象
  PdfUnsupportedAnnoObject,    // 不支持的注解类型对象
  PdfTextAnnoObject,     // 文本注解对象（便签式注解）
  PdfSignatureObject,    // 签名对象
  PdfInkAnnoObject,      // 墨迹注解对象（手写批注）
  PdfInkListObject,      // 墨迹笔画列表对象
  Position,              // 坐标位置（x, y）
  PdfStampAnnoObject,    // 图章注解对象
  PdfCircleAnnoObject,   // 圆形注解对象
  PdfSquareAnnoObject,   // 方形注解对象
  PdfFreeTextAnnoObject, // 自由文本注解对象
  PdfCaretAnnoObject,    // 插入符（Caret）注解对象
  PdfRedactAnnoObject,   // 涂黑（Redact）注解对象
  PdfSquigglyAnnoObject, // 波浪线下划线注解对象
  PdfStrikeOutAnnoObject, // 删除线注解对象
  PdfUnderlineAnnoObject, // 下划线注解对象
  PdfFile,               // PDF 文件数据对象（包含 id 和 content）
  PdfSegmentObject,      // 文本段对象
  AppearanceMode,        // 注解外观模式（Normal/Rollover/Down）
  PdfImageObject,        // PDF 页面中的图像对象
  PdfPageObjectType,     // 页面对象类型枚举（Text/Path/Image/Form 等）
  PdfPathObject,         // PDF 页面中的路径对象
  PdfFormObject,         // PDF 页面中的表单 XObject 对象
  PdfPolygonAnnoObject,  // 多边形注解对象
  PdfPolylineAnnoObject, // 折线注解对象
  PdfLineAnnoObject,     // 线段注解对象
  PdfHighlightAnnoObject, // 高亮注解对象
  PdfStampAnnoObjectContents, // 图章注解内容对象
  PdfWidgetAnnoField,    // 表单控件字段基础对象
  PdfTextWidgetAnnoField, // 文本表单控件字段对象
  PdfUnknownWidgetAnnoField, // 未知类型表单控件字段对象
  PdfTransformMatrix,    // 变换矩阵（2D 仿射变换）
  FormFieldValue,        // 表单字段值类型（字符串、布尔值等）
  PdfErrorCode,          // PDF 错误码枚举
  PdfTaskHelper,         // PDF 任务辅助工具（创建 resolve/reject 任务）
  PdfPageFlattenFlag,    // 页面扁平化标志
  PdfPageFlattenResult,  // 页面扁平化结果
  PdfTask,               // PDF 异步任务包装器
  transformRect,         // 矩形变换工具函数
  PdfOpenDocumentBufferOptions, // 打开文档的配置选项（密码、旋转归一化等）
  Task,                  // 通用异步任务类型
  PdfErrorReason,        // PDF 错误原因对象
  TextContext,           // 文本上下文（用于文本提取）
  PdfGlyphObject,        // 字形对象（包含字符信息与位置）
  PdfGlyphSlim,          // 精简字形对象
  PdfPageGeometry,       // 页面几何信息对象
  PdfRun,                // 文本运行（一段连续的同样式文本）
  toIntRect,             // 矩形转整数矩形工具函数
  PdfDocumentJavaScriptActionObject, // 文档级 JavaScript 动作对象
  PdfWidgetJavaScriptActionObject,   // 控件级 JavaScript 动作对象
  PdfJavaScriptActionTrigger,        // JavaScript 动作触发器枚举
  PdfJavaScriptWidgetEventType,      // 控件 JavaScript 事件类型枚举
  PDF_ANNOT_AACTION_EVENT,           // 注解附加动作事件类型枚举
  Quad,                  // 四边形对象（用于文本标记注解）
  PdfAnnotationState,    // 注解状态枚举
  PdfAnnotationStateModel, // 注解状态模型枚举
  quadToRect,            // 四边形转矩形工具函数
  PageTextSlice,         // 页面文本切片对象
  stripPdfUnwantedMarkers, // 清除 PDF 文本中的特殊标记
  rectToQuad,            // 矩形转四边形工具函数
  dateToPdfDate,         // Date 对象转 PDF 日期字符串
  pdfDateToDate,         // PDF 日期字符串转 Date 对象
  PdfAnnotationColorType, // 注解颜色类型枚举
  PdfAnnotationBorderStyle, // 注解边框样式对象
  flagsToNames,          // 注解标志位转名称列表
  PdfAnnotationFlagName, // 注解标志名称枚举
  namesToFlags,          // 名称列表转注解标志位
  PdfAnnotationLineEnding, // 注解线条端点样式枚举
  LinePoints,            // 线段端点坐标对象
  LineEndings,           // 线条端点样式配置对象
  WebColor,              // Web 颜色对象（r, g, b）
  webColorToPdfColor,    // Web 颜色转 PDF 颜色
  PdfColor,              // PDF 颜色对象（值范围 0-255）
  pdfColorToWebColor,    // PDF 颜色转 Web 颜色
  pdfAlphaToWebOpacity,  // PDF Alpha（0-255）转 Web 透明度（0-1）
  webOpacityToPdfAlpha,  // Web 透明度（0-1）转 PDF Alpha（0-255）
  PdfStandardFont,       // PDF 标准字体枚举
  PdfTextAlignment,      // 文本对齐方式枚举
  PdfVerticalAlignment,  // 垂直对齐方式枚举
  AnnotationCreateContext, // 注解创建上下文（创建时传入的额外信息）
  getImageMetadata,      // 获取图像元数据的工具函数
  uuidV4,                // 生成 UUID v4 的工具函数
  PdfAnnotationName,     // 注解名称类型
  PdfAnnotationReplyType, // 注解回复类型枚举
  PdfRenderPageAnnotationOptions, // 页面渲染时注解相关选项
  PdfRedactTextOptions,  // 涂黑文本选项
  PdfFlattenPageOptions, // 页面扁平化选项
  PdfRenderThumbnailOptions, // 缩略图渲染选项
  PdfRenderPageOptions,  // 页面渲染选项（缩放、注解、水印等）
  buildUserToDeviceMatrix, // 构建用户空间到设备空间的变换矩阵
  Matrix,                // 矩阵参数对象（a, b, c, d, e, f）
  PdfMetadataObject,     // PDF 元数据对象（标题、作者、创建日期等）
  PdfPrintOptions,       // 打印选项
  PdfTrappedStatus,      // PDF Trapped 状态枚举
  PdfStampFit,           // 图章适配模式
  PdfAddAttachmentParams, // 添加附件的参数对象
  AnnotationAppearanceMap, // 注解外观映射（按模式索引外观）
  AnnotationAppearances,  // 注解外观集合对象
  AnnotationAppearanceImage, // 注解外观图像对象
  AP_MODE_NORMAL,        // 正常状态外观模式常量
  AP_MODE_ROLLOVER,      // 悬停状态外观模式常量
  AP_MODE_DOWN,          // 按下状态外观模式常量
  PdfFontInfo,           // PDF 字体信息对象
  PdfTextRun,            // 文本运行对象（一段连续文本及其字体信息）
  PdfPageTextRuns,       // 页面文本运行集合
  PdfAlphaColor,         // PDF 带透明度的颜色对象
  PdfBlendMode,          // PDF 混合模式枚举
} from '../../models';

// ======================== 内部工具模块导入 ========================
import { computeFormDrawParams, isValidCustomKey, readArrayBuffer, readString } from './helper';
import { WrappedPdfiumModule } from '../../pdfium';    // PDFium WASM 模块的 TypeScript 封装
import { DocumentContext, PageContext, PdfCache } from './cache'; // 文档/页面上下文与缓存管理
import { MemoryManager } from './core/memory-manager'; // WASM 线性内存管理器
import { WasmPointer } from './types/branded';         // WASM 指针的品牌类型（防止与普通数字混淆）
import { FontFallbackManager, FontFallbackConfig } from './font-fallback'; // 字体回退管理器

/**
 * 位图像素格式枚举
 *
 * 定义渲染输出的像素排列方式。PDFium 渲染时使用这些格式来决定
 * 每个 pixel 占多少字节以及各通道的排列顺序。
 * 注意：PDFium 原生输出的是 BGR 系列（而非 RGB），需要根据格式做转换。
 */
export enum BitmapFormat {
  /** 灰度图，每像素 1 字节 */
  Bitmap_Gray = 1,
  /** BGR 格式，每像素 3 字节（蓝-绿-红） */
  Bitmap_BGR = 2,
  /** BGRx 格式，每像素 4 字节（蓝-绿-红-填充） */
  Bitmap_BGRx = 3,
  /** BGRA 格式，每像素 4 字节（蓝-绿-红-透明度）—— 最常用的渲染格式 */
  Bitmap_BGRA = 4,
}

/**
 * PDF 渲染标志位枚举
 *
 * 用于控制 FPDF_RenderPageBitmap 等渲染函数的行为。
 * 多个标志可以通过位或（|）组合使用。
 */
export enum RenderFlag {
  /** 渲染时包含注解（Annotations） */
  ANNOT = 0x01,
  /** 使用针对 LCD 显示优化的文本渲染（子像素抗锯齿） */
  LCD_TEXT = 0x02,
  /** 不使用某些平台提供的原生文本输出 */
  NO_NATIVETEXT = 0x04,
  /** 灰度输出（不使用彩色） */
  GRAYSCALE = 0x08,
  /** 输出调试信息（需与 Foxit 协商后使用） */
  DEBUG_INFO = 0x80,
  /** 不捕获异常（让异常直接抛出） */
  NO_CATCH = 0x100,
  /** 限制图像缓存大小 */
  RENDER_LIMITEDIMAGECACHE = 0x200,
  /** 始终使用半色调（halftone）进行图像缩放 */
  RENDER_FORCEHALFTONE = 0x400,
  /** 以打印模式渲染（影响某些注解的显示） */
  PRINTING = 0x800,
  /** 以反向字节序渲染（将 BGRA 转为 RGBA） */
  REVERSE_BYTE_ORDER = 0x10,
}

// 日志标识常量，用于在日志中区分引擎模块
const LOG_SOURCE = 'PDFiumEngine';
const LOG_CATEGORY = 'Engine';

/**
 * PDFium 库错误码枚举
 *
 * 对应 FPDF_GetLastError() 返回的错误码。
 * 在文档加载失败、页面操作出错等场景下使用。
 */
export enum PdfiumErrorCode {
  /** 操作成功 */
  Success = 0,
  /** 未知错误 */
  Unknown = 1,
  /** 文件错误（找不到文件、无法读取等） */
  File = 2,
  /** 格式错误（文件不是有效的 PDF） */
  Format = 3,
  /** 密码错误（需要密码或密码不正确） */
  Password = 4,
  /** 安全错误（文档被加密且权限不足） */
  Security = 5,
  /** 页面错误（页面加载或解析失败） */
  Page = 6,
  /** XFA 表单加载失败 */
  XFALoad = 7,
  /** XFA 表单布局失败 */
  XFALayout = 8,
}

/**
 * PdfiumNative 引擎配置选项
 */
export interface PdfiumEngineOptions {
  /** 日志记录器实例，默认使用 NoopLogger（不记录日志） */
  logger?: Logger;
  /**
   * 字体回退（Font Fallback）配置。
   * 当启用时，PDFium 遇到 PDF 中未嵌入的字体时，
   * 会从配置的 URL 请求回退字体，确保文本正确渲染。
   */
  fontFallback?: FontFallbackConfig;
}

/**
 * 基于 PDFium WASM 的 PDF 引擎实现
 *
 * 这是整个 PDF 渲染引擎的核心类，封装了 PDFium C API 的所有 WASM 绑定。
 * 它通过 WASM 模块操作 PDFium 库，提供以下能力：
 *   - 文档生命周期管理（打开、创建、关闭、保存）
 *   - 页面渲染（光栅化为位图）
 *   - 注解完整 CRUD
 *   - 表单字段交互
 *   - 书签与大纲管理
 *   - 文本提取与搜索
 *   - 元数据与附件管理
 *
 * 该类通过 IPdfiumExecutor 接口暴露给上层使用，
 * 所有 WASM 内存指针操作均在此类内部完成。
 */
export class PdfiumNative implements IPdfiumExecutor {
  /**
   * 文档缓存，管理所有已打开的 PDF 文档及其页面上下文。
   * 每个打开的文档由一个 DocumentContext 表示，包含 WASM 中的文档指针、
   * 文件数据指针以及页面缓存。
   */
  private readonly cache: PdfCache;

  /**
   * WASM 线性内存管理器，负责 malloc/free 操作的追踪。
   * 每次分配的内存都会被记录，确保在操作完成后正确释放，
   * 避免 WASM 内存泄漏。
   */
  private readonly memoryManager: MemoryManager;

  /**
   * 内存泄漏检查的定时器 ID。
   * 仅在 debug 模式下启用，每 10 秒调用 memoryManager.checkLeaks()。
   */
  private memoryLeakCheckInterval: number | null = null;

  /** 日志记录器实例 */
  private logger: Logger;

  /**
   * 字体回退管理器实例。
   * 当 PDF 使用了未嵌入的字体时，负责从外部加载回退字体。
   * 仅在配置了 fontFallback 选项时初始化。
   */
  private fontFallbackManager: FontFallbackManager | null = null;

  /**
   * 创建 PdfiumNative 实例并初始化 PDFium 库
   *
   * 初始化流程：
   * 1. 设置日志记录器
   * 2. 创建内存管理器（追踪 WASM 内存分配）
   * 3. 创建文档缓存
   * 4. 在 debug 模式下启用内存泄漏检测定时器
   * 5. 调用 PDFiumExt_Init() 初始化 PDFium 库
   * 6. 如果配置了字体回退，初始化 FontFallbackManager
   *
   * @param pdfiumModule - PDFium WASM 模块的 TypeScript 封装实例
   * @param options - 引擎配置选项（日志、字体回退等）
   */
  constructor(
    private pdfiumModule: WrappedPdfiumModule,
    options: PdfiumEngineOptions = {},
  ) {
    const { logger = new NoopLogger(), fontFallback } = options;

    this.logger = logger;
    // 创建内存管理器，用于安全地管理 WASM 堆内存的分配和释放
    this.memoryManager = new MemoryManager(this.pdfiumModule, this.logger);
    // 创建文档缓存，用于管理已打开的文档和页面资源
    this.cache = new PdfCache(this.pdfiumModule, this.memoryManager);

    // 仅在 debug 日志级别下启动内存泄漏检测定时器（每 10 秒检查一次）
    if (this.logger.isEnabled('debug')) {
      this.memoryLeakCheckInterval = setInterval(() => {
        this.memoryManager.checkLeaks();
      }, 10000) as unknown as number;
    }

    // 调用 PDFium 扩展初始化函数，设置 WASM 环境中的 PDFium 库
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'initialize');
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `Initialize`, 'Begin', 'General');
    this.pdfiumModule.PDFiumExt_Init();
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `Initialize`, 'End', 'General');

    // 如果配置了字体回退，初始化字体回退系统
    // 字体回退允许在 PDF 中使用未嵌入的字体时，从外部 URL 加载替代字体
    if (fontFallback) {
      this.fontFallbackManager = new FontFallbackManager(fontFallback, this.logger);
      this.fontFallbackManager.initialize(this.pdfiumModule);
      this.logger.info(LOG_SOURCE, LOG_CATEGORY, 'Font fallback system enabled');
    }
  }

  /**
   * 销毁引擎实例，释放所有 PDFium 资源。
   *
   * 销毁流程：
   * 1. 禁用字体回退系统
   * 2. 调用 FPDF_DestroyLibrary() 销毁 PDFium 库实例
   * 3. 停止内存泄漏检测定时器
   *
   * @public
   */
  destroy() {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'destroy');
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `Destroy`, 'Begin', 'General');

    // 在销毁 PDFium 库之前，先禁用字体回退系统
    if (this.fontFallbackManager) {
      this.fontFallbackManager.disable();
      this.fontFallbackManager = null;
    }

    // 销毁 PDFium 库，释放所有关联的文档和页面资源
    this.pdfiumModule.FPDF_DestroyLibrary();
    // 停止内存泄漏检测定时器
    if (this.memoryLeakCheckInterval) {
      clearInterval(this.memoryLeakCheckInterval);
      this.memoryLeakCheckInterval = null;
    }
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `Destroy`, 'End', 'General');
    return PdfTaskHelper.resolve(true);
  }

  /**
   * Get the font fallback manager instance
   * Useful for pre-loading fonts or checking stats
   */
  getFontFallbackManager(): FontFallbackManager | null {
    return this.fontFallbackManager;
  }

  /** Write a UTF-16LE (WIDESTRING) to wasm, call `fn(ptr)`, then free. */
  private withWString<T>(value: string, fn: (ptr: number) => T): T {
    // bytes = (len + 1) * 2
    const length = (value.length + 1) * 2;
    const ptr = this.memoryManager.malloc(length);
    try {
      // emscripten runtime exposes stringToUTF16
      this.pdfiumModule.pdfium.stringToUTF16(value, ptr, length);
      return fn(ptr);
    } finally {
      this.memoryManager.free(ptr);
    }
  }

  /** Write a float[] to wasm, call `fn(ptr, count)`, then free. */
  private withFloatArray<T>(
    values: number[] | undefined,
    fn: (ptr: number, count: number) => T,
  ): T {
    const arr = values ?? [];
    const bytes = arr.length * 4;
    const ptr = bytes ? this.memoryManager.malloc(bytes) : WasmPointer(0);
    try {
      if (bytes) {
        for (let i = 0; i < arr.length; i++) {
          this.pdfiumModule.pdfium.setValue(ptr + i * 4, arr[i], 'float');
        }
      }
      return fn(ptr, arr.length);
    } finally {
      if (bytes) this.memoryManager.free(ptr);
    }
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.openDocument}
   *
   * @public
   */
  openDocumentBuffer(file: PdfFile, options?: PdfOpenDocumentBufferOptions) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'openDocumentBuffer', file, options);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `OpenDocumentBuffer`, 'Begin', file.id);

    // Per-document normalizeRotation setting (defaults to false for backwards compatibility)
    const normalizeRotation = options?.normalizeRotation ?? false;

    const array = new Uint8Array(file.content);
    const length = array.length;
    const filePtr = this.memoryManager.malloc(length);
    this.pdfiumModule.pdfium.HEAPU8.set(array, filePtr);

    const docPtr = this.pdfiumModule.FPDF_LoadMemDocument(filePtr, length, options?.password ?? '');

    if (!docPtr) {
      const lastError = this.pdfiumModule.FPDF_GetLastError();
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, `FPDF_LoadMemDocument failed with ${lastError}`);
      this.memoryManager.free(filePtr);
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `OpenDocumentBuffer`, 'End', file.id);

      return PdfTaskHelper.reject<PdfDocumentObject>({
        code: lastError,
        message: `FPDF_LoadMemDocument failed`,
      });
    }

    const pageCount = this.pdfiumModule.FPDF_GetPageCount(docPtr);

    const pages: PdfPageObject[] = [];
    const sizePtr = this.memoryManager.malloc(8);
    for (let index = 0; index < pageCount; index++) {
      // Use normalized size function when normalizeRotation is enabled
      const result = normalizeRotation
        ? this.pdfiumModule.EPDF_GetPageSizeByIndexNormalized(docPtr, index, sizePtr)
        : this.pdfiumModule.FPDF_GetPageSizeByIndexF(docPtr, index, sizePtr);

      if (!result) {
        const lastError = this.pdfiumModule.FPDF_GetLastError();
        this.logger.error(
          LOG_SOURCE,
          LOG_CATEGORY,
          `${normalizeRotation ? 'EPDF_GetPageSizeByIndexNormalized' : 'FPDF_GetPageSizeByIndexF'} failed with ${lastError}`,
        );
        this.memoryManager.free(sizePtr);
        this.pdfiumModule.FPDF_CloseDocument(docPtr);
        this.memoryManager.free(filePtr);
        this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `OpenDocumentBuffer`, 'End', file.id);
        return PdfTaskHelper.reject<PdfDocumentObject>({
          code: lastError,
          message: `${normalizeRotation ? 'EPDF_GetPageSizeByIndexNormalized' : 'FPDF_GetPageSizeByIndexF'} failed`,
        });
      }

      const rotation = this.pdfiumModule.EPDF_GetPageRotationByIndex(docPtr, index) as Rotation;
      const objectNumber = this.pdfiumModule.EPDFDoc_GetPageObjectNumberByIndex(docPtr, index);

      const page = {
        index,
        size: {
          width: this.pdfiumModule.pdfium.getValue(sizePtr, 'float'),
          height: this.pdfiumModule.pdfium.getValue(sizePtr + 4, 'float'),
        },
        rotation,
        objectNumber,
      };

      pages.push(page);
    }
    this.memoryManager.free(sizePtr);

    // Query security state
    const isEncrypted = this.pdfiumModule.EPDF_IsEncrypted(docPtr);
    const isOwnerUnlocked = this.pdfiumModule.EPDF_IsOwnerUnlocked(docPtr);
    const permissions = this.pdfiumModule.FPDF_GetDocPermissions(docPtr);

    const pdfDoc: PdfDocumentObject = {
      id: file.id,
      pageCount,
      pages,
      isEncrypted,
      isOwnerUnlocked,
      permissions,
      normalizedRotation: normalizeRotation,
    };

    this.cache.setDocument(file.id, filePtr, docPtr, normalizeRotation);

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `OpenDocumentBuffer`, 'End', file.id);

    return PdfTaskHelper.resolve(pdfDoc);
  }

  /**
   * Create a new empty PDF document and register it in the cache.
   *
   * @param id - unique document identifier
   * @returns task containing the empty PdfDocumentObject
   */
  public createDocument(id: string): PdfTask<PdfDocumentObject> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'createDocument', id);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'CreateDocument', 'Begin', id);

    const docPtr = this.pdfiumModule.FPDF_CreateNewDocument();
    if (!docPtr) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'CreateDocument', 'End', id);
      return PdfTaskHelper.reject<PdfDocumentObject>({
        code: PdfErrorCode.CantCreateNewDoc,
        message: 'can not create new document',
      });
    }

    const pdfDoc: PdfDocumentObject = {
      id,
      pageCount: 0,
      pages: [],
      isEncrypted: false,
      isOwnerUnlocked: true,
      permissions: 0xffffffff,
      normalizedRotation: false,
    };

    this.cache.setDocument(id, 0, docPtr, false);

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'CreateDocument', 'End', id);
    return PdfTaskHelper.resolve(pdfDoc);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.getMetadata}
   *
   * @public
   */
  getMetadata(doc: PdfDocumentObject): PdfTask<PdfMetadataObject> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getMetadata', doc);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetMetadata`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetMetadata`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const creationRaw = this.readMetaText(ctx.docPtr, 'CreationDate');
    const modRaw = this.readMetaText(ctx.docPtr, 'ModDate');

    const metadata: PdfMetadataObject = {
      title: this.readMetaText(ctx.docPtr, 'Title'),
      author: this.readMetaText(ctx.docPtr, 'Author'),
      subject: this.readMetaText(ctx.docPtr, 'Subject'),
      keywords: this.readMetaText(ctx.docPtr, 'Keywords'),
      producer: this.readMetaText(ctx.docPtr, 'Producer'),
      creator: this.readMetaText(ctx.docPtr, 'Creator'),
      creationDate: creationRaw ? (pdfDateToDate(creationRaw) ?? null) : null,
      modificationDate: modRaw ? (pdfDateToDate(modRaw) ?? null) : null,
      trapped: this.getMetaTrapped(ctx.docPtr),
      custom: this.readAllMeta(ctx.docPtr, true),
    };

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetMetadata`, 'End', doc.id);

    return PdfTaskHelper.resolve(metadata);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.setMetadata}
   *
   * @public
   */
  setMetadata(doc: PdfDocumentObject, meta: Partial<PdfMetadataObject>) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'setMetadata', doc, meta);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'SetMetadata', 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'SetMetadata', 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // Field -> PDF Info key
    const strMap: Array<[keyof PdfMetadataObject, string]> = [
      ['title', 'Title'],
      ['author', 'Author'],
      ['subject', 'Subject'],
      ['keywords', 'Keywords'],
      ['producer', 'Producer'],
      ['creator', 'Creator'],
    ];

    let ok = true;

    // Write string fields (string|null|undefined)
    for (const [field, key] of strMap) {
      const v = meta[field];
      if (v === undefined) continue;
      const s = v === null ? null : (v as string);
      if (!this.setMetaText(ctx.docPtr, key, s)) ok = false;
    }

    // Write date fields (Date|null|undefined)
    const writeDate = (
      field: 'creationDate' | 'modificationDate',
      key: 'CreationDate' | 'ModDate',
    ) => {
      const v = meta[field];
      if (v === undefined) return;
      if (v === null) {
        if (!this.setMetaText(ctx.docPtr, key, null)) ok = false;
        return;
      }
      const d = v as Date;
      const raw = dateToPdfDate(d);
      if (!this.setMetaText(ctx.docPtr, key, raw)) ok = false;
    };

    writeDate('creationDate', 'CreationDate');
    writeDate('modificationDate', 'ModDate');

    if (meta.trapped !== undefined) {
      if (!this.setMetaTrapped(ctx.docPtr, meta.trapped ?? null)) ok = false;
    }

    if (meta.custom !== undefined) {
      for (const [key, value] of Object.entries(meta.custom)) {
        if (!isValidCustomKey(key)) {
          this.logger.warn(LOG_SOURCE, LOG_CATEGORY, 'Invalid custom metadata key skipped', key);
          continue;
        }
        if (!this.setMetaText(ctx.docPtr, key, value ?? null)) ok = false;
      }
    }

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'SetMetadata', 'End', doc.id);

    return ok
      ? PdfTaskHelper.resolve(true)
      : PdfTaskHelper.reject({
          code: PdfErrorCode.Unknown,
          message: 'one or more metadata fields could not be written',
        });
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.getDocPermissions}
   *
   * @public
   */
  getDocPermissions(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getDocPermissions', doc);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `getDocPermissions`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `getDocPermissions`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const permissions = this.pdfiumModule.FPDF_GetDocPermissions(ctx.docPtr);

    return PdfTaskHelper.resolve(permissions);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.getDocUserPermissions}
   *
   * @public
   */
  getDocUserPermissions(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getDocUserPermissions', doc);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `getDocUserPermissions`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `getDocUserPermissions`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const permissions = this.pdfiumModule.FPDF_GetDocUserPermissions(ctx.docPtr);

    return PdfTaskHelper.resolve(permissions);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.getSignatures}
   *
   * @public
   */
  getSignatures(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getSignatures', doc);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetSignatures`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetSignatures`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const signatures: PdfSignatureObject[] = [];

    const count = this.pdfiumModule.FPDF_GetSignatureCount(ctx.docPtr);
    for (let i = 0; i < count; i++) {
      const signatureObjPtr = this.pdfiumModule.FPDF_GetSignatureObject(ctx.docPtr, i);

      const contents = readArrayBuffer(this.pdfiumModule.pdfium, (buffer, bufferSize) => {
        return this.pdfiumModule.FPDFSignatureObj_GetContents(signatureObjPtr, buffer, bufferSize);
      });

      const byteRange = readArrayBuffer(this.pdfiumModule.pdfium, (buffer, bufferSize) => {
        return (
          this.pdfiumModule.FPDFSignatureObj_GetByteRange(signatureObjPtr, buffer, bufferSize) * 4
        );
      });

      const subFilter = readArrayBuffer(this.pdfiumModule.pdfium, (buffer, bufferSize) => {
        return this.pdfiumModule.FPDFSignatureObj_GetSubFilter(signatureObjPtr, buffer, bufferSize);
      });

      const reason = readString(
        this.pdfiumModule.pdfium,
        (buffer, bufferLength) => {
          return this.pdfiumModule.FPDFSignatureObj_GetReason(
            signatureObjPtr,
            buffer,
            bufferLength,
          );
        },
        this.pdfiumModule.pdfium.UTF16ToString,
      );

      const time = readString(
        this.pdfiumModule.pdfium,
        (buffer, bufferLength) => {
          return this.pdfiumModule.FPDFSignatureObj_GetTime(signatureObjPtr, buffer, bufferLength);
        },
        this.pdfiumModule.pdfium.UTF8ToString,
      );

      const docMDP = this.pdfiumModule.FPDFSignatureObj_GetDocMDPPermission(signatureObjPtr);

      signatures.push({
        contents,
        byteRange,
        subFilter,
        reason,
        time,
        docMDP,
      });
    }
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetSignatures`, 'End', doc.id);

    return PdfTaskHelper.resolve(signatures);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.getBookmarks}
   *
   * @public
   */
  getBookmarks(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getBookmarks', doc);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetBookmarks`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `getBookmarks`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const bookmarks = this.readPdfBookmarks(ctx.docPtr, 0);

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetBookmarks`, 'End', doc.id);

    return PdfTaskHelper.resolve({
      bookmarks,
    });
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.setBookmarks}
   *
   * @public
   */
  setBookmarks(doc: PdfDocumentObject, list: PdfBookmarkObject[]) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'setBookmarks', doc, list);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `SetBookmarks`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `SetBookmarks`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // Clear any existing outlines
    if (!this.pdfiumModule.EPDFBookmark_Clear(ctx.docPtr)) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `SetBookmarks`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: 'failed to clear existing bookmarks',
      });
    }

    // Recursive builder
    const build = (parentPtr: number, items: PdfBookmarkObject[]): boolean => {
      let prevChild = 0;
      for (const item of items) {
        // Create
        const bmPtr = this.withWString(item.title ?? '', (wptr) =>
          this.pdfiumModule.EPDFBookmark_AppendChild(ctx.docPtr, parentPtr, wptr),
        );
        if (!bmPtr) return false;

        // Target (optional)
        if (item.target) {
          const ok = this.applyBookmarkTarget(ctx.docPtr, bmPtr, item.target);
          if (!ok) return false;
        }

        // Children
        if (item.children?.length) {
          const ok = build(bmPtr, item.children);
          if (!ok) return false;
        }

        prevChild = bmPtr;
      }
      return true;
    };

    const ok = build(/*top-level*/ 0, list);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `SetBookmarks`, 'End', doc.id);

    if (!ok) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: 'failed to build bookmark tree',
      });
    }
    return PdfTaskHelper.resolve(true);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.deleteBookmarks}
   *
   * @public
   */
  deleteBookmarks(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'deleteBookmarks', doc);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `DeleteBookmarks`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `DeleteBookmarks`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const ok = this.pdfiumModule.EPDFBookmark_Clear(ctx.docPtr);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `DeleteBookmarks`, 'End', doc.id);

    return ok
      ? PdfTaskHelper.resolve(true)
      : PdfTaskHelper.reject({
          code: PdfErrorCode.Unknown,
          message: 'failed to clear bookmarks',
        });
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.renderPage}
   *
   * @public
   */
  renderPageRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageOptions,
  ): PdfTask<ImageDataLike> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderPage', doc, page, options);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `RenderPage`, 'Begin', `${doc.id}-${page.index}`);

    const rect = { origin: { x: 0, y: 0 }, size: page.size };
    const task = this.renderRectEncoded(doc, page, rect, options);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `RenderPage`, 'End', `${doc.id}-${page.index}`);

    return task;
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.renderPageRect}
   *
   * @public
   */
  renderPageRect(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rect: Rect,
    options?: PdfRenderPageOptions,
  ): PdfTask<ImageDataLike> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderPageRect', doc, page, rect, options);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RenderPageRect`,
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const task = this.renderRectEncoded(doc, page, rect, options);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `RenderPageRect`, 'End', `${doc.id}-${page.index}`);

    return task;
  }

  getDocumentJavaScriptActions(
    doc: PdfDocumentObject,
  ): PdfTask<PdfDocumentJavaScriptActionObject[]> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getDocumentJavaScriptActions', doc);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const count = this.pdfiumModule.FPDFDoc_GetJavaScriptActionCount(ctx.docPtr);
    if (count < 0) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: 'failed to read document javascript actions',
      });
    }

    const actions: PdfDocumentJavaScriptActionObject[] = [];
    for (let index = 0; index < count; index++) {
      const actionPtr = this.pdfiumModule.FPDFDoc_GetJavaScriptAction(ctx.docPtr, index);
      if (!actionPtr) continue;

      try {
        const name =
          readString(
            this.pdfiumModule.pdfium,
            (buffer: number, bufferLength) =>
              this.pdfiumModule.FPDFJavaScriptAction_GetName(actionPtr, buffer, bufferLength),
            this.pdfiumModule.pdfium.UTF16ToString,
          ) ?? '';
        const script =
          readString(
            this.pdfiumModule.pdfium,
            (buffer: number, bufferLength) =>
              this.pdfiumModule.FPDFJavaScriptAction_GetScript(actionPtr, buffer, bufferLength),
            this.pdfiumModule.pdfium.UTF16ToString,
          ) ?? '';

        if (!script) continue;

        actions.push({
          id: `document:${index}:${name}`,
          trigger: PdfJavaScriptActionTrigger.DocumentNamed,
          name,
          script,
        });
      } finally {
        this.pdfiumModule.FPDFDoc_CloseJavaScriptAction(actionPtr);
      }
    }

    return PdfTaskHelper.resolve(actions);
  }

  getPageAnnoWidgets(
    doc: PdfDocumentObject,
    page: PdfPageObject,
  ): PdfTask<PdfWidgetAnnoObject[]> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageAnnoWidgets', doc, page);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `GetPageAnnoWidgets`,
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `GetPageAnnoWidgets`,
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const annotationWidgets = this.readPageAnnoWidgets(doc, ctx, page);

    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `GetPageAnnoWidgets`,
      'End',
      `${doc.id}-${page.index}`,
    );

    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      `GetPageAnnoWidgets`,
      `${doc.id}-${page.index}`,
      annotationWidgets,
    );

    return PdfTaskHelper.resolve(annotationWidgets);
  }

  getPageWidgetJavaScriptActions(
    doc: PdfDocumentObject,
    page: PdfPageObject,
  ): PdfTask<PdfWidgetJavaScriptActionObject[]> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageWidgetJavaScriptActions', doc, page);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const actions: PdfWidgetJavaScriptActionObject[] = [];
    ctx.borrowPage(page.index, (pageCtx) => {
      pageCtx.withFormHandle((formHandle) => {
        const annotCount = this.pdfiumModule.FPDFPage_GetAnnotCount(pageCtx.pagePtr);
        for (let i = 0; i < annotCount; i++) {
          pageCtx.withAnnotation(i, (annotPtr) => {
            const subtype = this.pdfiumModule.FPDFAnnot_GetSubtype(
              annotPtr,
            ) as PdfAnnotationObject['type'];
            if (subtype !== PdfAnnotationSubtype.WIDGET) return;

            let annotationId = this.getAnnotString(annotPtr, 'NM');
            if (!annotationId) {
              annotationId = uuidV4();
              this.setAnnotString(annotPtr, 'NM', annotationId);
            }

            const fieldName =
              readString(
                this.pdfiumModule.pdfium,
                (buffer: number, bufferLength) =>
                  this.pdfiumModule.FPDFAnnot_GetFormFieldName(
                    formHandle,
                    annotPtr,
                    buffer,
                    bufferLength,
                  ),
                this.pdfiumModule.pdfium.UTF16ToString,
              ) ?? '';

            const eventConfigs = [
              {
                event: PDF_ANNOT_AACTION_EVENT.KEY_STROKE,
                eventType: PdfJavaScriptWidgetEventType.Keystroke,
                trigger: PdfJavaScriptActionTrigger.WidgetKeystroke,
              },
              {
                event: PDF_ANNOT_AACTION_EVENT.FORMAT,
                eventType: PdfJavaScriptWidgetEventType.Format,
                trigger: PdfJavaScriptActionTrigger.WidgetFormat,
              },
              {
                event: PDF_ANNOT_AACTION_EVENT.VALIDATE,
                eventType: PdfJavaScriptWidgetEventType.Validate,
                trigger: PdfJavaScriptActionTrigger.WidgetValidate,
              },
              {
                event: PDF_ANNOT_AACTION_EVENT.CALCULATE,
                eventType: PdfJavaScriptWidgetEventType.Calculate,
                trigger: PdfJavaScriptActionTrigger.WidgetCalculate,
              },
            ] as const;

            for (const config of eventConfigs) {
              const script =
                readString(
                  this.pdfiumModule.pdfium,
                  (buffer: number, bufferLength) =>
                    this.pdfiumModule.FPDFAnnot_GetFormAdditionalActionJavaScript(
                      formHandle,
                      annotPtr,
                      config.event,
                      buffer,
                      bufferLength,
                    ),
                  this.pdfiumModule.pdfium.UTF16ToString,
                ) ?? '';

              if (!script) continue;

              actions.push({
                id: `widget:${page.index}:${annotationId}:${config.eventType}`,
                trigger: config.trigger,
                eventType: config.eventType,
                pageIndex: page.index,
                annotationId,
                fieldName,
                script,
              });
            }
          });
        }
      });
    });

    return PdfTaskHelper.resolve(actions);
  }

  regenerateWidgetAppearances(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationIds: string[],
  ): PdfTask<boolean> {
    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const idSet = new Set(annotationIds);
    let regenerated = 0;

    ctx.borrowPage(page.index, (pageCtx) => {
      const count = this.pdfiumModule.FPDFPage_GetAnnotCount(pageCtx.pagePtr);
      for (let i = 0; i < count; i++) {
        pageCtx.withAnnotation(i, (annotPtr) => {
          const nm = this.getAnnotString(annotPtr, 'NM');
          if (nm && idSet.has(nm)) {
            this.pdfiumModule.EPDFAnnot_GenerateFormFieldAP(annotPtr);
            regenerated++;
          }
        });
      }
      if (regenerated > 0) {
        this.pdfiumModule.FPDFPage_GenerateContent(pageCtx.pagePtr);
      }
    });

    return PdfTaskHelper.resolve(regenerated > 0);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.getPageAnnotations}
   *
   * @public
   */
  getPageAnnotations(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageAnnotations', doc, page);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `GetPageAnnotations`,
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `GetPageAnnotations`,
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const annotations = this.readPageAnnotations(doc, ctx, page);

    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `GetPageAnnotations`,
      'End',
      `${doc.id}-${page.index}`,
    );

    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      `GetPageAnnotations`,
      `${doc.id}-${page.index}`,
      annotations,
    );

    return PdfTaskHelper.resolve(annotations);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.createPageAnnotation}
   *
   * @public
   */
  createPageAnnotation<A extends PdfAnnotationObject>(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: A,
    context?: AnnotationCreateContext<A>,
  ): PdfTask<string> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'createPageAnnotation', doc, page, annotation);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `CreatePageAnnotation`,
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `CreatePageAnnotation`,
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);

    let annotationPtr: number;
    let widgetFormInfoPtr: number | undefined;
    let widgetFormHandle: number | undefined;
    if (annotation.type === PdfAnnotationSubtype.WIDGET) {
      const widget = annotation as unknown as PdfWidgetAnnoObject;
      widgetFormInfoPtr = this.pdfiumModule.PDFiumExt_OpenFormFillInfo();
      widgetFormHandle = this.pdfiumModule.PDFiumExt_InitFormFillEnvironment(
        ctx.docPtr,
        widgetFormInfoPtr,
      );
      this.pdfiumModule.FORM_OnAfterLoadPage(pageCtx.pagePtr, widgetFormHandle);
      const fieldName = widget.field?.name ?? '';
      annotationPtr = this.withWString(fieldName, (namePtr) =>
        this.pdfiumModule.EPDFPage_CreateFormField(
          pageCtx.pagePtr,
          widgetFormHandle!,
          widget.field.type,
          namePtr,
        ),
      );
    } else {
      annotationPtr = this.pdfiumModule.EPDFPage_CreateAnnot(pageCtx.pagePtr, annotation.type);
    }

    if (!annotationPtr) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `CreatePageAnnotation`,
        'End',
        `${doc.id}-${page.index}`,
      );
      pageCtx.release();

      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantCreateAnnot,
        message: 'can not create annotation with specified type',
      });
    }

    if (!annotation.id) {
      annotation.id = uuidV4();
    }

    if (!this.setAnnotString(annotationPtr, 'NM', annotation.id)) {
      this.pdfiumModule.FPDFPage_CloseAnnot(annotationPtr);
      pageCtx.release();
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantSetAnnotString,
        message: 'can not set the name of the annotation',
      });
    }

    if (!this.setPageAnnoRect(doc, page, annotationPtr, annotation.rect)) {
      this.pdfiumModule.FPDFPage_CloseAnnot(annotationPtr);
      pageCtx.release();
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `CreatePageAnnotation`,
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantSetAnnotRect,
        message: 'can not set the rect of the annotation',
      });
    }

    // Rotate vertices for PDF storage if the annotation has rotation
    const saveAnnotation = this.prepareAnnotationForSave(annotation);

    let isSucceed = false;
    switch (saveAnnotation.type) {
      case PdfAnnotationSubtype.INK:
        isSucceed = this.addInkStroke(
          doc,
          page,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfInkAnnoObject,
        );
        break;
      case PdfAnnotationSubtype.STAMP:
        isSucceed = this.addStampContent(
          doc,
          ctx.docPtr,
          page,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfStampAnnoObject,
          context,
        );
        break;
      case PdfAnnotationSubtype.TEXT:
        isSucceed = this.addTextContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfTextAnnoObject,
        );
        break;
      case PdfAnnotationSubtype.FREETEXT:
        isSucceed = this.addFreeTextContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfFreeTextAnnoObject,
        );
        break;
      case PdfAnnotationSubtype.LINE:
        isSucceed = this.addLineContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfLineAnnoObject,
        );
        break;
      case PdfAnnotationSubtype.POLYLINE:
      case PdfAnnotationSubtype.POLYGON:
        isSucceed = this.addPolyContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfPolygonAnnoObject | PdfPolylineAnnoObject,
        );
        break;
      case PdfAnnotationSubtype.CIRCLE:
      case PdfAnnotationSubtype.SQUARE:
        isSucceed = this.addShapeContent(doc, page, pageCtx.pagePtr, annotationPtr, saveAnnotation);
        break;
      case PdfAnnotationSubtype.UNDERLINE:
      case PdfAnnotationSubtype.STRIKEOUT:
      case PdfAnnotationSubtype.SQUIGGLY:
      case PdfAnnotationSubtype.HIGHLIGHT:
        isSucceed = this.addTextMarkupContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfHighlightAnnoObject,
        );
        break;
      case PdfAnnotationSubtype.LINK:
        isSucceed = this.addLinkContent(
          doc,
          page,
          ctx.docPtr,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfLinkAnnoObject,
        );
        break;
      case PdfAnnotationSubtype.CARET:
        isSucceed = this.addCaretContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfCaretAnnoObject,
        );
        break;
      case PdfAnnotationSubtype.REDACT:
        isSucceed = this.addRedactContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotationPtr,
          saveAnnotation as PdfRedactAnnoObject,
        );
        break;
      case PdfAnnotationSubtype.WIDGET: {
        const widget = saveAnnotation as PdfWidgetAnnoObject;
        if (widgetFormHandle !== undefined) {
          switch (widget.field.type) {
            case PDF_FORM_FIELD_TYPE.TEXTFIELD:
              isSucceed = this.addTextFieldContent(widgetFormHandle, annotationPtr, widget);
              break;
            case PDF_FORM_FIELD_TYPE.CHECKBOX:
            case PDF_FORM_FIELD_TYPE.RADIOBUTTON:
              isSucceed = this.addToggleFieldContent(widgetFormHandle, annotationPtr, widget);
              break;
            case PDF_FORM_FIELD_TYPE.COMBOBOX:
            case PDF_FORM_FIELD_TYPE.LISTBOX:
              isSucceed = this.addChoiceFieldContent(widgetFormHandle, annotationPtr, widget);
              break;
          }
        }
        break;
      }
    }

    if (widgetFormHandle !== undefined) {
      this.pdfiumModule.FORM_OnBeforeClosePage(pageCtx.pagePtr, widgetFormHandle);
      this.pdfiumModule.PDFiumExt_ExitFormFillEnvironment(widgetFormHandle);
    }
    if (widgetFormInfoPtr !== undefined) {
      this.pdfiumModule.PDFiumExt_CloseFormFillInfo(widgetFormInfoPtr);
    }

    if (!isSucceed) {
      // FPDFPage_RemoveAnnot's C signature is (FPDF_PAGE, int index), not
      // (FPDF_PAGE, FPDF_ANNOTATION). Passing the annotation pointer here is
      // interpreted as an out-of-range index and silently no-ops, leaving the
      // half-built annotation in the page. Use the name-based remover instead,
      // which matches how /NM was set above.
      this.removeAnnotationByName(pageCtx.pagePtr, annotation.id);
      this.pdfiumModule.FPDFPage_CloseAnnot(annotationPtr);
      pageCtx.release();
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `CreatePageAnnotation`,
        'End',
        `${doc.id}-${page.index}`,
      );

      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantSetAnnotContent,
        message: 'can not add content of the annotation',
      });
    }

    if (annotation.type === PdfAnnotationSubtype.WIDGET) {
      this.pdfiumModule.EPDFAnnot_GenerateFormFieldAP(annotationPtr);
    } else if (annotation.blendMode !== undefined) {
      this.pdfiumModule.EPDFAnnot_GenerateAppearanceWithBlend(annotationPtr, annotation.blendMode);
    } else {
      this.pdfiumModule.EPDFAnnot_GenerateAppearance(annotationPtr);
    }

    this.pdfiumModule.FPDFPage_GenerateContent(pageCtx.pagePtr);

    this.pdfiumModule.FPDFPage_CloseAnnot(annotationPtr);
    pageCtx.release();
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `CreatePageAnnotation`,
      'End',
      `${doc.id}-${page.index}`,
    );

    return PdfTaskHelper.resolve<string>(annotation.id);
  }

  /**
   * Update an existing page annotation in-place
   *
   *  • Locates the annot by page-local index (`annotation.id`)
   *  • Re-writes its /Rect and type-specific payload
   *  • Calls FPDFPage_GenerateContent so the new appearance is rendered
   *
   * @returns PdfTask<boolean>  –  true on success
   */
  updatePageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
    options?: { regenerateAppearance?: boolean },
  ): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'updatePageAnnotation', doc, page, annotation);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      'UpdatePageAnnotation',
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        'UpdatePageAnnotation',
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    const annotPtr = this.getAnnotationByName(pageCtx.pagePtr, annotation.id);
    if (!annotPtr) {
      pageCtx.release();
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        'UpdatePageAnnotation',
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({ code: PdfErrorCode.NotFound, message: 'annotation not found' });
    }

    /* 1 ── (re)set bounding-box ────────────────────────────────────────────── */
    if (!this.setPageAnnoRect(doc, page, annotPtr, annotation.rect)) {
      this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);
      pageCtx.release();
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        'UpdatePageAnnotation',
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantSetAnnotRect,
        message: 'failed to move annotation',
      });
    }

    /* 2 ── For rotated vertex types, rotate vertices for PDF storage ────────── */
    const saveAnnotation = this.prepareAnnotationForSave(annotation);

    /* 3 ── wipe previous payload and rebuild fresh one ─────────────────────── */
    let ok = false;
    switch (saveAnnotation.type) {
      /* ── Ink ─────────────────────────────────────────────────────────────── */
      case PdfAnnotationSubtype.INK: {
        /* clear every existing stroke first */
        if (!this.pdfiumModule.FPDFAnnot_RemoveInkList(annotPtr)) break;
        ok = this.addInkStroke(
          doc,
          page,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfInkAnnoObject,
        );
        break;
      }

      /* ── Stamp ───────────────────────────────────────────────────────────── */
      case PdfAnnotationSubtype.STAMP: {
        ok = this.addStampContent(
          doc,
          ctx.docPtr,
          page,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfStampAnnoObject,
        );
        break;
      }

      case PdfAnnotationSubtype.TEXT: {
        ok = this.addTextContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfTextAnnoObject,
        );
        break;
      }

      /* ── Free text ────────────────────────────────────────────────────────── */
      case PdfAnnotationSubtype.FREETEXT: {
        ok = this.addFreeTextContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfFreeTextAnnoObject,
        );
        break;
      }

      /* ── Shape ───────────────────────────────────────────────────────────── */
      case PdfAnnotationSubtype.CIRCLE:
      case PdfAnnotationSubtype.SQUARE: {
        ok = this.addShapeContent(doc, page, pageCtx.pagePtr, annotPtr, saveAnnotation);
        break;
      }

      /* ── Line ─────────────────────────────────────────────────────────────── */
      case PdfAnnotationSubtype.LINE: {
        ok = this.addLineContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfLineAnnoObject,
        );
        break;
      }

      /* ── Polygon / Polyline ───────────────────────────────────────────────── */
      case PdfAnnotationSubtype.POLYGON:
      case PdfAnnotationSubtype.POLYLINE: {
        ok = this.addPolyContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfPolygonAnnoObject | PdfPolylineAnnoObject,
        );
        break;
      }

      /* ── Text-markup family ──────────────────────────────────────────────── */
      case PdfAnnotationSubtype.HIGHLIGHT:
      case PdfAnnotationSubtype.UNDERLINE:
      case PdfAnnotationSubtype.STRIKEOUT:
      case PdfAnnotationSubtype.SQUIGGLY: {
        ok = this.addTextMarkupContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfHighlightAnnoObject,
        );
        break;
      }

      /* ── Link ─────────────────────────────────────────────────────────────── */
      case PdfAnnotationSubtype.LINK: {
        ok = this.addLinkContent(
          doc,
          page,
          ctx.docPtr,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfLinkAnnoObject,
        );
        break;
      }

      /* ── Caret ────────────────────────────────────────────────────────────── */
      case PdfAnnotationSubtype.CARET: {
        ok = this.addCaretContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfCaretAnnoObject,
        );
        break;
      }

      /* ── Redact ───────────────────────────────────────────────────────────── */
      case PdfAnnotationSubtype.REDACT: {
        ok = this.addRedactContent(
          doc,
          page,
          pageCtx.pagePtr,
          annotPtr,
          saveAnnotation as PdfRedactAnnoObject,
        );
        break;
      }

      /* ── Widget (form field) ─────────────────────────────────────────────── */
      case PdfAnnotationSubtype.WIDGET: {
        const widget = saveAnnotation as PdfWidgetAnnoObject;
        pageCtx.withFormHandle((formHandle) => {
          switch (widget.field.type) {
            case PDF_FORM_FIELD_TYPE.TEXTFIELD:
              ok = this.addTextFieldContent(formHandle, annotPtr, widget);
              break;
            case PDF_FORM_FIELD_TYPE.CHECKBOX:
            case PDF_FORM_FIELD_TYPE.RADIOBUTTON:
              ok = this.addToggleFieldContent(formHandle, annotPtr, widget);
              break;
            case PDF_FORM_FIELD_TYPE.COMBOBOX:
            case PDF_FORM_FIELD_TYPE.LISTBOX:
              ok = this.addChoiceFieldContent(formHandle, annotPtr, widget);
              break;
          }
        });
        break;
      }

      /* ── Unsupported edits – fall through to error ───────────────────────── */
      default:
        ok = false;
    }

    /* 4 ── regenerate appearance if payload was changed ───────────────────── */
    if (ok && options?.regenerateAppearance !== false) {
      if (annotation.type === PdfAnnotationSubtype.WIDGET) {
        this.pdfiumModule.EPDFAnnot_GenerateFormFieldAP(annotPtr);
      } else if (annotation.blendMode !== undefined) {
        this.pdfiumModule.EPDFAnnot_GenerateAppearanceWithBlend(annotPtr, annotation.blendMode);
      } else {
        this.pdfiumModule.EPDFAnnot_GenerateAppearance(annotPtr);
      }
      this.pdfiumModule.FPDFPage_GenerateContent(pageCtx.pagePtr);
    }

    /* 5 ── tidy-up native handles ──────────────────────────────────────────── */
    this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);
    pageCtx.release();
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      'UpdatePageAnnotation',
      'End',
      `${doc.id}-${page.index}`,
    );

    return ok
      ? PdfTaskHelper.resolve<boolean>(true)
      : PdfTaskHelper.reject<boolean>({
          code: PdfErrorCode.CantSetAnnotContent,
          message: 'failed to update annotation',
        });
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.removePageAnnotation}
   *
   * @public
   */
  removePageAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'removePageAnnotation', doc, page, annotation);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RemovePageAnnotation`,
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `RemovePageAnnotation`,
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    let result = false;
    result = this.removeAnnotationByName(pageCtx.pagePtr, annotation.id);
    if (!result) {
      this.logger.error(
        LOG_SOURCE,
        LOG_CATEGORY,
        `FPDFPage_RemoveAnnot Failed`,
        `${doc.id}-${page.index}`,
      );
    } else {
      result = this.pdfiumModule.FPDFPage_GenerateContent(pageCtx.pagePtr);
      if (!result) {
        this.logger.error(
          LOG_SOURCE,
          LOG_CATEGORY,
          `FPDFPage_GenerateContent Failed`,
          `${doc.id}-${page.index}`,
        );
      }
    }

    pageCtx.release();

    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RemovePageAnnotation`,
      'End',
      `${doc.id}-${page.index}`,
    );
    return PdfTaskHelper.resolve(result);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.getPageTextRects}
   *
   * @public
   */
  getPageTextRects(doc: PdfDocumentObject, page: PdfPageObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageTextRects', doc, page);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `GetPageTextRects`,
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `GetPageTextRects`,
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    const textPagePtr = this.pdfiumModule.FPDFText_LoadPage(pageCtx.pagePtr);

    const textRects = this.readPageTextRects(page, pageCtx.docPtr, pageCtx.pagePtr, textPagePtr);

    this.pdfiumModule.FPDFText_ClosePage(textPagePtr);
    pageCtx.release();

    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `GetPageTextRects`,
      'End',
      `${doc.id}-${page.index}`,
    );
    return PdfTaskHelper.resolve(textRects);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.renderThumbnail}
   *
   * @public
   */
  renderThumbnailRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderThumbnailOptions,
  ): PdfTask<ImageDataLike> {
    const { scaleFactor = 1, ...rest } = options ?? {};
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderThumbnail', doc, page, options);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RenderThumbnail`,
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `RenderThumbnail`,
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const result = this.renderPageRaw(doc, page, {
      scaleFactor: Math.max(scaleFactor, 0.5),
      ...rest,
    });
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `RenderThumbnail`, 'End', `${doc.id}-${page.index}`);

    return result;
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.getAttachments}
   *
   * @public
   */
  getAttachments(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getAttachments', doc);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetAttachments`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetAttachments`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const attachments: PdfAttachmentObject[] = [];

    const count = this.pdfiumModule.FPDFDoc_GetAttachmentCount(ctx.docPtr);
    for (let i = 0; i < count; i++) {
      const attachment = this.readPdfAttachment(ctx.docPtr, i);
      attachments.push(attachment);
    }

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `GetAttachments`, 'End', doc.id);
    return PdfTaskHelper.resolve(attachments);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.addAttachment}
   *
   * @public
   */
  addAttachment(doc: PdfDocumentObject, params: PdfAddAttachmentParams): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'addAttachment', doc, params?.name);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `AddAttachment`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `AddAttachment`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const { name, description, mimeType, data } = params ?? {};
    if (!name) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `AddAttachment`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.NotFound,
        message: 'attachment name is required',
      });
    }
    if (!data || (data instanceof Uint8Array ? data.byteLength === 0 : data.byteLength === 0)) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `AddAttachment`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.NotFound,
        message: 'attachment data is empty',
      });
    }

    // 1) Create the attachment handle (also inserts into the EmbeddedFiles name tree).
    const attachmentPtr = this.withWString(name, (wNamePtr) =>
      this.pdfiumModule.FPDFDoc_AddAttachment(ctx.docPtr, wNamePtr),
    );

    if (!attachmentPtr) {
      // Most likely: duplicate name in the name tree.
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `AddAttachment`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: `An attachment named "${name}" already exists`,
      });
    }

    this.withWString(description, (wDescriptionPtr) =>
      this.pdfiumModule.EPDFAttachment_SetDescription(attachmentPtr, wDescriptionPtr),
    );

    this.pdfiumModule.EPDFAttachment_SetSubtype(attachmentPtr, mimeType);

    // 3) Copy data into WASM memory and call SetFile (this stores bytes and fills Size/CreationDate/CheckSum)
    const u8 = data instanceof Uint8Array ? data : new Uint8Array(data);
    const len = u8.byteLength;

    const contentPtr = this.memoryManager.malloc(len);
    try {
      this.pdfiumModule.pdfium.HEAPU8.set(u8, contentPtr);
      const ok = this.pdfiumModule.FPDFAttachment_SetFile(
        attachmentPtr,
        ctx.docPtr,
        contentPtr,
        len,
      );
      if (!ok) {
        this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `AddAttachment`, 'End', doc.id);
        return PdfTaskHelper.reject({
          code: PdfErrorCode.Unknown,
          message: 'failed to write attachment bytes',
        });
      }
    } finally {
      this.memoryManager.free(contentPtr);
    }

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `AddAttachment`, 'End', doc.id);
    return PdfTaskHelper.resolve<boolean>(true);
  }

  /**
   * 删除 PDF 文档中的指定附件（嵌入文件）。
   *
   * 流程：
   * 1. 从缓存中获取文档上下文（包含 docPtr 指针）
   * 2. 校验附件索引是否在有效范围内 [0, count)
   * 3. 调用 PDFium 的 FPDFDoc_DeleteAttachment 删除对应索引的附件
   *
   * {@inheritDoc @embedpdf/models!PdfEngine.removeAttachment}
   *
   * @param doc - 要操作的 PDF 文档对象
   * @param attachment - 要删除的附件对象（主要使用其 index 属性）
   * @returns 成功返回 true，失败返回错误码
   *
   * @public
   */
  removeAttachment(doc: PdfDocumentObject, attachment: PdfAttachmentObject): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'deleteAttachment', doc, attachment);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `DeleteAttachment`, 'Begin', doc.id);

    // 从缓存中获取文档上下文，ctx 中包含 PDFium 的文档指针 docPtr
    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `DeleteAttachment`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // 通过 PDFium API 获取当前文档的附件总数，用于索引范围校验
    const count = this.pdfiumModule.FPDFDoc_GetAttachmentCount(ctx.docPtr);
    if (attachment.index < 0 || attachment.index >= count) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `DeleteAttachment`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: `attachment index ${attachment.index} out of range`,
      });
    }

    // 调用 PDFium API 删除指定索引位置的附件
    const ok = this.pdfiumModule.FPDFDoc_DeleteAttachment(ctx.docPtr, attachment.index);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `DeleteAttachment`, 'End', doc.id);

    if (!ok) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: 'failed to delete attachment',
      });
    }
    return PdfTaskHelper.resolve<boolean>(true);
  }

  /**
   * 读取 PDF 文档中指定附件的二进制内容。
   *
   * 实现流程：
   * 1. 通过 FPDFDoc_GetAttachment 获取附件指针
   * 2. 第一次调用 FPDFAttachment_GetFile（buffer 传 0）获取附件大小
   * 3. 分配 WASM 堆内存，第二次调用读取实际内容
   * 4. 将 WASM 内存中的字节数据逐字节拷贝到 JS ArrayBuffer 中
   *
   * 注意：此处需要手动在 WASM 线性内存和 JS 堆之间拷贝数据，
   * 因为 PDFium 的附件 API 直接操作 WASM 内存空间。
   *
   * {@inheritDoc @embedpdf/models!PdfEngine.readAttachmentContent}
   *
   * @param doc - PDF 文档对象
   * @param attachment - 附件对象（使用其 index 属性定位附件）
   * @returns 包含附件二进制内容的 ArrayBuffer
   *
   * @public
   */
  readAttachmentContent(doc: PdfDocumentObject, attachment: PdfAttachmentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'readAttachmentContent', doc, attachment);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ReadAttachmentContent`, 'Begin', doc.id);

    // 获取文档上下文
    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ReadAttachmentContent`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // 通过索引获取 PDFium 附件对象的指针
    const attachmentPtr = this.pdfiumModule.FPDFDoc_GetAttachment(ctx.docPtr, attachment.index);

    // 在 WASM 堆中分配 4 字节空间，用于接收附件文件的大小（i32 类型）
    const sizePtr = this.memoryManager.malloc(4);

    // 第一次调用：buffer 传 0（空指针），仅获取附件大小，不读取内容
    // PDFium 会将文件大小写入 sizePtr 指向的内存位置
    if (!this.pdfiumModule.FPDFAttachment_GetFile(attachmentPtr, 0, 0, sizePtr)) {
      this.memoryManager.free(sizePtr);
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ReadAttachmentContent`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantReadAttachmentSize,
        message: 'can not read attachment size',
      });
    }

    // 从 WASM 内存中读取附件大小，使用 >>> 0 确保无符号解释
    const size = this.pdfiumModule.pdfium.getValue(sizePtr, 'i32') >>> 0;

    // 根据附件大小分配足够的 WASM 堆内存来存放附件内容
    const contentPtr = this.memoryManager.malloc(size);

    // 第二次调用：传入实际 buffer 指针和大小，读取附件的完整二进制内容
    if (!this.pdfiumModule.FPDFAttachment_GetFile(attachmentPtr, contentPtr, size, sizePtr)) {
      this.memoryManager.free(sizePtr);
      this.memoryManager.free(contentPtr);
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ReadAttachmentContent`, 'End', doc.id);

      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantReadAttachmentContent,
        message: 'can not read attachment content',
      });
    }

    // 将 WASM 线性内存中的附件数据逐字节拷贝到 JS 的 ArrayBuffer 中
    // 这是一个常见的 WASM ↔ JS 数据桥接操作
    const buffer = new ArrayBuffer(size);
    const view = new DataView(buffer);
    for (let i = 0; i < size; i++) {
      // contentPtr + i 计算每个字节在 WASM 内存中的绝对地址
      view.setInt8(i, this.pdfiumModule.pdfium.getValue(contentPtr + i, 'i8'));
    }

    // 释放之前分配的 WASM 内存，避免内存泄漏
    this.memoryManager.free(sizePtr);
    this.memoryManager.free(contentPtr);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ReadAttachmentContent`, 'End', doc.id);

    return PdfTaskHelper.resolve(buffer);
  }

  /**
   * 设置表单字段的值。
   *
   * 支持三种类型的表单字段值设置：
   * - text：文本输入框，通过"全选 → 替换选区"的方式设置文本内容
   * - selection：下拉框/列表框的选项选中状态
   * - checked：复选框/单选按钮的勾选状态（通过模拟回车键切换）
   *
   * 关键步骤：
   * 1. 获取页面上下文并加载表单句柄
   * 2. 通过注解名称定位到具体的表单控件注解
   * 3. 设置焦点到该注解（FORM_SetFocusedAnnot）
   * 4. 根据字段类型执行不同的值设置逻辑
   * 5. 释放焦点并重新生成表单字段的外观流（AP）
   *
   * {@inheritDoc @embedpdf/models!PdfEngine.setFormFieldValue}
   *
   * @param doc - PDF 文档对象
   * @param page - 页面对象，用于定位表单字段所在的页
   * @param annotation - 表单控件注解对象
   * @param value - 要设置的值（文本 / 选中项 / 勾选状态）
   *
   * @public
   */
  setFormFieldValue(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    value: FormFieldValue,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'SetFormFieldValue', doc, annotation, value);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `SetFormFieldValue`,
      'Begin',
      `${doc.id}-${annotation.id}`,
    );

    // 获取文档上下文
    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'SetFormFieldValue', 'document is not opened');
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `SetFormFieldValue`,
        'End',
        `${doc.id}-${annotation.id}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // 获取页面上下文，acquirePage 会加载页面到 PDFium 中并返回 pagePtr
    const pageCtx = ctx.acquirePage(page.index);
    try {
      // 使用表单句柄进行操作，formHandle 是 PDFium 表单交互的核心句柄
      return pageCtx.withFormHandle((formHandle) => {
        // 通过注解的唯一名称（annotation.id）在页面上查找对应的 PDFium 注解指针
        const annotationPtr = this.getAnnotationByName(pageCtx.pagePtr, annotation.id);
        if (!annotationPtr) {
          return PdfTaskHelper.reject({
            code: PdfErrorCode.NotFound,
            message: 'annotation not found',
          });
        }

        try {
          // 先将焦点设置到目标注解上，后续的表单操作都需要焦点在目标控件上
          if (!this.pdfiumModule.FORM_SetFocusedAnnot(formHandle, annotationPtr)) {
            return PdfTaskHelper.reject({
              code: PdfErrorCode.CantFocusAnnot,
              message: 'failed to set focused annotation',
            });
          }

          // 根据表单字段的值类型执行不同的设置逻辑
          switch (value.kind) {
            // 文本类型：先全选已有文本，再替换为新文本
            case 'text': {
              // 全选当前文本字段中的所有文字
              if (!this.pdfiumModule.FORM_SelectAllText(formHandle, pageCtx.pagePtr)) {
                this.pdfiumModule.FORM_ForceToKillFocus(formHandle);
                return PdfTaskHelper.reject({
                  code: PdfErrorCode.CantSelectText,
                  message: 'failed to select all text',
                });
              }
              // 将 JS 字符串转为 UTF-16 编码并写入 WASM 内存
              // 每个字符占 2 字节，加 1 是为末尾的 null 终止符
              const length = 2 * (value.text.length + 1);
              const textPtr = this.memoryManager.malloc(length);
              this.pdfiumModule.pdfium.stringToUTF16(value.text, textPtr, length);
              // 用新文本替换已选中的文本内容
              this.pdfiumModule.FORM_ReplaceSelection(formHandle, pageCtx.pagePtr, textPtr);
              this.memoryManager.free(textPtr);
              break;
            }
            // 选项类型：设置下拉框/列表框中某一项的选中状态
            case 'selection': {
              if (
                !this.pdfiumModule.FORM_SetIndexSelected(
                  formHandle,
                  pageCtx.pagePtr,
                  value.index,
                  value.isSelected,
                )
              ) {
                this.pdfiumModule.FORM_ForceToKillFocus(formHandle);
                return PdfTaskHelper.reject({
                  code: PdfErrorCode.CantSelectOption,
                  message: 'failed to set index selected',
                });
              }
              break;
            }
            // 勾选类型：切换复选框/单选按钮的选中状态
            case 'checked': {
              // 查询当前控件是否已被勾选
              const rawChecked = this.pdfiumModule.FPDFAnnot_IsChecked(formHandle, annotationPtr);
              const currentlyChecked = !!rawChecked;
              // 仅在状态需要切换时才执行操作
              if (currentlyChecked !== value.checked) {
                // 通过模拟回车键（0x0D）来切换复选框状态
                // 这是 PDFium 中切换勾选状态的标准方式
                const kReturn = 0x0d;
                if (!this.pdfiumModule.FORM_OnChar(formHandle, pageCtx.pagePtr, kReturn, 0)) {
                  this.pdfiumModule.FORM_ForceToKillFocus(formHandle);
                  return PdfTaskHelper.reject({
                    code: PdfErrorCode.CantCheckField,
                    message: 'failed to set field checked',
                  });
                }
              }
              break;
            }
          }

          // 操作完成后强制释放焦点
          this.pdfiumModule.FORM_ForceToKillFocus(formHandle);
          // 重新生成表单字段的外观流（Appearance Stream），
          // 使 PDF 阅读器能正确渲染更新后的表单状态
          this.pdfiumModule.EPDFAnnot_GenerateFormFieldAP(annotationPtr);
          return PdfTaskHelper.resolve<boolean>(true);
        } finally {
          // 关闭注解句柄，释放 PDFium 内部资源
          this.pdfiumModule.FPDFPage_CloseAnnot(annotationPtr);
        }
      });
    } finally {
      // 释放页面上下文引用
      pageCtx.release();
    }
  }

  /**
   * 设置表单字段的状态（比 setFormFieldValue 更完整的状态设置方法）。
   *
   * 支持以下表单字段类型的状态设置：
   * - TEXTFIELD：文本输入框，通过"全选 → 替换选区"方式设置文本
   * - CHECKBOX / RADIOBUTTON：复选框/单选按钮，通过模拟回车键切换选中状态
   * - COMBOBOX：下拉组合框，设置选中的选项索引
   * - LISTBOX：列表框，逐项设置每个选项的选中状态（支持多选）
   *
   * 注意：复选框和单选按钮在状态设置后不重新生成外观流，
   * 因为它们的勾选标记由 PDFium 内部管理。
   *
   * {@inheritDoc @embedpdf/models!PdfEngine.setFormFieldState}
   *
   * @param doc - PDF 文档对象
   * @param page - 页面对象
   * @param annotation - 表单控件注解对象
   * @param field - 表单字段的完整状态（包含类型、值、选项等）
   *
   * @public
   */
  setFormFieldState(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    field: PdfWidgetAnnoField,
  ) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'SetFormFieldState', doc, annotation, field);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `SetFormFieldState`,
      'Begin',
      `${doc.id}-${annotation.id}`,
    );

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'SetFormFieldState', 'document is not opened');
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `SetFormFieldState`,
        'End',
        `${doc.id}-${annotation.id}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // 获取页面上下文并加载页面
    const pageCtx = ctx.acquirePage(page.index);
    try {
      // 获取表单句柄，用于执行表单交互操作
      return pageCtx.withFormHandle((formHandle) => {
        // 通过注解名称查找 PDFium 注解指针
        const annotationPtr = this.getAnnotationByName(pageCtx.pagePtr, annotation.id);
        if (!annotationPtr) {
          return PdfTaskHelper.reject({
            code: PdfErrorCode.NotFound,
            message: 'annotation not found',
          });
        }

        try {
          // 将焦点设置到目标注解
          if (!this.pdfiumModule.FORM_SetFocusedAnnot(formHandle, annotationPtr)) {
            return PdfTaskHelper.reject({
              code: PdfErrorCode.CantFocusAnnot,
              message: 'failed to set focused annotation',
            });
          }

          // 根据字段类型执行不同的状态设置逻辑
          switch (field.type) {
            // 文本输入框：全选后替换文本
            case PDF_FORM_FIELD_TYPE.TEXTFIELD: {
              if (!this.pdfiumModule.FORM_SelectAllText(formHandle, pageCtx.pagePtr)) {
                this.pdfiumModule.FORM_ForceToKillFocus(formHandle);
                return PdfTaskHelper.reject({
                  code: PdfErrorCode.CantSelectText,
                  message: 'failed to select all text',
                });
              }
              // 将文本转为 UTF-16 并写入 WASM 内存
              const length = 2 * (field.value.length + 1);
              const textPtr = this.memoryManager.malloc(length);
              this.pdfiumModule.pdfium.stringToUTF16(field.value, textPtr, length);
              // 用新文本替换选中的内容
              this.pdfiumModule.FORM_ReplaceSelection(formHandle, pageCtx.pagePtr, textPtr);
              this.memoryManager.free(textPtr);
              break;
            }

            // 复选框和单选按钮：通过模拟回车键切换选中状态
            case PDF_FORM_FIELD_TYPE.CHECKBOX:
            case PDF_FORM_FIELD_TYPE.RADIOBUTTON: {
              // 获取当前控件的勾选状态
              const currentlyChecked = !!this.pdfiumModule.FPDFAnnot_IsChecked(
                formHandle,
                annotationPtr,
              );
              // 判断目标状态：通过比较 field.value 和 annotation.exportValue 确定
              // exportValue 是 PDF 中单选/复选框的导出值（如 "Yes"、"1" 等）
              const desiredChecked =
                annotation.exportValue != null && field.value === annotation.exportValue;
              // 仅在需要切换时才模拟按键
              if (currentlyChecked !== desiredChecked) {
                const kReturn = 0x0d; // 回车键的虚拟键码
                if (!this.pdfiumModule.FORM_OnChar(formHandle, pageCtx.pagePtr, kReturn, 0)) {
                  this.pdfiumModule.FORM_ForceToKillFocus(formHandle);
                  return PdfTaskHelper.reject({
                    code: PdfErrorCode.CantCheckField,
                    message: 'failed to set field checked',
                  });
                }
              }
              break;
            }

            // 下拉组合框：只选中第一个 isSelected 的选项
            case PDF_FORM_FIELD_TYPE.COMBOBOX: {
              // 在选项列表中找到第一个被标记为选中的选项
              const selectedIndex = field.options.findIndex((opt) => opt.isSelected);
              if (selectedIndex >= 0) {
                if (
                  !this.pdfiumModule.FORM_SetIndexSelected(
                    formHandle,
                    pageCtx.pagePtr,
                    selectedIndex,
                    true,
                  )
                ) {
                  this.pdfiumModule.FORM_ForceToKillFocus(formHandle);
                  return PdfTaskHelper.reject({
                    code: PdfErrorCode.CantSelectOption,
                    message: 'failed to set index selected',
                  });
                }
              }
              break;
            }

            // 列表框：逐项设置每个选项的选中状态（支持多选）
            case PDF_FORM_FIELD_TYPE.LISTBOX: {
              for (let i = 0; i < field.options.length; i++) {
                if (
                  !this.pdfiumModule.FORM_SetIndexSelected(
                    formHandle,
                    pageCtx.pagePtr,
                    i,
                    field.options[i].isSelected,
                  )
                ) {
                  this.pdfiumModule.FORM_ForceToKillFocus(formHandle);
                  return PdfTaskHelper.reject({
                    code: PdfErrorCode.CantSelectOption,
                    message: 'failed to set index selected',
                  });
                }
              }
              break;
            }

            default:
              break;
          }

          // 操作完成后释放焦点
          this.pdfiumModule.FORM_ForceToKillFocus(formHandle);

          // 对于非复选框/单选按钮的字段，需要重新生成外观流
          // 复选框和单选按钮的外观由 PDFium 内部管理，不需要手动生成
          if (
            field.type !== PDF_FORM_FIELD_TYPE.CHECKBOX &&
            field.type !== PDF_FORM_FIELD_TYPE.RADIOBUTTON
          ) {
            this.pdfiumModule.EPDFAnnot_GenerateFormFieldAP(annotationPtr);
          }

          this.logger.perf(
            LOG_SOURCE,
            LOG_CATEGORY,
            `SetFormFieldState`,
            'End',
            `${doc.id}-${annotation.id}`,
          );

          return PdfTaskHelper.resolve<boolean>(true);
        } finally {
          // 关闭注解句柄
          this.pdfiumModule.FPDFPage_CloseAnnot(annotationPtr);
        }
      });
    } finally {
      // 释放页面上下文引用
      pageCtx.release();
    }
  }

  /**
   * 重命名表单控件（Widget）的字段名称。
   *
   * 字段名称是 PDF 表单中用于标识和关联表单控件的关键属性。
   * 具有相同字段名称的多个控件会共享相同的值（如一组单选按钮）。
   *
   * @param doc - PDF 文档对象
   * @param page - 页面对象
   * @param annotation - 要重命名的表单控件注解
   * @param name - 新的字段名称
   * @returns 成功返回 true
   */
  renameWidgetField(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfWidgetAnnoObject,
    name: string,
  ): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'RenameWidgetField', doc, annotation, name);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RenameWidgetField`,
      'Begin',
      `${doc.id}-${annotation.id}`,
    );

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `RenameWidgetField`,
        'End',
        `${doc.id}-${annotation.id}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    try {
      return pageCtx.withFormHandle((formHandle) => {
        // 通过注解名称查找目标注解的 PDFium 指针
        const annotationPtr = this.getAnnotationByName(pageCtx.pagePtr, annotation.id);
        if (!annotationPtr) {
          return PdfTaskHelper.reject({
            code: PdfErrorCode.NotFound,
            message: 'annotation not found',
          });
        }

        try {
          // withWString 辅助方法：将 JS 字符串转为 UTF-16 写入 WASM 内存，
          // 执行回调后自动释放内存
          const ok = this.withWString(name, (namePtr) =>
            this.pdfiumModule.EPDFAnnot_SetFormFieldName(formHandle, annotationPtr, namePtr),
          );
          if (!ok) {
            return PdfTaskHelper.reject({
              code: PdfErrorCode.CantSetAnnotString,
              message: 'failed to rename widget field',
            });
          }

          this.logger.perf(
            LOG_SOURCE,
            LOG_CATEGORY,
            `RenameWidgetField`,
            'End',
            `${doc.id}-${annotation.id}`,
          );
          return PdfTaskHelper.resolve<boolean>(true);
        } finally {
          this.pdfiumModule.FPDFPage_CloseAnnot(annotationPtr);
        }
      });
    } finally {
      pageCtx.release();
    }
  }

  shareWidgetField(
    doc: PdfDocumentObject,
    sourcePage: PdfPageObject,
    sourceAnnotation: PdfWidgetAnnoObject,
    targetPage: PdfPageObject,
    targetAnnotation: PdfWidgetAnnoObject,
  ): PdfTask<boolean> {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'ShareWidgetField',
      doc,
      sourceAnnotation,
      targetAnnotation,
    );
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `ShareWidgetField`,
      'Begin',
      `${doc.id}-${sourceAnnotation.id}-${targetAnnotation.id}`,
    );

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `ShareWidgetField`,
        'End',
        `${doc.id}-${sourceAnnotation.id}-${targetAnnotation.id}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const sourcePageCtx = ctx.acquirePage(sourcePage.index);
    const targetPageCtx =
      targetPage.index === sourcePage.index ? sourcePageCtx : ctx.acquirePage(targetPage.index);

    try {
      return sourcePageCtx.withFormHandle((formHandle) => {
        let targetPageLoaded = false;
        if (targetPageCtx !== sourcePageCtx) {
          this.pdfiumModule.FORM_OnAfterLoadPage(targetPageCtx.pagePtr, formHandle);
          targetPageLoaded = true;
        }

        const sourceAnnotationPtr = this.getAnnotationByName(
          sourcePageCtx.pagePtr,
          sourceAnnotation.id,
        );
        const targetAnnotationPtr = this.getAnnotationByName(
          targetPageCtx.pagePtr,
          targetAnnotation.id,
        );
        if (!sourceAnnotationPtr || !targetAnnotationPtr) {
          if (sourceAnnotationPtr) {
            this.pdfiumModule.FPDFPage_CloseAnnot(sourceAnnotationPtr);
          }
          if (targetAnnotationPtr) {
            this.pdfiumModule.FPDFPage_CloseAnnot(targetAnnotationPtr);
          }
          if (targetPageLoaded) {
            this.pdfiumModule.FORM_OnBeforeClosePage(targetPageCtx.pagePtr, formHandle);
          }
          return PdfTaskHelper.reject({
            code: PdfErrorCode.NotFound,
            message: 'annotation not found',
          });
        }

        try {
          const ok = this.pdfiumModule.EPDFAnnot_ShareFormField(
            formHandle,
            sourceAnnotationPtr,
            targetAnnotationPtr,
          );
          if (!ok) {
            return PdfTaskHelper.reject({
              code: PdfErrorCode.Unknown,
              message: 'failed to share widget field',
            });
          }

          this.logger.perf(
            LOG_SOURCE,
            LOG_CATEGORY,
            `ShareWidgetField`,
            'End',
            `${doc.id}-${sourceAnnotation.id}-${targetAnnotation.id}`,
          );
          return PdfTaskHelper.resolve<boolean>(true);
        } finally {
          this.pdfiumModule.FPDFPage_CloseAnnot(sourceAnnotationPtr);
          this.pdfiumModule.FPDFPage_CloseAnnot(targetAnnotationPtr);
          if (targetPageLoaded) {
            this.pdfiumModule.FORM_OnBeforeClosePage(targetPageCtx.pagePtr, formHandle);
          }
        }
      });
    } finally {
      sourcePageCtx.release();
      if (targetPageCtx !== sourcePageCtx) {
        targetPageCtx.release();
      }
    }
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.flattenPage}
   *
   * @public
   */
  flattenPage(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfFlattenPageOptions,
  ): PdfTask<PdfPageFlattenResult> {
    const { flag = PdfPageFlattenFlag.Display } = options ?? {};
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'flattenPage', doc, page, flag);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `flattenPage`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `flattenPage`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    const result = this.pdfiumModule.FPDFPage_Flatten(pageCtx.pagePtr, flag);
    pageCtx.release();

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `flattenPage`, 'End', doc.id);

    return PdfTaskHelper.resolve(result);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.extractPages}
   *
   * @public
   */
  extractPages(doc: PdfDocumentObject, pageIndexes: number[]) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'extractPages', doc, pageIndexes);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ExtractPages`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ExtractPages`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const newDocPtr = this.pdfiumModule.FPDF_CreateNewDocument();
    if (!newDocPtr) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ExtractPages`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantCreateNewDoc,
        message: 'can not create new document',
      });
    }

    const pageIndexesPtr = this.memoryManager.malloc(pageIndexes.length * 4);
    for (let i = 0; i < pageIndexes.length; i++) {
      this.pdfiumModule.pdfium.setValue(pageIndexesPtr + i * 4, pageIndexes[i], 'i32');
    }

    if (
      !this.pdfiumModule.FPDF_ImportPagesByIndex(
        newDocPtr,
        ctx.docPtr,
        pageIndexesPtr,
        pageIndexes.length,
        0,
      )
    ) {
      this.pdfiumModule.FPDF_CloseDocument(newDocPtr);
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ExtractPages`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantImportPages,
        message: 'can not import pages to new document',
      });
    }

    const buffer = this.saveDocument(newDocPtr);

    this.pdfiumModule.FPDF_CloseDocument(newDocPtr);

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ExtractPages`, 'End', doc.id);
    return PdfTaskHelper.resolve(buffer);
  }

  /**
   * Import pages from a source document into a destination document.
   *
   * @param destDoc - destination document (must be open in cache)
   * @param srcDoc - source document (must be open in cache)
   * @param srcPageIndices - zero-based page indices in the source document
   * @param insertIndex - position to insert at in destination (defaults to end)
   * @returns task containing the newly added PdfPageObjects
   */
  public importPages(
    destDoc: PdfDocumentObject,
    srcDoc: PdfDocumentObject,
    srcPageIndices: number[],
    insertIndex?: number,
  ): PdfTask<PdfPageObject[]> {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'importPages',
      destDoc.id,
      srcDoc.id,
      srcPageIndices,
    );
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'ImportPages', 'Begin', destDoc.id);

    const destCtx = this.cache.getContext(destDoc.id);
    if (!destCtx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'ImportPages', 'End', destDoc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'destination document is not open',
      });
    }

    const srcCtx = this.cache.getContext(srcDoc.id);
    if (!srcCtx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'ImportPages', 'End', destDoc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'source document is not open',
      });
    }

    const destInsertIndex = insertIndex ?? this.pdfiumModule.FPDF_GetPageCount(destCtx.docPtr);

    const indicesPtr = this.memoryManager.malloc(srcPageIndices.length * 4);
    for (let i = 0; i < srcPageIndices.length; i++) {
      this.pdfiumModule.pdfium.setValue(indicesPtr + i * 4, srcPageIndices[i], 'i32');
    }

    if (
      !this.pdfiumModule.FPDF_ImportPagesByIndex(
        destCtx.docPtr,
        srcCtx.docPtr,
        indicesPtr,
        srcPageIndices.length,
        destInsertIndex,
      )
    ) {
      this.memoryManager.free(indicesPtr);
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'ImportPages', 'End', destDoc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantImportPages,
        message: 'can not import pages into destination document',
      });
    }

    this.memoryManager.free(indicesPtr);

    const newPages: PdfPageObject[] = [];
    const sizePtr = this.memoryManager.malloc(8);
    const normalizeRotation = destCtx.normalizeRotation;

    for (let i = 0; i < srcPageIndices.length; i++) {
      const newPageIndex = destInsertIndex + i;
      const result = normalizeRotation
        ? this.pdfiumModule.EPDF_GetPageSizeByIndexNormalized(destCtx.docPtr, newPageIndex, sizePtr)
        : this.pdfiumModule.FPDF_GetPageSizeByIndexF(destCtx.docPtr, newPageIndex, sizePtr);

      if (!result) {
        this.memoryManager.free(sizePtr);
        this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'ImportPages', 'End', destDoc.id);
        return PdfTaskHelper.reject({
          code: PdfErrorCode.Unknown,
          message: `failed to read metadata for imported page ${newPageIndex}`,
        });
      }

      const rotation = this.pdfiumModule.EPDF_GetPageRotationByIndex(
        destCtx.docPtr,
        newPageIndex,
      ) as Rotation;
      const objectNumber = this.pdfiumModule.EPDFDoc_GetPageObjectNumberByIndex(
        destCtx.docPtr,
        newPageIndex,
      );

      newPages.push({
        index: newPageIndex,
        size: {
          width: this.pdfiumModule.pdfium.getValue(sizePtr, 'float'),
          height: this.pdfiumModule.pdfium.getValue(sizePtr + 4, 'float'),
        },
        rotation,
        objectNumber,
      });
    }

    this.memoryManager.free(sizePtr);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'ImportPages', 'End', destDoc.id);
    return PdfTaskHelper.resolve(newPages);
  }

  public deletePage(doc: PdfDocumentObject, pageIndex: number): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'deletePage', doc.id, pageIndex);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'DeletePage', 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'DeletePage', 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document is not open',
      });
    }

    const pageCount = this.pdfiumModule.FPDF_GetPageCount(ctx.docPtr);
    if (pageIndex < 0 || pageIndex >= pageCount) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'DeletePage', 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantDeletePage,
        message: `page index ${pageIndex} out of range (0..${pageCount - 1})`,
      });
    }

    this.pdfiumModule.FPDFPage_Delete(ctx.docPtr, pageIndex);

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'DeletePage', 'End', doc.id);
    return PdfTaskHelper.resolve(true);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.extractText}
   *
   * @public
   */
  extractText(doc: PdfDocumentObject, pageIndexes: number[]) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'extractText', doc, pageIndexes);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ExtractText`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ExtractText`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const strings: string[] = [];
    for (let i = 0; i < pageIndexes.length; i++) {
      const pageCtx = ctx.acquirePage(pageIndexes[i]);
      const textPagePtr = this.pdfiumModule.FPDFText_LoadPage(pageCtx.pagePtr);
      const charCount = this.pdfiumModule.FPDFText_CountChars(textPagePtr);
      const bufferPtr = this.memoryManager.malloc((charCount + 1) * 2);
      this.pdfiumModule.FPDFText_GetText(textPagePtr, 0, charCount, bufferPtr);
      const text = this.pdfiumModule.pdfium.UTF16ToString(bufferPtr);
      this.memoryManager.free(bufferPtr);
      strings.push(text);
      this.pdfiumModule.FPDFText_ClosePage(textPagePtr);
      pageCtx.release();
    }

    const text = strings.join('\n\n');
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `ExtractText`, 'End', doc.id);
    return PdfTaskHelper.resolve(text);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.getTextSlices}
   *
   * @public
   */
  getTextSlices(doc: PdfDocumentObject, slices: PageTextSlice[]): PdfTask<string[]> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getTextSlices', doc, slices);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'GetTextSlices', 'Begin', doc.id);

    /* ⚠︎ 1 — trivial case */
    if (slices.length === 0) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'GetTextSlices', 'End', doc.id);
      return PdfTaskHelper.resolve<string[]>([]);
    }

    /* ⚠︎ 2 — document must be open */
    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'GetTextSlices', 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    try {
      /* keep caller order */
      const out = new Array<string>(slices.length);

      /* group → open each page once */
      const byPage = new Map<number, { slice: PageTextSlice; pos: number }[]>();
      slices.forEach((s, i) => {
        (byPage.get(s.pageIndex) ?? byPage.set(s.pageIndex, []).get(s.pageIndex))!.push({
          slice: s,
          pos: i,
        });
      });

      for (const [pageIdx, list] of byPage) {
        const pageCtx = ctx.acquirePage(pageIdx);
        const textPagePtr = pageCtx.getTextPage();

        for (const { slice, pos } of list) {
          const bufPtr = this.memoryManager.malloc(2 * (slice.charCount + 1)); // UTF-16 + NIL
          this.pdfiumModule.FPDFText_GetText(textPagePtr, slice.charIndex, slice.charCount, bufPtr);
          out[pos] = stripPdfUnwantedMarkers(this.pdfiumModule.pdfium.UTF16ToString(bufPtr));
          this.memoryManager.free(bufPtr);
        }
        pageCtx.release();
      }

      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'GetTextSlices', 'End', doc.id);
      return PdfTaskHelper.resolve(out);
    } catch (e) {
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, 'getTextSlices error', e);
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'GetTextSlices', 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: String(e),
      });
    }
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.merge}
   *
   * @public
   */
  merge(files: PdfFile[]) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'merge', files);
    const fileIds = files.map((file) => file.id).join('.');
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `Merge`, 'Begin', fileIds);

    const newDocPtr = this.pdfiumModule.FPDF_CreateNewDocument();
    if (!newDocPtr) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `Merge`, 'End', fileIds);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantCreateNewDoc,
        message: 'can not create new document',
      });
    }

    const ptrs: { docPtr: number; filePtr: WasmPointer }[] = [];
    for (const file of files.reverse()) {
      const array = new Uint8Array(file.content);
      const length = array.length;
      const filePtr = this.memoryManager.malloc(length);
      this.pdfiumModule.pdfium.HEAPU8.set(array, filePtr);

      const docPtr = this.pdfiumModule.FPDF_LoadMemDocument(filePtr, length, '');
      if (!docPtr) {
        const lastError = this.pdfiumModule.FPDF_GetLastError();
        this.logger.error(
          LOG_SOURCE,
          LOG_CATEGORY,
          `FPDF_LoadMemDocument failed with ${lastError}`,
        );
        this.memoryManager.free(filePtr);

        for (const ptr of ptrs) {
          this.pdfiumModule.FPDF_CloseDocument(ptr.docPtr);
          this.memoryManager.free(ptr.filePtr);
        }

        this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `Merge`, 'End', fileIds);
        return PdfTaskHelper.reject<PdfFile>({
          code: lastError,
          message: `FPDF_LoadMemDocument failed`,
        });
      }
      ptrs.push({ filePtr, docPtr });

      if (!this.pdfiumModule.FPDF_ImportPages(newDocPtr, docPtr, '', 0)) {
        this.pdfiumModule.FPDF_CloseDocument(newDocPtr);

        for (const ptr of ptrs) {
          this.pdfiumModule.FPDF_CloseDocument(ptr.docPtr);
          this.memoryManager.free(ptr.filePtr);
        }

        this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `Merge`, 'End', fileIds);
        return PdfTaskHelper.reject({
          code: PdfErrorCode.CantImportPages,
          message: 'can not import pages to new document',
        });
      }
    }
    const buffer = this.saveDocument(newDocPtr);

    this.pdfiumModule.FPDF_CloseDocument(newDocPtr);

    for (const ptr of ptrs) {
      this.pdfiumModule.FPDF_CloseDocument(ptr.docPtr);
      this.memoryManager.free(ptr.filePtr);
    }

    const file: PdfFile = {
      id: `${Math.random()}`,
      content: buffer,
    };
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `Merge`, 'End', fileIds);
    return PdfTaskHelper.resolve(file);
  }

  /**
   * Merges specific pages from multiple PDF documents in a custom order
   *
   * @param mergeConfigs Array of configurations specifying which pages to merge from which documents
   * @returns A PdfTask that resolves with the merged PDF file
   * @public
   */
  mergePages(mergeConfigs: Array<{ docId: string; pageIndices: number[] }>) {
    const configIds = mergeConfigs
      .map((config) => `${config.docId}:${config.pageIndices.join(',')}`)
      .join('|');
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'mergePages', mergeConfigs);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `MergePages`, 'Begin', configIds);

    // Create a new document to import pages into
    const newDocPtr = this.pdfiumModule.FPDF_CreateNewDocument();
    if (!newDocPtr) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `MergePages`, 'End', configIds);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantCreateNewDoc,
        message: 'Cannot create new document',
      });
    }

    try {
      // Process each merge configuration in reverse order (since we're inserting at position 0)
      // This ensures the final document has pages in the order specified by the user
      for (const config of [...mergeConfigs].reverse()) {
        // Check if the document is open
        const ctx = this.cache.getContext(config.docId);

        if (!ctx) {
          this.logger.warn(
            LOG_SOURCE,
            LOG_CATEGORY,
            `Document ${config.docId} is not open, skipping`,
          );
          continue;
        }

        // Get the page count for this document
        const pageCount = this.pdfiumModule.FPDF_GetPageCount(ctx.docPtr);

        // Filter out invalid page indices
        const validPageIndices = config.pageIndices.filter(
          (index) => index >= 0 && index < pageCount,
        );

        if (validPageIndices.length === 0) {
          continue; // No valid pages to import
        }

        // Convert 0-based indices to 1-based for PDFium and join with commas
        const pageString = validPageIndices.map((index) => index + 1).join(',');

        try {
          // Import all specified pages at once from this document
          if (
            !this.pdfiumModule.FPDF_ImportPages(
              newDocPtr,
              ctx.docPtr,
              pageString,
              0, // Insert at the beginning
            )
          ) {
            throw new Error(`Failed to import pages ${pageString} from document ${config.docId}`);
          }
        } finally {
        }
      }

      // Save the new document to buffer
      const buffer = this.saveDocument(newDocPtr);

      const file: PdfFile = {
        id: `${Math.random()}`,
        content: buffer,
      };

      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `MergePages`, 'End', configIds);
      return PdfTaskHelper.resolve(file);
    } catch (error) {
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, 'mergePages failed', error);
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `MergePages`, 'End', configIds);

      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantImportPages,
        message: error instanceof Error ? error.message : 'Failed to merge pages',
      });
    } finally {
      // 清理资源 the new document
      if (newDocPtr) {
        this.pdfiumModule.FPDF_CloseDocument(newDocPtr);
      }
    }
  }

  /**
   * Sets AES-256 encryption on a document.
   * Must be called before saveAsCopy() for encryption to take effect.
   *
   * @param doc - Document to encrypt
   * @param userPassword - Password to open document (empty = no open password)
   * @param ownerPassword - Password to change permissions (required)
   * @param allowedFlags - OR'd PdfPermissionFlag values indicating allowed actions
   * @returns true on success, false if already encrypted or invalid params
   *
   * @public
   */
  setDocumentEncryption(
    doc: PdfDocumentObject,
    userPassword: string,
    ownerPassword: string,
    allowedFlags: number,
  ): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'setDocumentEncryption', doc, allowedFlags);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const result = this.pdfiumModule.EPDF_SetEncryption(
      ctx.docPtr,
      userPassword,
      ownerPassword,
      allowedFlags,
    );

    return PdfTaskHelper.resolve(result);
  }

  /**
   * Marks document for encryption removal on save.
   * When saveAsCopy is called, the document will be saved without encryption.
   *
   * @param doc - Document to remove encryption from
   * @returns true on success
   *
   * @public
   */
  removeEncryption(doc: PdfDocumentObject): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'removeEncryption', doc);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const result = this.pdfiumModule.EPDF_RemoveEncryption(ctx.docPtr);

    return PdfTaskHelper.resolve(result);
  }

  /**
   * Attempts to unlock owner permissions for an already-opened encrypted document.
   *
   * @param doc - Document to unlock
   * @param ownerPassword - The owner password
   * @returns true on success, false on failure
   *
   * @public
   */
  unlockOwnerPermissions(doc: PdfDocumentObject, ownerPassword: string): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'unlockOwnerPermissions', doc.id);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const success = this.pdfiumModule.EPDF_UnlockOwnerPermissions(ctx.docPtr, ownerPassword);

    return PdfTaskHelper.resolve(success);
  }

  /**
   * Check if a document is encrypted.
   *
   * @param doc - Document to check
   * @returns true if the document is encrypted
   *
   * @public
   */
  isEncrypted(doc: PdfDocumentObject): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'isEncrypted', doc.id);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const result = this.pdfiumModule.EPDF_IsEncrypted(ctx.docPtr);
    return PdfTaskHelper.resolve(result);
  }

  /**
   * Check if owner permissions are currently unlocked.
   *
   * @param doc - Document to check
   * @returns true if owner permissions are unlocked
   *
   * @public
   */
  isOwnerUnlocked(doc: PdfDocumentObject): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'isOwnerUnlocked', doc.id);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const result = this.pdfiumModule.EPDF_IsOwnerUnlocked(ctx.docPtr);
    return PdfTaskHelper.resolve(result);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.saveAsCopy}
   *
   * @public
   */
  saveAsCopy(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'saveAsCopy', doc);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `SaveAsCopy`, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `SaveAsCopy`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const buffer = this.saveDocument(ctx.docPtr);

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `SaveAsCopy`, 'End', doc.id);
    return PdfTaskHelper.resolve(buffer);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.closeDocument}
   *
   * @public
   */
  closeDocument(doc: PdfDocumentObject) {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'closeDocument', doc);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `CloseDocument`, 'Begin', doc.id);

    this.cache.closeDocument(doc.id);

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `CloseDocument`, 'End', doc.id);
    return PdfTaskHelper.resolve(true);
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.closeAllDocuments}
   *
   * @public
   */
  closeAllDocuments() {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'closeAllDocuments');
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `CloseAllDocuments`, 'Begin');
    this.cache.closeAllDocuments();
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `CloseAllDocuments`, 'End');
    return PdfTaskHelper.resolve(true);
  }

  /**
   * Add text content to annotation
   * @param page - page info
   * @param pagePtr - pointer to page object
   * @param annotationPtr - pointer to text annotation
   * @param annotation - text annotation
   * @returns whether text content is added to annotation
   *
   * @private
   */
  private addTextContent(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfTextAnnoObject,
  ) {
    // Type-specific properties
    if (
      !this.setAnnotationName(
        annotationPtr,
        annotation.name ?? annotation.icon ?? PdfAnnotationName.Comment,
      )
    ) {
      return false;
    }
    if (annotation.state && !this.setAnnotString(annotationPtr, 'State', annotation.state)) {
      return false;
    }
    if (
      annotation.stateModel &&
      !this.setAnnotString(annotationPtr, 'StateModel', annotation.stateModel)
    ) {
      return false;
    }

    if (!this.setAnnotationOpacity(annotationPtr, annotation.opacity ?? 1)) {
      return false;
    }
    // Prefer strokeColor, fall back to deprecated color
    const strokeColor = annotation.strokeColor ?? annotation.color ?? '#FFFF00';
    if (!this.setAnnotationColor(annotationPtr, strokeColor, PdfAnnotationColorType.Color)) {
      return false;
    }

    // Text annotations have default flags if not specified
    if (!annotation.flags) {
      if (!this.setAnnotationFlags(annotationPtr, ['print', 'noZoom', 'noRotate'])) {
        return false;
      }
    }

    // Apply base annotation properties (author, contents, dates, flags, custom, IRT, RT)
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  /**
   * Add caret content to annotation
   * @param doc - document object
   * @param page - page info
   * @param pagePtr - pointer to page object
   * @param annotationPtr - pointer to caret annotation
   * @param annotation - caret annotation
   * @returns whether caret content is added to annotation
   *
   * @private
   */
  private addCaretContent(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfCaretAnnoObject,
  ) {
    if (annotation.strokeColor) {
      this.setAnnotationColor(annotationPtr, annotation.strokeColor, PdfAnnotationColorType.Color);
    }
    if (annotation.opacity !== undefined) {
      this.setAnnotationOpacity(annotationPtr, annotation.opacity);
    }
    if (annotation.intent) {
      this.setAnnotIntent(annotationPtr, annotation.intent);
    }
    this.setRectangleDifferences(annotationPtr, annotation.rectangleDifferences);
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  /**
   * Add free text content to annotation
   * @param page - page info
   * @param pagePtr - pointer to page object
   * @param annotationPtr - pointer to free text annotation
   * @param annotation - free text annotation
   * @returns whether free text content is added to annotation
   *
   * @private
   */
  private addFreeTextContent(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfFreeTextAnnoObject,
  ) {
    // Type-specific properties
    if (
      !this.setBorderStyle(
        annotationPtr,
        PdfAnnotationBorderStyle.SOLID,
        annotation.strokeWidth ?? 0,
      )
    ) {
      return false;
    }
    if (!this.setAnnotationOpacity(annotationPtr, annotation.opacity ?? 1)) {
      return false;
    }
    if (!this.setAnnotationTextAlignment(annotationPtr, annotation.textAlign)) {
      return false;
    }
    if (!this.setAnnotationVerticalAlignment(annotationPtr, annotation.verticalAlign)) {
      return false;
    }
    const daColor = annotation.strokeColor ?? annotation.fontColor;
    if (
      !this.setAnnotationDefaultAppearance(
        annotationPtr,
        annotation.fontFamily === PdfStandardFont.Unknown
          ? PdfStandardFont.Helvetica
          : annotation.fontFamily,
        annotation.fontSize,
        daColor,
      )
    ) {
      return false;
    }
    if (annotation.strokeColor && annotation.strokeColor !== annotation.fontColor) {
      this.setAnnotationColor(
        annotationPtr,
        annotation.fontColor,
        PdfAnnotationColorType.TextColor,
      );
    }
    if (annotation.intent && !this.setAnnotIntent(annotationPtr, annotation.intent)) {
      return false;
    }
    if (
      annotation.calloutLine &&
      annotation.calloutLine.length >= 2 &&
      !this.setCalloutLine(doc, page, annotationPtr, annotation.calloutLine)
    ) {
      return false;
    }
    if (
      annotation.lineEnding !== undefined &&
      !this.setLineEndings(annotationPtr, PdfAnnotationLineEnding.None, annotation.lineEnding)
    ) {
      return false;
    }
    // Prefer color, fall back to deprecated backgroundColor
    const bgColor = annotation.color ?? annotation.backgroundColor;
    if (!bgColor || bgColor === 'transparent') {
      if (!this.pdfiumModule.EPDFAnnot_ClearColor(annotationPtr, PdfAnnotationColorType.Color)) {
        return false;
      }
    } else if (
      !this.setAnnotationColor(annotationPtr, bgColor ?? '#FFFFFF', PdfAnnotationColorType.Color)
    ) {
      return false;
    }

    this.setRectangleDifferences(annotationPtr, annotation.rectangleDifferences);

    // Apply base annotation properties (author, contents, dates, flags, custom, IRT, RT)
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  private addTextFieldContent(
    formHandle: number,
    annotationPtr: number,
    annotation: PdfWidgetAnnoObject,
  ): boolean {
    // 1. DA (font, size, color)
    if (
      !this.setAnnotationDefaultAppearance(
        annotationPtr,
        annotation.fontFamily,
        annotation.fontSize,
        annotation.fontColor,
      )
    ) {
      return false;
    }

    // 2. BS (border style / width)
    if (
      !this.setBorderStyle(
        annotationPtr,
        PdfAnnotationBorderStyle.SOLID,
        annotation.strokeWidth ?? 1,
      )
    ) {
      return false;
    }

    // 3. MK colors (border / background)
    if (annotation.strokeColor && annotation.strokeColor !== 'transparent') {
      this.setMKColor(annotationPtr, 0, annotation.strokeColor); // EPDF_MK_COLOR_BC
    } else {
      this.clearMKColor(annotationPtr, 0);
    }
    if (annotation.color && annotation.color !== 'transparent') {
      this.setMKColor(annotationPtr, 1, annotation.color); // EPDF_MK_COLOR_BG
    } else {
      this.clearMKColor(annotationPtr, 1);
    }

    // 4. Form field flags
    const userFlags = annotation.field.flag ?? PDF_FORM_FIELD_FLAG.NONE;
    this.pdfiumModule.FPDFAnnot_SetFormFieldFlags(formHandle, annotationPtr, userFlags);

    // 5. Field name
    if (annotation.field.name) {
      this.withWString(annotation.field.name, (namePtr) =>
        this.pdfiumModule.EPDFAnnot_SetFormFieldName(formHandle, annotationPtr, namePtr),
      );
    }

    // 6. Field value
    this.withWString(annotation.field.value ?? '', (valuePtr) =>
      this.pdfiumModule.EPDFAnnot_SetFormFieldValue(formHandle, annotationPtr, valuePtr),
    );

    // 7. MaxLen
    const textField = annotation.field as PdfTextWidgetAnnoField;
    if (textField.maxLen != null && textField.maxLen > 0) {
      this.pdfiumModule.EPDFAnnot_SetNumberValue(annotationPtr, 'MaxLen', textField.maxLen);
    }

    return true;
  }

  private addToggleFieldContent(
    formHandle: number,
    annotationPtr: number,
    annotation: PdfWidgetAnnoObject,
  ): boolean {
    // 1. BS (border style / width)
    if (
      !this.setBorderStyle(
        annotationPtr,
        PdfAnnotationBorderStyle.SOLID,
        annotation.strokeWidth ?? 1,
      )
    ) {
      return false;
    }

    // 2. MK colors (border / background)
    if (annotation.strokeColor && annotation.strokeColor !== 'transparent') {
      this.setMKColor(annotationPtr, 0, annotation.strokeColor);
    } else {
      this.clearMKColor(annotationPtr, 0);
    }
    if (annotation.color && annotation.color !== 'transparent') {
      this.setMKColor(annotationPtr, 1, annotation.color);
    } else {
      this.clearMKColor(annotationPtr, 1);
    }

    // 3. Form field flags (preserve structural bits for radio buttons)
    let finalFlags = annotation.field.flag ?? PDF_FORM_FIELD_FLAG.NONE;
    if (annotation.field.type === PDF_FORM_FIELD_TYPE.RADIOBUTTON) {
      finalFlags |= PDF_FORM_FIELD_FLAG.BUTTON_RADIO | PDF_FORM_FIELD_FLAG.BUTTON_NOTOGGLETOOFF;
    }
    this.pdfiumModule.FPDFAnnot_SetFormFieldFlags(formHandle, annotationPtr, finalFlags);

    // 4. Field name
    if (annotation.field.name) {
      this.withWString(annotation.field.name, (namePtr) =>
        this.pdfiumModule.EPDFAnnot_SetFormFieldName(formHandle, annotationPtr, namePtr),
      );
    }

    return true;
  }

  private addChoiceFieldContent(
    formHandle: number,
    annotationPtr: number,
    annotation: PdfWidgetAnnoObject,
  ): boolean {
    // 1. DA (font, size, color)
    if (
      !this.setAnnotationDefaultAppearance(
        annotationPtr,
        annotation.fontFamily,
        annotation.fontSize,
        annotation.fontColor,
      )
    ) {
      return false;
    }

    // 2. BS (border style / width)
    if (
      !this.setBorderStyle(
        annotationPtr,
        PdfAnnotationBorderStyle.SOLID,
        annotation.strokeWidth ?? 1,
      )
    ) {
      return false;
    }

    // 3. MK colors (border / background)
    if (annotation.strokeColor && annotation.strokeColor !== 'transparent') {
      this.setMKColor(annotationPtr, 0, annotation.strokeColor);
    } else {
      this.clearMKColor(annotationPtr, 0);
    }
    if (annotation.color && annotation.color !== 'transparent') {
      this.setMKColor(annotationPtr, 1, annotation.color);
    } else {
      this.clearMKColor(annotationPtr, 1);
    }

    // 4. Form field flags -- preserve the base type bit for combobox (bit 17 = Combo)
    let choiceFlags = annotation.field.flag ?? PDF_FORM_FIELD_FLAG.NONE;
    if (annotation.field.type === PDF_FORM_FIELD_TYPE.COMBOBOX) {
      choiceFlags |= 1 << 17;
    }
    this.pdfiumModule.FPDFAnnot_SetFormFieldFlags(formHandle, annotationPtr, choiceFlags);

    // 5. Field name
    if (annotation.field.name) {
      this.withWString(annotation.field.name, (namePtr) =>
        this.pdfiumModule.EPDFAnnot_SetFormFieldName(formHandle, annotationPtr, namePtr),
      );
    }

    // 6. Options (/Opt array)
    const field = annotation.field as { options?: { label: string; isSelected: boolean }[] };
    const options = field.options ?? [];
    if (options.length > 0) {
      const ptrSize = 4;
      const arrayPtr = this.memoryManager.malloc(options.length * ptrSize);
      const labelPtrs: number[] = [];
      try {
        for (let i = 0; i < options.length; i++) {
          const label = options[i].label;
          const byteLen = (label.length + 1) * 2;
          const labelPtr = this.memoryManager.malloc(byteLen);
          this.pdfiumModule.pdfium.stringToUTF16(label, labelPtr, byteLen);
          labelPtrs.push(labelPtr);
          this.pdfiumModule.pdfium.setValue(arrayPtr + i * ptrSize, labelPtr, '*');
        }
        this.pdfiumModule.EPDFAnnot_SetFormFieldOptions(
          formHandle,
          annotationPtr,
          arrayPtr,
          options.length,
        );
      } finally {
        for (const ptr of labelPtrs) {
          this.memoryManager.free(WasmPointer(ptr));
        }
        this.memoryManager.free(arrayPtr);
      }
    }

    // 7. Field value (/V) — set to the first selected option or empty
    const selectedOption = options.find((opt) => opt.isSelected);
    const value = selectedOption?.label ?? annotation.field.value ?? '';
    this.withWString(value, (valuePtr) =>
      this.pdfiumModule.EPDFAnnot_SetFormFieldValue(formHandle, annotationPtr, valuePtr),
    );

    return true;
  }

  private setMKColor(annotationPtr: number, mkType: number, webColor: string): boolean {
    const { red, green, blue } = webColorToPdfColor(webColor);
    return this.pdfiumModule.EPDFAnnot_SetMKColor(
      annotationPtr,
      mkType,
      red & 0xff,
      green & 0xff,
      blue & 0xff,
    );
  }

  private clearMKColor(annotationPtr: number, mkType: number): boolean {
    return this.pdfiumModule.EPDFAnnot_ClearMKColor(annotationPtr, mkType);
  }

  private getMKColor(annotationPtr: number, mkType: number): string | undefined {
    const rPtr = this.memoryManager.malloc(4);
    const gPtr = this.memoryManager.malloc(4);
    const bPtr = this.memoryManager.malloc(4);
    try {
      const ok = this.pdfiumModule.EPDFAnnot_GetMKColor(annotationPtr, mkType, rPtr, gPtr, bPtr);
      if (!ok) return undefined;
      const r = this.pdfiumModule.pdfium.getValue(rPtr, 'i32') & 0xff;
      const g = this.pdfiumModule.pdfium.getValue(gPtr, 'i32') & 0xff;
      const b = this.pdfiumModule.pdfium.getValue(bPtr, 'i32') & 0xff;
      return pdfColorToWebColor({ red: r, green: g, blue: b });
    } finally {
      this.memoryManager.free(bPtr);
      this.memoryManager.free(gPtr);
      this.memoryManager.free(rPtr);
    }
  }

  /**
   * 设置注解的矩形区域
   *
   * 将设备坐标系中的矩形转换为页面坐标系后写入 PDF 注解字典。
   * 转换时将矩形的四个角点分别映射到页面坐标，然后计算 AABB（轴对齐包围盒）。
   * @param page - page info that the annotation is belonged to
   * @param pagePtr - pointer of page object
   * @param annotationPtr - pointer to annotation object
   * @param inkList - ink lists that added to the annotation
   * @returns whether the ink lists is setted
   *
   * @private
   */
  private addInkStroke(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfInkAnnoObject,
  ) {
    // Type-specific properties
    if (
      !this.setBorderStyle(annotationPtr, PdfAnnotationBorderStyle.SOLID, annotation.strokeWidth)
    ) {
      return false;
    }
    if (!this.setInkList(doc, page, annotationPtr, annotation.inkList)) {
      return false;
    }
    if (!this.setAnnotationOpacity(annotationPtr, annotation.opacity ?? 1)) {
      return false;
    }
    // Prefer strokeColor, fall back to deprecated color
    const strokeColor = annotation.strokeColor ?? annotation.color ?? '#FFFF00';
    if (!this.setAnnotationColor(annotationPtr, strokeColor, PdfAnnotationColorType.Color)) {
      return false;
    }

    // Apply base annotation properties (author, contents, dates, flags, custom, IRT, RT)
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  /**
   * Add line content to annotation
   * @param page - page info
   * @param pagePtr - pointer to page object
   * @param annotationPtr - pointer to line annotation
   * @param annotation - line annotation
   * @returns whether line content is added to annotation
   *
   * @private
   */
  private addLineContent(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfLineAnnoObject,
  ) {
    // Type-specific properties
    if (
      !this.setLinePoints(
        doc,
        page,
        annotationPtr,
        annotation.linePoints.start,
        annotation.linePoints.end,
      )
    ) {
      return false;
    }
    if (
      !this.setLineEndings(
        annotationPtr,
        annotation.lineEndings?.start ?? PdfAnnotationLineEnding.None,
        annotation.lineEndings?.end ?? PdfAnnotationLineEnding.None,
      )
    ) {
      return false;
    }
    if (!this.setBorderStyle(annotationPtr, annotation.strokeStyle, annotation.strokeWidth)) {
      return false;
    }
    if (!this.setBorderDashPattern(annotationPtr, annotation.strokeDashArray ?? [])) {
      return false;
    }
    if (annotation.intent && !this.setAnnotIntent(annotationPtr, annotation.intent)) {
      return false;
    }
    if (!annotation.color || annotation.color === 'transparent') {
      if (
        !this.pdfiumModule.EPDFAnnot_ClearColor(annotationPtr, PdfAnnotationColorType.InteriorColor)
      ) {
        return false;
      }
    } else if (
      !this.setAnnotationColor(
        annotationPtr,
        annotation.color ?? '#FFFF00',
        PdfAnnotationColorType.InteriorColor,
      )
    ) {
      return false;
    }
    if (!this.setAnnotationOpacity(annotationPtr, annotation.opacity ?? 1)) {
      return false;
    }
    if (
      !this.setAnnotationColor(
        annotationPtr,
        annotation.strokeColor ?? '#FFFF00',
        PdfAnnotationColorType.Color,
      )
    ) {
      return false;
    }

    // Apply base annotation properties (author, contents, dates, flags, custom, IRT, RT)
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  /**
   * Add polygon or polyline content to annotation
   * @param page - page info
   * @param pagePtr - pointer to page object
   * @param annotationPtr - pointer to polygon or polyline annotation
   * @param annotation - polygon or polyline annotation
   * @returns whether polygon or polyline content is added to annotation
   *
   * @private
   */
  private addPolyContent(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfPolygonAnnoObject | PdfPolylineAnnoObject,
  ) {
    // Type-specific properties
    if (
      annotation.type === PdfAnnotationSubtype.POLYLINE &&
      !this.setLineEndings(
        annotationPtr,
        annotation.lineEndings?.start ?? PdfAnnotationLineEnding.None,
        annotation.lineEndings?.end ?? PdfAnnotationLineEnding.None,
      )
    ) {
      return false;
    }
    if (!this.setPdfAnnoVertices(doc, page, annotationPtr, annotation.vertices)) {
      return false;
    }
    if (!this.setBorderStyle(annotationPtr, annotation.strokeStyle, annotation.strokeWidth)) {
      return false;
    }
    if (!this.setBorderDashPattern(annotationPtr, annotation.strokeDashArray ?? [])) {
      return false;
    }
    if (annotation.intent && !this.setAnnotIntent(annotationPtr, annotation.intent)) {
      return false;
    }
    if (!annotation.color || annotation.color === 'transparent') {
      if (
        !this.pdfiumModule.EPDFAnnot_ClearColor(annotationPtr, PdfAnnotationColorType.InteriorColor)
      ) {
        return false;
      }
    } else if (
      !this.setAnnotationColor(
        annotationPtr,
        annotation.color ?? '#FFFF00',
        PdfAnnotationColorType.InteriorColor,
      )
    ) {
      return false;
    }
    if (!this.setAnnotationOpacity(annotationPtr, annotation.opacity ?? 1)) {
      return false;
    }
    if (
      !this.setAnnotationColor(
        annotationPtr,
        annotation.strokeColor ?? '#FFFF00',
        PdfAnnotationColorType.Color,
      )
    ) {
      return false;
    }

    if (annotation.type === PdfAnnotationSubtype.POLYGON) {
      const poly = annotation as PdfPolygonAnnoObject;
      this.setRectangleDifferences(annotationPtr, poly.rectangleDifferences);
      this.setBorderEffect(annotationPtr, poly.cloudyBorderIntensity);
    }

    // Apply base annotation properties (author, contents, dates, flags, custom, IRT, RT)
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  /**
   * Add link content (action or destination) to a link annotation
   * @param docPtr - pointer to pdf document
   * @param pagePtr - pointer to the page
   * @param annotationPtr - pointer to pdf annotation
   * @param annotation - the link annotation object
   * @returns true if successful
   *
   * @private
   */
  private addLinkContent(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    docPtr: number,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfLinkAnnoObject,
  ): boolean {
    // Type-specific properties
    // Border style and width (default: underline with width 2)
    const style = annotation.strokeStyle ?? PdfAnnotationBorderStyle.UNDERLINE;
    const width = annotation.strokeWidth ?? 2;
    if (!this.setBorderStyle(annotationPtr, style, width)) {
      return false;
    }
    if (
      annotation.strokeDashArray &&
      !this.setBorderDashPattern(annotationPtr, annotation.strokeDashArray)
    ) {
      return false;
    }

    // Stroke color
    if (annotation.strokeColor) {
      if (
        !this.setAnnotationColor(
          annotationPtr,
          annotation.strokeColor,
          PdfAnnotationColorType.Color,
        )
      ) {
        return false;
      }
    }

    // Target (action or destination)
    if (annotation.target) {
      if (!this.applyLinkTarget(docPtr, annotationPtr, annotation.target)) {
        return false;
      }
    }

    // Apply base annotation properties (author, contents, dates, flags, custom, IRT, RT)
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  /**
   * Add shape content to annotation
   * @param page - page info
   * @param pagePtr - pointer to page object
   * @param annotationPtr - pointer to shape annotation
   * @param annotation - shape annotation
   * @returns whether shape content is added to annotation
   *
   * @private
   */
  addShapeContent(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfCircleAnnoObject | PdfSquareAnnoObject,
  ) {
    // Type-specific properties
    if (!this.setBorderStyle(annotationPtr, annotation.strokeStyle, annotation.strokeWidth)) {
      return false;
    }
    if (!this.setBorderDashPattern(annotationPtr, annotation.strokeDashArray ?? [])) {
      return false;
    }
    if (!annotation.color || annotation.color === 'transparent') {
      if (
        !this.pdfiumModule.EPDFAnnot_ClearColor(annotationPtr, PdfAnnotationColorType.InteriorColor)
      ) {
        return false;
      }
    } else if (
      !this.setAnnotationColor(
        annotationPtr,
        annotation.color ?? '#FFFF00',
        PdfAnnotationColorType.InteriorColor,
      )
    ) {
      return false;
    }
    if (!this.setAnnotationOpacity(annotationPtr, annotation.opacity ?? 1)) {
      return false;
    }
    if (
      !this.setAnnotationColor(
        annotationPtr,
        annotation.strokeColor ?? '#FFFF00',
        PdfAnnotationColorType.Color,
      )
    ) {
      return false;
    }

    this.setRectangleDifferences(annotationPtr, annotation.rectangleDifferences);
    this.setBorderEffect(annotationPtr, annotation.cloudyBorderIntensity);

    // Apply base annotation properties (author, contents, dates, flags, custom, IRT, RT)
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  /**
   * Add highlight content to annotation
   * @param page - page info
   * @param annotationPtr - pointer to highlight annotation
   * @param annotation - highlight annotation
   * @returns whether highlight content is added to annotation
   *
   * @private
   */
  addTextMarkupContent(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation:
      | PdfHighlightAnnoObject
      | PdfUnderlineAnnoObject
      | PdfStrikeOutAnnoObject
      | PdfSquigglyAnnoObject,
  ) {
    // Type-specific properties
    if (!this.syncQuadPointsAnno(doc, page, annotationPtr, annotation.segmentRects)) {
      return false;
    }
    if (!this.setAnnotationOpacity(annotationPtr, annotation.opacity ?? 1)) {
      return false;
    }
    // Prefer strokeColor, fall back to deprecated color
    const strokeColor = annotation.strokeColor ?? annotation.color ?? '#FFFF00';
    if (!this.setAnnotationColor(annotationPtr, strokeColor, PdfAnnotationColorType.Color)) {
      return false;
    }

    // Apply base annotation properties (author, contents, dates, flags, custom, IRT, RT)
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  /**
   * Add content to redact annotation
   * @param page - page info
   * @param pagePtr - pointer to page object
   * @param annotationPtr - pointer to redact annotation
   * @param annotation - redact annotation
   * @returns whether redact content is added to annotation
   *
   * @private
   */
  private addRedactContent(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfRedactAnnoObject,
  ) {
    // Sync QuadPoints for redaction areas
    if (!this.syncQuadPointsAnno(doc, page, annotationPtr, annotation.segmentRects)) {
      return false;
    }

    // Set opacity
    if (!this.setAnnotationOpacity(annotationPtr, annotation.opacity ?? 1)) {
      return false;
    }

    // Set interior/preview color (IC)
    if (!annotation.color || annotation.color === 'transparent') {
      if (
        !this.pdfiumModule.EPDFAnnot_ClearColor(annotationPtr, PdfAnnotationColorType.InteriorColor)
      ) {
        return false;
      }
    } else if (
      !this.setAnnotationColor(
        annotationPtr,
        annotation.color,
        PdfAnnotationColorType.InteriorColor,
      )
    ) {
      return false;
    }

    // Set overlay color (OC) - the fill after redaction is applied
    if (!annotation.overlayColor || annotation.overlayColor === 'transparent') {
      if (
        !this.pdfiumModule.EPDFAnnot_ClearColor(annotationPtr, PdfAnnotationColorType.OverlayColor)
      ) {
        return false;
      }
    } else if (
      !this.setAnnotationColor(
        annotationPtr,
        annotation.overlayColor,
        PdfAnnotationColorType.OverlayColor,
      )
    ) {
      return false;
    }

    // Set stroke/border color (C)
    if (!annotation.strokeColor || annotation.strokeColor === 'transparent') {
      if (!this.pdfiumModule.EPDFAnnot_ClearColor(annotationPtr, PdfAnnotationColorType.Color)) {
        return false;
      }
    } else if (
      !this.setAnnotationColor(annotationPtr, annotation.strokeColor, PdfAnnotationColorType.Color)
    ) {
      return false;
    }

    // Set overlay text
    if (!this.setOverlayText(annotationPtr, annotation.overlayText)) {
      return false;
    }

    // Set overlay text repeat
    if (
      annotation.overlayTextRepeat !== undefined &&
      !this.setOverlayTextRepeat(annotationPtr, annotation.overlayTextRepeat)
    ) {
      return false;
    }

    // Set font properties via default appearance (DA) if provided
    if (annotation.fontFamily !== undefined || annotation.fontSize !== undefined) {
      const font =
        annotation.fontFamily == null || annotation.fontFamily === PdfStandardFont.Unknown
          ? PdfStandardFont.Helvetica
          : annotation.fontFamily;
      if (
        !this.setAnnotationDefaultAppearance(
          annotationPtr,
          font,
          annotation.fontSize ?? 12,
          annotation.fontColor ?? '#000000',
        )
      ) {
        return false;
      }
    }

    // Set text alignment
    if (
      annotation.textAlign !== undefined &&
      !this.setAnnotationTextAlignment(annotationPtr, annotation.textAlign)
    ) {
      return false;
    }

    // Apply base annotation properties (author, contents, dates, flags, custom, IRT, RT)
    return this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation);
  }

  /**
   * Add contents to stamp annotation
   * @param doc - pdf document object
   * @param docPtr - pointer to pdf document object
   * @param page - page info
   * @param pagePtr - pointer to page object
   * @param annotationPtr - pointer to stamp annotation
   * @param rect - rect of stamp annotation
   * @param contents - contents of stamp annotation
   * @returns whether contents is added to annotation
   *
   * @private
   */
  addStampContent(
    doc: PdfDocumentObject,
    docPtr: number,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfStampAnnoObject,
    context?: AnnotationCreateContext<PdfStampAnnoObject>,
  ) {
    const stampName = annotation.name ?? annotation.icon;
    if (stampName && !this.setAnnotationName(annotationPtr, stampName)) {
      return false;
    }

    if (context && 'data' in context && context.data) {
      const meta = getImageMetadata(context.data);
      if (!meta) return false;

      if (meta.mimeType === 'application/pdf') {
        if (!this.setAppearanceFromPdf(docPtr, annotationPtr, context.data)) {
          return false;
        }
      } else {
        for (let i = this.pdfiumModule.FPDFAnnot_GetObjectCount(annotationPtr) - 1; i >= 0; i--) {
          this.pdfiumModule.FPDFAnnot_RemoveObject(annotationPtr, i);
        }

        if (meta.mimeType === 'image/png') {
          if (
            !this.addPngImageObject(
              doc,
              docPtr,
              page,
              pagePtr,
              annotationPtr,
              annotation.rect,
              context.data,
            )
          ) {
            return false;
          }
        } else if (meta.mimeType === 'image/jpeg') {
          if (
            !this.addJpegImageObject(
              doc,
              docPtr,
              page,
              pagePtr,
              annotationPtr,
              annotation.rect,
              context.data,
            )
          ) {
            return false;
          }
        }
      }
    } else if (context && 'imageData' in context && context.imageData) {
      for (let i = this.pdfiumModule.FPDFAnnot_GetObjectCount(annotationPtr) - 1; i >= 0; i--) {
        this.pdfiumModule.FPDFAnnot_RemoveObject(annotationPtr, i);
      }

      if (
        !this.addImageObject(
          doc,
          docPtr,
          page,
          pagePtr,
          annotationPtr,
          annotation.rect,
          context.imageData,
        )
      ) {
        return false;
      }
    } else if (context && 'appearance' in context && context.appearance) {
      /** @deprecated Use { data } instead */
      if (!this.setAppearanceFromPdf(docPtr, annotationPtr, context.appearance)) {
        return false;
      }
    }

    // Apply base annotation properties first so that EPDFRotate / EPDFUnrotatedRect
    // are available when UpdateAppearanceToRect reads them for rotation-aware AP generation.
    if (!this.applyBaseAnnotationProperties(doc, page, pagePtr, annotationPtr, annotation)) {
      return false;
    }

    return !!this.pdfiumModule.EPDFAnnot_UpdateAppearanceToRect(annotationPtr, PdfStampFit.Cover);
  }

  /**
   * Set an annotation's appearance from a single-page PDF document.
   * Loads the PDF into WASM memory, calls the native SetAppearanceFromPage,
   * then cleans up.
   */
  private setAppearanceFromPdf(
    docPtr: number,
    annotationPtr: number,
    appearance: ArrayBuffer,
  ): boolean {
    const data = new Uint8Array(appearance);
    const filePtr = this.memoryManager.malloc(data.byteLength);
    this.pdfiumModule.pdfium.HEAPU8.set(data, filePtr);

    const tempDocPtr = this.pdfiumModule.FPDF_LoadMemDocument(filePtr, data.byteLength, '');
    if (!tempDocPtr) {
      this.memoryManager.free(filePtr);
      return false;
    }

    const ok = this.pdfiumModule.EPDFAnnot_SetAppearanceFromPage(annotationPtr, tempDocPtr, 0);

    this.pdfiumModule.FPDF_CloseDocument(tempDocPtr);
    this.memoryManager.free(filePtr);
    return !!ok;
  }

  /**
   * 将 ImageData 像素数据以 BGRA 格式写入 WASM 内存，创建位图并附加到印章注解。
   *
   * 处理流程：
   *  1. 在 WASM 堆中分配像素缓冲区，将 RGBA → BGRA 逐像素转换写入
   *  2. 用 FPDFBitmap_CreateEx 创建 PDFium 位图句柄
   *  3. 创建图像对象并绑定位图
   *  4. 设置变换矩阵（缩放 + 平移到页面坐标）
   *  5. 将图像对象追加到注解的对象列表
   *
   * @param doc - PDF 文档对象
   * @param docPtr - PDF 文档的 WASM 指针
   * @param page - 页面信息对象
   * @param pagePtr - 页面对象的 WASM 指针
   * @param annotationPtr - 印章注解的 WASM 指针
   * @param rect - 图像在设备空间中的位置和尺寸
   * @param imageData - 浏览器 ImageData 像素数据（RGBA 格式）
   * @returns 是否成功将图像添加到注解
   *
   * @private
   */
  addImageObject(
    doc: PdfDocumentObject,
    docPtr: number,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    rect: Rect,
    imageData: ImageData,
  ) {
    const bytesPerPixel = 4; // 每像素 4 字节（BGRA）
    const pixelCount = imageData.width * imageData.height; // 总像素数

    // 在 WASM 堆中分配像素缓冲区，大小 = 像素数 × 4 字节
    const bitmapBufferPtr = this.memoryManager.malloc(bytesPerPixel * pixelCount);
    if (!bitmapBufferPtr) {
      return false;
    }

    // 批量将 RGBA 转换为 BGRA 写入 WASM 内存
    // PDFium 要求 BGRA 字节序：Blue → Green → Red → Alpha
    // 创建临时 BGRA 缓冲区，一次性转换所有像素
    const bgraData = new Uint8Array(pixelCount * bytesPerPixel);
    for (let i = 0; i < pixelCount; i++) {
      const srcOffset = i * bytesPerPixel;
      const dstOffset = i * bytesPerPixel;
      bgraData[dstOffset] = imageData.data[srcOffset + 2]; // B <- R
      bgraData[dstOffset + 1] = imageData.data[srcOffset + 1]; // G <- G
      bgraData[dstOffset + 2] = imageData.data[srcOffset]; // R <- B
      bgraData[dstOffset + 3] = imageData.data[srcOffset + 3]; // A <- A
    }
    // 一次性写入 WASM 内存，减少调用开销
    this.pdfiumModule.pdfium.HEAPU8.set(bgraData, bitmapBufferPtr);

    // 使用 BGRA 格式创建 PDFium 位图，stride=0 让 PDFium 自动计算行跨度
    const format = BitmapFormat.Bitmap_BGRA;
    const bitmapPtr = this.pdfiumModule.FPDFBitmap_CreateEx(
      imageData.width,
      imageData.height,
      format,
      bitmapBufferPtr,
      0, // stride = 0，自动计算
    );
    if (!bitmapPtr) {
      this.memoryManager.free(bitmapBufferPtr);
      return false;
    }

    // 创建新的 PDF 图像对象（归属于指定文档）
    const imageObjectPtr = this.pdfiumModule.FPDFPageObj_NewImageObj(docPtr);
    if (!imageObjectPtr) {
      this.pdfiumModule.FPDFBitmap_Destroy(bitmapPtr);
      this.memoryManager.free(bitmapBufferPtr);
      return false;
    }

    // 将位图数据绑定到图像对象（page_count=0 表示单页）
    if (!this.pdfiumModule.FPDFImageObj_SetBitmap(pagePtr, 0, imageObjectPtr, bitmapPtr)) {
      this.pdfiumModule.FPDFBitmap_Destroy(bitmapPtr);
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      this.memoryManager.free(bitmapBufferPtr);
      return false;
    }

    // 分配 6×4 = 24 字节的变换矩阵（仿射矩阵 [a, b, c, d, e, f]）
    const matrixPtr = this.memoryManager.malloc(6 * 4);
    // 使用注解矩形的尺寸设置缩放矩阵
    // 这样在外观流 (AP) 调整大小时能与自定义显示尺寸协同工作
    // 矩阵布局: [scaleX, shearY, shearX, scaleY, translateX, translateY]
    this.pdfiumModule.pdfium.setValue(matrixPtr, rect.size.width, 'float'); // scaleX = 宽度
    this.pdfiumModule.pdfium.setValue(matrixPtr + 4, 0, 'float'); // shearY = 0
    this.pdfiumModule.pdfium.setValue(matrixPtr + 8, 0, 'float'); // shearX = 0
    this.pdfiumModule.pdfium.setValue(matrixPtr + 12, rect.size.height, 'float'); // scaleY = 高度
    this.pdfiumModule.pdfium.setValue(matrixPtr + 16, 0, 'float'); // translateX = 0（后续通过 Transform 设置）
    this.pdfiumModule.pdfium.setValue(matrixPtr + 20, 0, 'float'); // translateY = 0
    if (!this.pdfiumModule.FPDFPageObj_SetMatrix(imageObjectPtr, matrixPtr)) {
      this.memoryManager.free(matrixPtr);
      this.pdfiumModule.FPDFBitmap_Destroy(bitmapPtr);
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      this.memoryManager.free(bitmapBufferPtr);
      return false;
    }
    this.memoryManager.free(matrixPtr);

    // 将设备坐标转换为页面坐标（PDF 坐标系原点在左下角）
    // y 偏移 rect.size.height 是因为 PDF 坐标系 y 轴向上
    const pagePos = this.convertDevicePointToPagePoint(doc, page, {
      x: rect.origin.x,
      y: rect.origin.y + rect.size.height, // 向下偏移图像高度，对应 PDF 坐标系原点在左下
    });
    // 应用平移变换，将图像定位到正确的页面坐标
    // 参数: (obj, a, b, c, d, e, f) → 平移矩阵 (1,0,0,1,tx,ty)
    this.pdfiumModule.FPDFPageObj_Transform(imageObjectPtr, 1, 0, 0, 1, pagePos.x, pagePos.y);

    // 将图像对象追加到注解的外观流对象列表
    if (!this.pdfiumModule.FPDFAnnot_AppendObject(annotationPtr, imageObjectPtr)) {
      this.pdfiumModule.FPDFBitmap_Destroy(bitmapPtr);
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      this.memoryManager.free(bitmapBufferPtr);
      return false;
    }

    // 清理：位图和像素缓冲区已拷贝到 PDFium 内部，可以释放
    this.pdfiumModule.FPDFBitmap_Destroy(bitmapPtr);
    this.memoryManager.free(bitmapBufferPtr);

    return true;
  }

  /**
   * 将 PNG 图片以原生格式添加到注解中。
   *
   * 直接将原始 PNG 字节流传给 PDFium，由 PDFium 解码并以 FlateDecode + PNG 预测过滤器
   * 存储，实现最优压缩。相比 addImageObject 避免了 RGBA→BGRA 的像素级转换。
   *
   * @param doc - PDF 文档对象
   * @param docPtr - PDF 文档的 WASM 指针
   * @param page - 页面信息对象
   * @param pagePtr - 页面对象的 WASM 指针
   * @param annotationPtr - 注解的 WASM 指针
   * @param rect - 图像在设备空间中的位置和尺寸
   * @param pngData - PNG 文件的原始字节数据（ArrayBuffer）
   * @returns 是否成功将 PNG 图像添加到注解
   *
   * @private
   */
  addPngImageObject(
    doc: PdfDocumentObject,
    docPtr: number,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    rect: Rect,
    pngData: ArrayBuffer,
  ) {
    // 创建新的 PDF 图像对象
    const imageObjectPtr = this.pdfiumModule.FPDFPageObj_NewImageObj(docPtr);
    if (!imageObjectPtr) {
      return false;
    }

    // 将 PNG 原始字节拷贝到 WASM 堆内存
    const pngBytes = new Uint8Array(pngData);
    const pngPtr = this.memoryManager.malloc(pngBytes.byteLength);
    if (!pngPtr) {
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      return false;
    }
    this.pdfiumModule.pdfium.HEAPU8.set(pngBytes, pngPtr); // 直接批量拷贝，比逐字节快

    // 调用扩展 API 将 PNG 字节解码并绑定到图像对象
    // PDFium 内部会以 FlateDecode + PNG prediction 存储以获得最优压缩
    if (
      !this.pdfiumModule.EPDFImageObj_SetPng(
        pagePtr,
        0,
        imageObjectPtr,
        pngPtr,
        pngBytes.byteLength,
      )
    ) {
      this.memoryManager.free(pngPtr);
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      return false;
    }
    this.memoryManager.free(pngPtr); // PNG 字节已拷贝到 PDFium 内部，释放临时缓冲区

    // 设置变换矩阵：缩放到 rect 指定的尺寸
    const matrixPtr = this.memoryManager.malloc(6 * 4);
    this.pdfiumModule.pdfium.setValue(matrixPtr, rect.size.width, 'float'); // scaleX
    this.pdfiumModule.pdfium.setValue(matrixPtr + 4, 0, 'float'); // shearY
    this.pdfiumModule.pdfium.setValue(matrixPtr + 8, 0, 'float'); // shearX
    this.pdfiumModule.pdfium.setValue(matrixPtr + 12, rect.size.height, 'float'); // scaleY
    this.pdfiumModule.pdfium.setValue(matrixPtr + 16, 0, 'float'); // translateX
    this.pdfiumModule.pdfium.setValue(matrixPtr + 20, 0, 'float'); // translateY
    if (!this.pdfiumModule.FPDFPageObj_SetMatrix(imageObjectPtr, matrixPtr)) {
      this.memoryManager.free(matrixPtr);
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      return false;
    }
    this.memoryManager.free(matrixPtr);

    // 设备坐标 → 页面坐标转换（PDF 坐标系 y 轴向上）
    const pagePos = this.convertDevicePointToPagePoint(doc, page, {
      x: rect.origin.x,
      y: rect.origin.y + rect.size.height,
    });
    this.pdfiumModule.FPDFPageObj_Transform(imageObjectPtr, 1, 0, 0, 1, pagePos.x, pagePos.y);

    // 将图像对象追加到注解
    if (!this.pdfiumModule.FPDFAnnot_AppendObject(annotationPtr, imageObjectPtr)) {
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      return false;
    }

    return true;
  }

  /**
   * 将 JPEG 图片以原生格式添加到注解中。
   *
   * 直接将原始 JPEG 字节流传给 PDFium，以 DCTDecode 流的方式嵌入，
   * 不经过解码/重新编码的往返过程，保持原始 JPEG 质量。
   *
   * @param doc - PDF 文档对象
   * @param docPtr - PDF 文档的 WASM 指针
   * @param page - 页面信息对象
   * @param pagePtr - 页面对象的 WASM 指针
   * @param annotationPtr - 注解的 WASM 指针
   * @param rect - 图像在设备空间中的位置和尺寸
   * @param jpegData - JPEG 文件的原始字节数据（ArrayBuffer）
   * @returns 是否成功将 JPEG 图像添加到注解
   *
   * @private
   */
  addJpegImageObject(
    doc: PdfDocumentObject,
    docPtr: number,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    rect: Rect,
    jpegData: ArrayBuffer,
  ) {
    // 创建新的 PDF 图像对象
    const imageObjectPtr = this.pdfiumModule.FPDFPageObj_NewImageObj(docPtr);
    if (!imageObjectPtr) {
      return false;
    }

    // 将 JPEG 原始字节拷贝到 WASM 堆内存
    const jpegBytes = new Uint8Array(jpegData);
    const jpegPtr = this.memoryManager.malloc(jpegBytes.byteLength);
    if (!jpegPtr) {
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      return false;
    }
    this.pdfiumModule.pdfium.HEAPU8.set(jpegBytes, jpegPtr); // 批量拷贝到 WASM 内存

    // 调用扩展 API 将 JPEG 字节以 DCTDecode 流方式直接嵌入（无解码/重编码）
    if (
      !this.pdfiumModule.EPDFImageObj_SetJpeg(
        pagePtr,
        0,
        imageObjectPtr,
        jpegPtr,
        jpegBytes.byteLength,
      )
    ) {
      this.memoryManager.free(jpegPtr);
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      return false;
    }
    this.memoryManager.free(jpegPtr); // JPEG 字节已拷贝，释放临时缓冲区

    // 设置变换矩阵：缩放到 rect 指定的尺寸
    const matrixPtr = this.memoryManager.malloc(6 * 4);
    this.pdfiumModule.pdfium.setValue(matrixPtr, rect.size.width, 'float'); // scaleX
    this.pdfiumModule.pdfium.setValue(matrixPtr + 4, 0, 'float'); // shearY
    this.pdfiumModule.pdfium.setValue(matrixPtr + 8, 0, 'float'); // shearX
    this.pdfiumModule.pdfium.setValue(matrixPtr + 12, rect.size.height, 'float'); // scaleY
    this.pdfiumModule.pdfium.setValue(matrixPtr + 16, 0, 'float'); // translateX
    this.pdfiumModule.pdfium.setValue(matrixPtr + 20, 0, 'float'); // translateY
    if (!this.pdfiumModule.FPDFPageObj_SetMatrix(imageObjectPtr, matrixPtr)) {
      this.memoryManager.free(matrixPtr);
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      return false;
    }
    this.memoryManager.free(matrixPtr);

    // 设备坐标 → 页面坐标转换
    const pagePos = this.convertDevicePointToPagePoint(doc, page, {
      x: rect.origin.x,
      y: rect.origin.y + rect.size.height,
    });
    this.pdfiumModule.FPDFPageObj_Transform(imageObjectPtr, 1, 0, 0, 1, pagePos.x, pagePos.y);

    // 将图像对象追加到注解
    if (!this.pdfiumModule.FPDFAnnot_AppendObject(annotationPtr, imageObjectPtr)) {
      this.pdfiumModule.FPDFPageObj_Destroy(imageObjectPtr);
      return false;
    }

    return true;
  }

  /**
   * 将 PDF 文档保存为 ArrayBuffer。
   *
   * 使用扩展 API 的文件写入器，先将文档写入内存中的虚拟文件，
   * 再将字节数据拷贝到 JavaScript 的 ArrayBuffer 中返回。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @returns 包含完整 PDF 内容的 ArrayBuffer
   *
   * @private
   */
  saveDocument(docPtr: number) {
    // 打开内存文件写入器
    const writerPtr = this.pdfiumModule.PDFiumExt_OpenFileWriter();
    // 将文档内容写入虚拟文件
    this.pdfiumModule.PDFiumExt_SaveAsCopy(docPtr, writerPtr);
    // 获取写入的文件大小（字节）
    const size = this.pdfiumModule.PDFiumExt_GetFileWriterSize(writerPtr);
    // 分配 WASM 内存缓冲区，准备读取文件数据
    const dataPtr = this.memoryManager.malloc(size);
    this.pdfiumModule.PDFiumExt_GetFileWriterData(writerPtr, dataPtr, size);
    // 将 WASM 内存中的字节数据逐字节拷贝到 JavaScript ArrayBuffer
    const buffer = new ArrayBuffer(size);
    const view = new DataView(buffer);
    for (let i = 0; i < size; i++) {
      view.setInt8(i, this.pdfiumModule.pdfium.getValue(dataPtr + i, 'i8'));
    }
    // 释放 WASM 内存和文件写入器
    this.memoryManager.free(dataPtr);
    this.pdfiumModule.PDFiumExt_CloseFileWriter(writerPtr);

    return buffer;
  }

  /**
   * 读取 PDF 文档 Catalog 中的 /Lang 条目。
   *
   * 通过 EPDFCatalog_GetLanguage 获取语言标签，返回 UTF-16LE 解码后的字符串。
   * 返回值的含义：
   *   null  → /Lang 不存在（getter 返回 0）或文档未打开
   *   ''    → /Lang 存在但值为空字符串
   *   'en', 'en-US' 等 → 正常的语言标签
   *
   * 注意：EPDFCatalog_GetLanguage 返回的长度单位是字节（包含尾部 NUL）
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @returns 语言标签字符串，或 null（不存在时）
   *
   * @private
   */
  private readCatalogLanguage(docPtr: number): string | null {
    // 探测所需缓冲区长度（字节），>>> 0 确保无符号整数
    // 返回值包含 UTF-16LE 尾部的 NUL 终止符
    const byteLen = this.pdfiumModule.EPDFCatalog_GetLanguage(docPtr, 0, 0) >>> 0;

    // 0 表示 /Lang 缺失（或文档/根节点无效），返回 null
    if (byteLen === 0) return null;

    // 2 字节 = 空 UTF-16LE 字符串（仅包含 NUL 终止符），返回空字符串
    if (byteLen === 2) return '';

    // 使用精确大小的缓冲区读取，避免额外分配
    return readString(
      this.pdfiumModule.pdfium,
      (buffer, bufferLength) =>
        this.pdfiumModule.EPDFCatalog_GetLanguage(docPtr, buffer, bufferLength),
      this.pdfiumModule.pdfium.UTF16ToString,
      byteLen,
    );
  }

  /**
   * 从 PDF 文档的 Info 字典中读取指定键的元数据文本。
   *
   * 先检查键是否存在，然后读取 UTF-16LE 编码的值并转换为 JavaScript 字符串。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @param key - 元数据字段名（如 'Title', 'Author', 'Subject' 等）
   * @returns 元数据值字符串，键不存在时返回 null
   *
   * @private
   */
  private readMetaText(docPtr: number, key: string): string | null {
    // 先检查该元数据键是否存在
    const exists = !!this.pdfiumModule.EPDF_HasMetaText(docPtr, key);
    if (!exists) return null;

    // 探测所需缓冲区长度（字节），len=2 表示空字符串（仅 NUL 终止符）
    const len = this.pdfiumModule.FPDF_GetMetaText(docPtr, key, 0, 0);
    if (len === 2) return '';

    // 使用精确大小的缓冲区读取，避免额外内存分配
    return readString(
      this.pdfiumModule.pdfium,
      (buffer, bufferLength) =>
        this.pdfiumModule.FPDF_GetMetaText(docPtr, key, buffer, bufferLength),
      this.pdfiumModule.pdfium.UTF16ToString,
      len,
    );
  }

  /**
   * 向 PDF 文档的 Info 字典写入元数据。
   *
   * 如果 value 为 null 或空字符串，则删除该键。
   * 将 JavaScript 字符串编码为 UTF-16LE 后通过 WASM 写入。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @param key - 元数据字段名
   * @param value - 要写入的值，null/undefined/空字符串 表示删除
   * @returns 是否成功写入
   *
   * @private
   */
  private setMetaText(docPtr: number, key: string, value: string | null | undefined): boolean {
    // 如果值为 null/undefined/空字符串，删除该键
    if (value == null || value.length === 0) {
      // 传入 nullptr 作为 value → C++ 层执行删除操作
      const ok = this.pdfiumModule.EPDF_SetMetaText(docPtr, key, 0);
      return !!ok;
    }

    // 计算UTF-16LE 缓冲区大小：每个字符 2 字节 + 尾部 NUL 2 字节
    const bytes = 2 * (value.length + 1);
    const ptr = this.memoryManager.malloc(bytes);
    try {
      // 将字符串编码为 UTF-16LE 写入 WASM 内存
      this.pdfiumModule.pdfium.stringToUTF16(value, ptr, bytes);
      const ok = this.pdfiumModule.EPDF_SetMetaText(docPtr, key, ptr);
      return !!ok;
    } finally {
      this.memoryManager.free(ptr); // 确保释放 WASM 内存
    }
  }

  /**
   * 读取 PDF 文档的 /Trapped 状态。
   *
   * /Trapped 是 PDF 规范中的标志，指示文档是否已被"陷印"处理。
   * 如果 PDFium 返回非预期值，则回退为 Unknown。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @returns PdfTrappedStatus 枚举值
   *
   * @private
   */
  private getMetaTrapped(docPtr: number): PdfTrappedStatus {
    // 从 PDFium 读取原始的 Trapped 状态值
    const raw = Number(this.pdfiumModule.EPDF_GetMetaTrapped(docPtr));
    switch (raw) {
      case PdfTrappedStatus.NotSet:  // 未设置
      case PdfTrappedStatus.True:    // 已陷印
      case PdfTrappedStatus.False:   // 未陷印
      case PdfTrappedStatus.Unknown: // 未知
        return raw;
      default:
        // 非预期值回退为 Unknown
        return PdfTrappedStatus.Unknown;
    }
  }

  /**
   * 写入或清除 PDF 文档的 /Trapped 状态。
   *
   * 传入 null/undefined 将删除 /Trapped 键（等同 NotSet）。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @param status - Trapped 状态值，null/undefined 表示删除
   * @returns 是否成功写入
   *
   * @private
   */
  private setMetaTrapped(docPtr: number, status: PdfTrappedStatus | null | undefined): boolean {
    // null/undefined 视为删除键，C++ 层处理 NotSet 时会从 Info 字典中删除 /Trapped
    const toSet = status == null || status === undefined ? PdfTrappedStatus.NotSet : status;

    // 校验枚举值的有效性，防止传入非法值
    const valid =
      toSet === PdfTrappedStatus.NotSet ||
      toSet === PdfTrappedStatus.True ||
      toSet === PdfTrappedStatus.False ||
      toSet === PdfTrappedStatus.Unknown;

    if (!valid) return false;

    return !!this.pdfiumModule.EPDF_SetMetaTrapped(docPtr, toSet);
  }

  /**
   * 获取 PDF 文档 Info 字典中的键数量。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @param customOnly - true 时只计算自定义键（排除保留键），false 时计算所有键
   * @returns 键的数量（可能为 0），出错时返回 0
   *
   * @private
   */
  private getMetaKeyCount(docPtr: number, customOnly: boolean): number {
    // 转为 JavaScript 数字，| 0 确保为 32 位整数
    return Number(this.pdfiumModule.EPDF_GetMetaKeyCount(docPtr, customOnly)) | 0;
  }

  /**
   * 获取 Info 字典中指定索引位置的键名。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @param index - 从 0 开始的键索引，按 PDFium 返回的顺序
   * @param customOnly - true 时仅遍历自定义键，false 时遍历所有键
   * @returns 键名字符串，未找到时返回 null
   *
   * @private
   */
  private getMetaKeyName(docPtr: number, index: number, customOnly: boolean): string | null {
    // 探测键名字符串所需缓冲区长度
    const len = this.pdfiumModule.EPDF_GetMetaKeyName(docPtr, index, customOnly, 0, 0);
    if (!len) return null;
    // 键名使用 UTF-8 编码读取
    return readString(
      this.pdfiumModule.pdfium,
      (buffer, buflen) =>
        this.pdfiumModule.EPDF_GetMetaKeyName(docPtr, index, customOnly, buffer, buflen),
      this.pdfiumModule.pdfium.UTF8ToString,
      len,
    );
  }

  /**
   * 读取 PDF 文档 Info 字典中的所有元数据。
   *
   * 遍历所有键并逐个读取对应的值。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @param customOnly - true 时仅读取自定义键，false 时读取所有键。默认 true
   * @returns 键值对对象，值为 null 表示键存在但无值
   *
   * @private
   */
  private readAllMeta(docPtr: number, customOnly: boolean = true): Record<string, string | null> {
    const n = this.getMetaKeyCount(docPtr, customOnly);
    const out: Record<string, string | null> = {};
    for (let i = 0; i < n; i++) {
      const key = this.getMetaKeyName(docPtr, i, customOnly);
      if (!key) continue;
      out[key] = this.readMetaText(docPtr, key); // returns null if not present
    }
    return out;
  }

  /**
   * 递归读取 PDF 文档中的书签（大纲）树。
   *
   * 从指定父节点的第一个子节点开始，逐个遍历同级书签。
   * 通常传入 rootBookmarkPtr=0 表示从根开始。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @param rootBookmarkPtr - 父书签的指针，0 表示从根节点开始。默认 0
   * @returns 当前层级下的书签数组
   *
   * @private
   */
  readPdfBookmarks(docPtr: number, rootBookmarkPtr = 0) {
    // 获取父节点的第一个子书签
    let bookmarkPtr = this.pdfiumModule.FPDFBookmark_GetFirstChild(docPtr, rootBookmarkPtr);

    const bookmarks: PdfBookmarkObject[] = [];
    // 遍历同级书签链表
    while (bookmarkPtr) {
      // 递归读取单个书签（含子书签）
      const bookmark = this.readPdfBookmark(docPtr, bookmarkPtr);
      bookmarks.push(bookmark);

      // 获取下一个同级书签
      const nextBookmarkPtr = this.pdfiumModule.FPDFBookmark_GetNextSibling(docPtr, bookmarkPtr);

      bookmarkPtr = nextBookmarkPtr;
    }

    return bookmarks;
  }

  /**
   * 读取单个书签的完整信息（标题、目标、子书签）。
   *
   * @param docPtr - PDF 文档的 WASM 指针
   * @param bookmarkPtr - 书签对象的 WASM 指针
   * @returns 包含标题、目标和子书签的书签对象
   *
   * @private
   */
  private readPdfBookmark(docPtr: number, bookmarkPtr: number): PdfBookmarkObject {
    // 读取书签标题（UTF-16LE 编码）
    const title = readString(
      this.pdfiumModule.pdfium,
      (buffer, bufferLength) => {
        return this.pdfiumModule.FPDFBookmark_GetTitle(bookmarkPtr, buffer, bufferLength);
      },
      this.pdfiumModule.pdfium.UTF16ToString,
    );

    // 递归读取子书签列表
    const bookmarks = this.readPdfBookmarks(docPtr, bookmarkPtr);

    // 读取书签的跳转目标（可能是页面内跳转或 URI 外部链接）
    const target = this.readPdfBookmarkTarget(
      docPtr,
      () => {
        return this.pdfiumModule.FPDFBookmark_GetAction(bookmarkPtr); // 获取 Action 句柄
      },
      () => {
        return this.pdfiumModule.FPDFBookmark_GetDest(docPtr, bookmarkPtr); // 获取 Dest 句柄
      },
    );

    return {
      title,
      target,
      children: bookmarks,
    };
  }

  /**
   * 读取 PDF 页面中所有文本矩形的完整信息。
   *
   * 遍历页面上的每个文本矩形区域，获取其：
   *  - 设备空间坐标和尺寸（通过 PageToDevice 坐标变换）
   *  - 区域内的文本内容（通过 GetBoundedText）
   *  - 字体信息（字体族名称和字号）
   *
   * @param page - PDF 页面信息对象
   * @param docPtr - PDF 文档的 WASM 指针
   * @param pagePtr - 页面对象的 WASM 指针
   * @param textPagePtr - 文本页面对象的 WASM 指针
   * @returns 文本矩形对象数组
   *
   * @public
   */
  private readPageTextRects(
    page: PdfPageObject,
    docPtr: number,
    pagePtr: number,
    textPagePtr: number,
  ) {
    // 获取页面上所有文本矩形的数量（从字符 0 开始，-1 表示到末尾）
    const rectsCount = this.pdfiumModule.FPDFText_CountRects(textPagePtr, 0, -1);

    const textRects: PdfTextRectObject[] = [];
    for (let i = 0; i < rectsCount; i++) {
      // 为四个坐标值分配 WASM 内存（每个 double = 8 字节）
      const topPtr = this.memoryManager.malloc(8);
      const leftPtr = this.memoryManager.malloc(8);
      const rightPtr = this.memoryManager.malloc(8);
      const bottomPtr = this.memoryManager.malloc(8);
      // 读取第 i 个文本矩形的页面坐标（left, top, right, bottom）
      const isSucceed = this.pdfiumModule.FPDFText_GetRect(
        textPagePtr,
        i,
        leftPtr,
        topPtr,
        rightPtr,
        bottomPtr,
      );
      if (!isSucceed) {
        this.memoryManager.free(leftPtr);
        this.memoryManager.free(topPtr);
        this.memoryManager.free(rightPtr);
        this.memoryManager.free(bottomPtr);
        continue;
      }

      // 从 WASM 内存读取 double 类型的坐标值
      const left = this.pdfiumModule.pdfium.getValue(leftPtr, 'double');
      const top = this.pdfiumModule.pdfium.getValue(topPtr, 'double');
      const right = this.pdfiumModule.pdfium.getValue(rightPtr, 'double');
      const bottom = this.pdfiumModule.pdfium.getValue(bottomPtr, 'double');

      // 释放坐标值的 WASM 内存
      this.memoryManager.free(leftPtr);
      this.memoryManager.free(topPtr);
      this.memoryManager.free(rightPtr);
      this.memoryManager.free(bottomPtr);

      // 将页面坐标左上角转换为设备坐标（像素）
      const deviceXPtr = this.memoryManager.malloc(4); // int32 = 4 字节
      const deviceYPtr = this.memoryManager.malloc(4);
      this.pdfiumModule.FPDF_PageToDevice(
        pagePtr,
        0,
        0,
        page.size.width,
        page.size.height,
        0,
        left,
        top,
        deviceXPtr,
        deviceYPtr,
      );
      const x = this.pdfiumModule.pdfium.getValue(deviceXPtr, 'i32');
      const y = this.pdfiumModule.pdfium.getValue(deviceYPtr, 'i32');
      this.memoryManager.free(deviceXPtr);
      this.memoryManager.free(deviceYPtr);

      // 构造设备空间中的矩形（坐标原点在左上角）
      const rect = {
        origin: {
          x,
          y,
        },
        size: {
          width: Math.ceil(Math.abs(right - left)), // 宽度向上取整
          height: Math.ceil(Math.abs(top - bottom)), // 高度向上取整
        },
      };

      // 读取该矩形区域内的文本内容
      // 先探测所需的 UTF-16 缓冲区长度
      const utf16Length = this.pdfiumModule.FPDFText_GetBoundedText(
        textPagePtr,
        left,
        top,
        right,
        bottom,
        0,
        0,
      );
      const bytesCount = (utf16Length + 1) * 2; // +1 包含 NUL 终止符，×2 为 UTF-16 每字符字节数
      const textBuffer = this.memoryManager.malloc(bytesCount);
      // 第二次调用，实际读取文本数据到缓冲区
      this.pdfiumModule.FPDFText_GetBoundedText(
        textPagePtr,
        left,
        top,
        right,
        bottom,
        textBuffer,
        utf16Length,
      );
      // 将 UTF-16LE 缓冲区解码为 JavaScript 字符串
      const content = this.pdfiumModule.pdfium.UTF16ToString(textBuffer);
      this.memoryManager.free(textBuffer);

      // 通过矩形左上角位置查找对应的字符索引，容差 2 像素
      const charIndex = this.pdfiumModule.FPDFText_GetCharIndexAtPos(textPagePtr, left, top, 2, 2);
      let fontFamily = '';
      let fontSize = rect.size.height; // 默认使用矩形高度作为字号
      if (charIndex >= 0) {
        // 读取该字符的实际字号（PDF 点数）
        fontSize = this.pdfiumModule.FPDFText_GetFontSize(textPagePtr, charIndex);

        // 探测字体名称所需缓冲区长度
        const fontNameLength = this.pdfiumModule.FPDFText_GetFontInfo(
          textPagePtr,
          charIndex,
          0,
          0,
          0,
        );

        // 读取字体名称（UTF-8 编码）和字体标志
        const bytesCount = fontNameLength + 1; // +1 包含 NUL 终止符
        const textBufferPtr = this.memoryManager.malloc(bytesCount);
        const flagsPtr = this.memoryManager.malloc(4); // 字体标志（int32）
        this.pdfiumModule.FPDFText_GetFontInfo(
          textPagePtr,
          charIndex,
          textBufferPtr,
          bytesCount,
          flagsPtr,
        );
        // UTF-8 解码字体族名称
        fontFamily = this.pdfiumModule.pdfium.UTF8ToString(textBufferPtr);
        this.memoryManager.free(textBufferPtr);
        this.memoryManager.free(flagsPtr);
      }

      const textRect: PdfTextRectObject = {
        content,
        rect,
        font: {
          family: fontFamily,
          size: fontSize,
        },
      };

      textRects.push(textRect);
    }

    return textRects;
  }

  /**
   * 获取单页的几何和逻辑文本布局（基于字形实现，不使用 FPDFText_GetRect）。
   *
   * 返回页面上的所有文本运行（run），每个运行包含一组属于同一文本对象的字形。
   * 适用于需要精确字形位置信息的场景（如文本选择、搜索高亮）。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @returns 包含 runs 数组的 PdfPageGeometry 对象的异步任务
   *
   * @public
   */
  getPageGeometry(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfPageGeometry> {
    const label = 'getPageGeometry';
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'Begin', doc.id);

    /* ── guards ───────────────────────────────────────────── */
    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    /* ── native handles ──────────────────────────────────── */
    const pageCtx = ctx.acquirePage(page.index);
    const textPagePtr = pageCtx.getTextPage();

    /* ── 1. read ALL glyphs in logical order ─────────────── */
    const glyphCount = this.pdfiumModule.FPDFText_CountChars(textPagePtr);
    const glyphs: PdfGlyphObject[] = [];

    for (let i = 0; i < glyphCount; i++) {
      const g = this.readGlyphInfo(page, pageCtx.pagePtr, textPagePtr, i);
      glyphs.push(g);
    }

    /* ── 2. build visual runs from glyph stream ───────────── */
    const runs: PdfRun[] = this.buildRunsFromGlyphs(glyphs, textPagePtr);

    /* ── 2b. fetch RAW page text (未 strip) ───────────────── */
    // 与 runs 共享同一字符索引空间——调用方（hover 翻译/高亮）依赖此对齐。
    // 不能用 stripPdfUnwantedMarkers，否则 text.length < runs 总字形数，索引错位。
    let rawPageText = '';
    if (glyphCount > 0) {
      const bufPtr = this.memoryManager.malloc(2 * (glyphCount + 1)); // UTF-16 + NIL
      this.pdfiumModule.FPDFText_GetText(textPagePtr, 0, glyphCount, bufPtr);
      rawPageText = this.pdfiumModule.pdfium.UTF16ToString(bufPtr);
      this.memoryManager.free(bufPtr);
    }

    /* ── 3. cleanup & resolve task ───────────────────────── */
    pageCtx.release();

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', doc.id);
    return PdfTaskHelper.resolve({ runs, text: rawPageText });
  }

  /**
   * 将连续的同源字形（属于同一 CPDF_TextObject）分组为运行（Run）。
   *
   * 通过 FPDFText_GetTextObject 判断每个字形所属的文本对象，
   * 当文本对象发生变化时创建新的运行。同时维护运行的整体包围盒。
   *
   * @param glyphs - 字形对象数组
   * @param textPagePtr - 文本页面对象的 WASM 指针
   * @returns 文本运行数组
   */
  private buildRunsFromGlyphs(glyphs: PdfGlyphObject[], textPagePtr: number): PdfRun[] {
    const runs: PdfRun[] = [];
    let current: PdfRun | null = null; // 当前正在构建的运行
    let curObjPtr: number | null = null; // 当前文本对象的指针
    let bounds: { minX: number; minY: number; maxX: number; maxY: number } | null = null; // 当前运行的包围盒

    /** ── 主循环：遍历所有字形 ──────────────────────────────────── */
    for (let i = 0; i < glyphs.length; i++) {
      const g = glyphs[i];

      /* 1 — 获取该字形所属的 CPDF_TextObject 指针 */
      const objPtr = this.pdfiumModule.FPDFText_GetTextObject(textPagePtr, i) as number;

      /* 2 — 当文本对象变化时，创建新的运行 */
      if (objPtr !== curObjPtr) {
        curObjPtr = objPtr;
        current = {
          rect: { // 初始矩形为第一个字形的位置和尺寸
            x: g.origin.x,
            y: g.origin.y,
            width: g.size.width,
            height: g.size.height,
          },
          charStart: i, // 该运行在字形数组中的起始索引
          glyphs: [],
          fontSize: this.pdfiumModule.FPDFText_GetFontSize(textPagePtr, i), // 获取字号
        };
        bounds = { // 初始化包围盒
          minX: g.origin.x,
          minY: g.origin.y,
          maxX: g.origin.x + g.size.width,
          maxY: g.origin.y + g.size.height,
        };
        runs.push(current);
      }

      /* 3 — 将精简字形记录追加到当前运行 */
      current!.glyphs.push({
        x: g.origin.x,
        y: g.origin.y,
        width: g.size.width,
        height: g.size.height,
        flags: g.isEmpty ? 2 : g.isSpace ? 1 : 0, // 0=普通, 1=空格, 2=空字形
        ...(g.tightOrigin && { tightX: g.tightOrigin.x, tightY: g.tightOrigin.y }), // 紧凑包围盒原点（可选）
        ...(g.tightSize && { tightWidth: g.tightSize.width, tightHeight: g.tightSize.height }), // 紧凑包围盒尺寸（可选）
      });

      /* 4 — 扩展运行的包围盒（跳过空字形） */
      if (g.isEmpty) {
        continue;
      }

      const right = g.origin.x + g.size.width;
      const bottom = g.origin.y + g.size.height;

      // 更新包围盒的最小/最大边界
      bounds!.minX = Math.min(bounds!.minX, g.origin.x);
      bounds!.minY = Math.min(bounds!.minY, g.origin.y);
      bounds!.maxX = Math.max(bounds!.maxX, right);
      bounds!.maxY = Math.max(bounds!.maxY, bottom);

      // 从包围盒计算最终的矩形位置和尺寸
      current!.rect.x = bounds!.minX;
      current!.rect.y = bounds!.minY;
      current!.rect.width = bounds!.maxX - bounds!.minX;
      current!.rect.height = bounds!.maxY - bounds!.minY;
    }

    return runs;
  }

  /**
   * 获取页面上的富文本运行信息。
   *
   * 将共享相同文本对象、字体、字号和填充颜色的连续字符分组为结构化的文本段。
   * 每个文本段包含完整的字体元数据和 PDF 页面坐标中的包围盒。
   * 适用于需要文本内容 + 样式信息的场景（如文本导出、格式保留复制）。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @returns 包含 runs 数组的 PdfPageTextRuns 对象的异步任务
   *
   * @public
   */
  getPageTextRuns(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfPageTextRuns> {
    const label = 'getPageTextRuns';
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'Begin', doc.id);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    const textPagePtr = pageCtx.getTextPage();
    const charCount = this.pdfiumModule.FPDFText_CountChars(textPagePtr);

    // ── 状态变量：追踪当前正在构建的文本段 ──────────────────────
    const runs: PdfTextRun[] = [];
    let runStart = 0; // 当前文本段的起始字符索引
    let curObjPtr: number | null = null; // 当前字符所属的 FPDF 文本对象指针
    let curFont: PdfFontInfo | null = null; // 当前文本段的字体信息
    let curFontSize = 0; // 当前文本段的字号
    let curColor: PdfAlphaColor | null = null; // 当前文本段的填充颜色
    let bounds: { minX: number; minY: number; maxX: number; maxY: number } | null = null; // 当前文本段的设备空间包围盒

    /**
     * 将从 runStart 到 end 的字符刷新为一个完整的 PdfTextRun 并推入 runs 数组。
     * 仅在文本对象指针、字体、颜色和包围盒均有效时才会产出结果。
     *
     * @param end - 结束字符索引（不包含），即下一个文本段的起始位置
     */
    const flushRun = (end: number) => {
      if (curObjPtr === null || curFont === null || curColor === null || bounds === null) return;
      const count = end - runStart;
      if (count <= 0) return;

      // 通过 FPDFText_GetText 提取 UTF-16 文本并转换为 JS 字符串
      const bufPtr = this.memoryManager.malloc(2 * (count + 1));
      this.pdfiumModule.FPDFText_GetText(textPagePtr, runStart, count, bufPtr);
      const text = stripPdfUnwantedMarkers(this.pdfiumModule.pdfium.UTF16ToString(bufPtr));
      this.memoryManager.free(bufPtr);

      runs.push({
        text,
        rect: {
          origin: { x: bounds.minX, y: bounds.minY },
          size: {
            width: Math.max(1, bounds.maxX - bounds.minX),
            height: Math.max(1, bounds.maxY - bounds.minY),
          },
        },
        font: curFont,
        fontSize: curFontSize,
        color: curColor,
        charIndex: runStart,
        charCount: count,
      });
    };

    // ── 预分配 WASM 堆内存用于存放颜色、矩形和坐标变换结果 ────
    // 每个指针在循环中反复使用，循环结束后统一释放
    const rPtr = this.memoryManager.malloc(4); // 红色分量
    const gPtr = this.memoryManager.malloc(4); // 绿色分量
    const bPtr = this.memoryManager.malloc(4); // 蓝色分量
    const aPtr = this.memoryManager.malloc(4); // 透明度分量
    const rectPtr = this.memoryManager.malloc(16); // FS_RECTF: 4 个 float（16 字节）
    const dx1Ptr = this.memoryManager.malloc(4); // 设备坐标 X1（用于 FPDF_PageToDevice）
    const dy1Ptr = this.memoryManager.malloc(4); // 设备坐标 Y1
    const dx2Ptr = this.memoryManager.malloc(4); // 设备坐标 X2
    const dy2Ptr = this.memoryManager.malloc(4); // 设备坐标 Y2
    const italicAnglePtr = this.memoryManager.malloc(4); // 斜体角度输出参数

    // ── 遍历每个字符，按文本对象/字体/字号/颜色分组合并成文本段 ──
    for (let i = 0; i < charCount; i++) {
      // 获取字符的 Unicode 码点，跳过无效的替换字符
      const uc = this.pdfiumModule.FPDFText_GetUnicode(textPagePtr, i);
      if (uc === 0xfffe || uc === 0xfffd) continue;

      // 获取该字符所属的底层 PDF 文本对象指针（用于判断是否属于同一段落）
      const objPtr = this.pdfiumModule.FPDFText_GetTextObject(textPagePtr, i) as number;
      if (objPtr === 0) continue;

      const fontSize = this.pdfiumModule.FPDFText_GetFontSize(textPagePtr, i);

      // 读取字符的填充颜色（RGBA）
      this.pdfiumModule.FPDFText_GetFillColor(textPagePtr, i, rPtr, gPtr, bPtr, aPtr);
      const red = this.pdfiumModule.pdfium.getValue(rPtr, 'i32') & 0xff;
      const green = this.pdfiumModule.pdfium.getValue(gPtr, 'i32') & 0xff;
      const blue = this.pdfiumModule.pdfium.getValue(bPtr, 'i32') & 0xff;
      const alpha = this.pdfiumModule.pdfium.getValue(aPtr, 'i32') & 0xff;

      // 从文本对象中读取完整的字体元数据
      const fontInfo = this.readFontInfoFromTextObject(objPtr, italicAnglePtr);

      // 判断是否需要开始一个新的文本段：
      // 当文本对象指针、字体名称、字号（允许 0.01 的浮点误差）或颜色任一发生变化时
      const needNewRun =
        curObjPtr === null ||
        objPtr !== curObjPtr ||
        fontInfo.name !== curFont!.name ||
        Math.abs(fontSize - curFontSize) > 0.01 ||
        red !== curColor!.red ||
        green !== curColor!.green ||
        blue !== curColor!.blue;

      if (needNewRun) {
        flushRun(i); // 将当前累积的字符刷新为一段
        curObjPtr = objPtr;
        curFont = fontInfo;
        curFontSize = fontSize;
        curColor = { red, green, blue, alpha };
        runStart = i;
        bounds = null; // 重置包围盒，等待新字符扩展
      }

      // 用当前字符的松散包围盒（LooseCharBox）扩展当前文本段的包围盒
      // LooseCharBox 返回页面坐标系（page user space）中的矩形
      if (this.pdfiumModule.FPDFText_GetLooseCharBox(textPagePtr, i, rectPtr)) {
        const left = this.pdfiumModule.pdfium.getValue(rectPtr, 'float');
        const top = this.pdfiumModule.pdfium.getValue(rectPtr + 4, 'float');
        const right = this.pdfiumModule.pdfium.getValue(rectPtr + 8, 'float');
        const bottom = this.pdfiumModule.pdfium.getValue(rectPtr + 12, 'float');

        // 跳过零宽或零高的退化字符
        if (left !== right && top !== bottom) {
          // 将页面坐标的左上角转换为设备坐标（像素空间）
          this.pdfiumModule.FPDF_PageToDevice(
            pageCtx.pagePtr,
            0,
            0,
            page.size.width,
            page.size.height,
            0,
            left,
            top,
            dx1Ptr,
            dy1Ptr,
          );
          // 将页面坐标的右下角转换为设备坐标
          this.pdfiumModule.FPDF_PageToDevice(
            pageCtx.pagePtr,
            0,
            0,
            page.size.width,
            page.size.height,
            0,
            right,
            bottom,
            dx2Ptr,
            dy2Ptr,
          );

          // 读取转换后的设备坐标整数像素值
          const x1 = this.pdfiumModule.pdfium.getValue(dx1Ptr, 'i32');
          const y1 = this.pdfiumModule.pdfium.getValue(dy1Ptr, 'i32');
          const x2 = this.pdfiumModule.pdfium.getValue(dx2Ptr, 'i32');
          const y2 = this.pdfiumModule.pdfium.getValue(dy2Ptr, 'i32');
          // 计算轴对齐的矩形（处理可能的翻转坐标）
          const cx = Math.min(x1, x2);
          const cy = Math.min(y1, y2);
          const cw = Math.abs(x2 - x1);
          const ch = Math.abs(y2 - y1);

          // 合并当前字符矩形到文本段的包围盒（取所有字符的最小/最大边界）
          if (bounds === null) {
            bounds = { minX: cx, minY: cy, maxX: cx + cw, maxY: cy + ch };
          } else {
            bounds.minX = Math.min(bounds.minX, cx);
            bounds.minY = Math.min(bounds.minY, cy);
            bounds.maxX = Math.max(bounds.maxX, cx + cw);
            bounds.maxY = Math.max(bounds.maxY, cy + ch);
          }
        }
      }
    }

    // 将最后一段累积的字符刷新出来
    flushRun(charCount);

    // 释放所有预分配的 WASM 堆内存
    [rPtr, gPtr, bPtr, aPtr, rectPtr, dx1Ptr, dy1Ptr, dx2Ptr, dy2Ptr, italicAnglePtr].forEach((p) =>
      this.memoryManager.free(p),
    );

    pageCtx.release();
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', doc.id);
    return PdfTaskHelper.resolve({ runs });
  }

  /**
   * 从 FPDF 文本对象中读取完整的字体元数据。
   *
   * 通过 FPDFFont_* 系列 API 提取字体的 PostScript 名称、家族名称、
   * 字重（weight）、是否斜体、是否等宽以及是否内嵌等信息。
   *
   * @param textObjPtr - FPDF 文本对象指针（由 FPDFText_GetTextObject 返回）
   * @param italicAnglePtr - 预分配的 WASM 指针，用于接收斜体角度（避免重复分配）
   * @returns 包含完整字体元数据的 PdfFontInfo 对象
   *
   * @private
   */
  private readFontInfoFromTextObject(textObjPtr: number, italicAnglePtr: number): PdfFontInfo {
    const fontPtr = this.pdfiumModule.FPDFTextObj_GetFont(textObjPtr);

    let name = '';
    let familyName = '';
    let weight = 400;
    let italic = false;
    let monospaced = false;
    let embedded = false;

    if (fontPtr) {
      // 通过 FPDFFont_GetBaseFontName 获取字体的 PostScript 名称
      const nameLen = this.pdfiumModule.FPDFFont_GetBaseFontName(fontPtr, 0, 0);
      if (nameLen > 0) {
        const nameBuf = this.memoryManager.malloc(nameLen + 1);
        this.pdfiumModule.FPDFFont_GetBaseFontName(fontPtr, nameBuf, nameLen + 1);
        name = this.pdfiumModule.pdfium.UTF8ToString(nameBuf);
        this.memoryManager.free(nameBuf);
      }

      // 通过 FPDFFont_GetFamilyName 获取字体的家族名称
      const famLen = this.pdfiumModule.FPDFFont_GetFamilyName(fontPtr, 0, 0);
      if (famLen > 0) {
        const famBuf = this.memoryManager.malloc(famLen + 1);
        this.pdfiumModule.FPDFFont_GetFamilyName(fontPtr, famBuf, famLen + 1);
        familyName = this.pdfiumModule.pdfium.UTF8ToString(famBuf);
        this.memoryManager.free(famBuf);
      }

      weight = this.pdfiumModule.FPDFFont_GetWeight(fontPtr);
      embedded = this.pdfiumModule.FPDFFont_GetIsEmbedded(fontPtr) !== 0;

      if (this.pdfiumModule.FPDFFont_GetItalicAngle(fontPtr, italicAnglePtr)) {
        const angle = this.pdfiumModule.pdfium.getValue(italicAnglePtr, 'i32');
        italic = angle !== 0;
      }

      // 字体标志的 Bit 0 = FixedPitch（等宽字体）
      const flags = this.pdfiumModule.FPDFFont_GetFlags(fontPtr);
      monospaced = (flags & 1) !== 0;
    }

    return { name, familyName, weight, italic, monospaced, embedded };
  }

  /**
   * 提取单个字符的字形几何信息和元数据。
   *
   * 分三步获取字形位置：
   *   1. 使用 FPDFText_GetLooseCharBox 获取松散包围盒（页面坐标系）
   *   2. 通过 FPDF_PageToDevice 将松散包围盒的角点映射到设备空间（像素坐标）
   *   3. 使用 FPDFText_GetCharBox 获取紧凑包围盒（tight bbox，同样映射到设备空间）
   *
   * 返回值说明：
   *   origin.x/y  → 左上角坐标（整数像素）
   *   size.width/height → 宽高（整数像素，最小为 1）
   *   isSpace → 当 Unicode 码点为 U+0020 时为 true
   *   tightOrigin/tightSize → 紧凑包围盒（可选，仅在获取成功时存在）
   *
   * @param page - PDF 页面信息对象
   * @param pagePtr - FPDF 页面指针
   * @param textPagePtr - FPDF 文本页面指针
   * @param charIndex - 字符索引（0 到 charCount-1）
   * @returns PdfGlyphObject 字形几何对象
   *
   * @private
   */
  private readGlyphInfo(
    page: PdfPageObject,
    pagePtr: number,
    textPagePtr: number,
    charIndex: number,
  ): PdfGlyphObject {
    // ── native stack temp pointers ──────────────────────────────
    const dx1Ptr = this.memoryManager.malloc(4);
    const dy1Ptr = this.memoryManager.malloc(4);
    const dx2Ptr = this.memoryManager.malloc(4);
    const dy2Ptr = this.memoryManager.malloc(4);
    const rectPtr = this.memoryManager.malloc(16); // 4 floats = 16 bytes
    const tLeftPtr = this.memoryManager.malloc(8);
    const tRightPtr = this.memoryManager.malloc(8);
    const tBottomPtr = this.memoryManager.malloc(8);
    const tTopPtr = this.memoryManager.malloc(8);

    const allPtrs = [
      rectPtr,
      dx1Ptr,
      dy1Ptr,
      dx2Ptr,
      dy2Ptr,
      tLeftPtr,
      tRightPtr,
      tBottomPtr,
      tTopPtr,
    ];

    let x = 0,
      y = 0,
      width = 0,
      height = 0,
      isSpace = false;
    let tightOrigin: { x: number; y: number } | undefined; // 紧凑包围盒原点（可选）
    let tightSize: { width: number; height: number } | undefined; // 紧凑包围盒尺寸（可选）

    // ── 步骤 1：获取松散包围盒（LooseCharBox，页面坐标系） ──────
    if (this.pdfiumModule.FPDFText_GetLooseCharBox(textPagePtr, charIndex, rectPtr)) {
      const left = this.pdfiumModule.pdfium.getValue(rectPtr, 'float');
      const top = this.pdfiumModule.pdfium.getValue(rectPtr + 4, 'float');
      const right = this.pdfiumModule.pdfium.getValue(rectPtr + 8, 'float');
      const bottom = this.pdfiumModule.pdfium.getValue(rectPtr + 12, 'float');

      // 零宽或零高的退化字形，直接标记为空
      if (left === right || top === bottom) {
        allPtrs.forEach((p) => this.memoryManager.free(p));

        return {
          origin: { x: 0, y: 0 },
          size: { width: 0, height: 0 },
          isEmpty: true,
        };
      }

      // ── 步骤 2：将松散包围盒的左上角和右下角从页面空间映射到设备空间 ──
      this.pdfiumModule.FPDF_PageToDevice(
        pagePtr,
        0,
        0,
        page.size.width,
        page.size.height,
        0,
        left,
        top,
        dx1Ptr,
        dy1Ptr,
      );
      this.pdfiumModule.FPDF_PageToDevice(
        pagePtr,
        0,
        0,
        page.size.width,
        page.size.height,
        0,
        right,
        bottom,
        dx2Ptr,
        dy2Ptr,
      );

      const x1 = this.pdfiumModule.pdfium.getValue(dx1Ptr, 'i32');
      const y1 = this.pdfiumModule.pdfium.getValue(dy1Ptr, 'i32');
      const x2 = this.pdfiumModule.pdfium.getValue(dx2Ptr, 'i32');
      const y2 = this.pdfiumModule.pdfium.getValue(dy2Ptr, 'i32');

      // 计算轴对齐的设备空间矩形（处理可能的坐标翻转）
      x = Math.min(x1, x2);
      y = Math.min(y1, y2);
      width = Math.max(1, Math.abs(x2 - x1)); // 最小 1 像素宽
      height = Math.max(1, Math.abs(y2 - y1)); // 最小 1 像素高

      // ── 步骤 3：获取紧凑包围盒（CharBox，更精确的字形边界） ────
      if (
        this.pdfiumModule.FPDFText_GetCharBox(
          textPagePtr,
          charIndex,
          tLeftPtr,
          tRightPtr,
          tBottomPtr,
          tTopPtr,
        )
      ) {
        const tLeft = this.pdfiumModule.pdfium.getValue(tLeftPtr, 'double');
        const tRight = this.pdfiumModule.pdfium.getValue(tRightPtr, 'double');
        const tBottom = this.pdfiumModule.pdfium.getValue(tBottomPtr, 'double');
        const tTop = this.pdfiumModule.pdfium.getValue(tTopPtr, 'double');

        this.pdfiumModule.FPDF_PageToDevice(
          pagePtr,
          0,
          0,
          page.size.width,
          page.size.height,
          0,
          tLeft,
          tTop,
          dx1Ptr,
          dy1Ptr,
        );
        this.pdfiumModule.FPDF_PageToDevice(
          pagePtr,
          0,
          0,
          page.size.width,
          page.size.height,
          0,
          tRight,
          tBottom,
          dx2Ptr,
          dy2Ptr,
        );

        const tx1 = this.pdfiumModule.pdfium.getValue(dx1Ptr, 'i32');
        const ty1 = this.pdfiumModule.pdfium.getValue(dy1Ptr, 'i32');
        const tx2 = this.pdfiumModule.pdfium.getValue(dx2Ptr, 'i32');
        const ty2 = this.pdfiumModule.pdfium.getValue(dy2Ptr, 'i32');

        tightOrigin = { x: Math.min(tx1, tx2), y: Math.min(ty1, ty2) };
        tightSize = {
          width: Math.max(1, Math.abs(tx2 - tx1)),
          height: Math.max(1, Math.abs(ty2 - ty1)),
        };
      }

      // ── 步骤 4：读取字符 Unicode 码点并判断是否为空格字符 ────
      const uc = this.pdfiumModule.FPDFText_GetUnicode(textPagePtr, charIndex);
      isSpace = uc === 32; // U+0020 空格
    }

    // ── 释放所有临时 WASM 内存 ───────────────────────────────────
    allPtrs.forEach((p) => this.memoryManager.free(p));

    return {
      origin: { x, y },
      size: { width, height },
      ...(tightOrigin && { tightOrigin }),
      ...(tightSize && { tightSize }),
      ...(isSpace && { isSpace }),
    };
  }

  /**
   * 获取页面上所有字形的几何信息（不包含 Unicode 文本）。
   *
   * 按 PDFium 返回的逻辑顺序遍历页面上的每个字符，提取其设备空间坐标和包围盒。
   * 前端可根据需要决定是否进一步加载 Unicode 内容。
   *
   * 返回值数组中每个元素的结构：
   *   origin:  { x, y }          // 字形左上角的设备空间坐标（整数像素）
   *   size:    { width, height }  // 字形的宽高（整数像素，最小为 1）
   *   tightOrigin: { x, y }      // 紧凑包围盒原点（可选）
   *   tightSize: { width, height } // 紧凑包围盒尺寸（可选）
   *   isSpace: boolean            // 当 Unicode 为 U+0020 空格时为 true
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @returns 包含所有字形几何信息的数组的异步任务
   *
   * @public
   */
  public getPageGlyphs(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<PdfGlyphObject[]> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageGlyphs', doc, page);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'getPageGlyphs', 'Begin', doc.id);

    // ── 步骤 1：安全检查，确保文档句柄仍然有效 ────────────────
    const ctx = this.cache.getContext(doc.id);

    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'getPageGlyphs', 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // ── 步骤 2：加载页面句柄和文本页面句柄 ────────────────────
    const pageCtx = ctx.acquirePage(page.index);
    const textPagePtr = pageCtx.getTextPage();

    // ── 步骤 3：按逻辑顺序遍历所有字形 ────────────────────────
    const total = this.pdfiumModule.FPDFText_CountChars(textPagePtr);
    const glyphs = new Array(total);

    for (let i = 0; i < total; i++) {
      const g = this.readGlyphInfo(page, pageCtx.pagePtr, textPagePtr, i);

      if (g.isEmpty) {
        continue;
      }

      glyphs[i] = { ...g };
    }

    // ── 步骤 4：释放原生句柄 ──────────────────────────────────
    pageCtx.release();

    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'getPageGlyphs', 'End', doc.id);

    return PdfTaskHelper.resolve(glyphs);
  }

  /**
   * 读取单个字符的紧凑包围盒（CharBox）并转换为设备空间坐标。
   *
   * 使用 FPDFText_GetCharBox 获取精确的字形边界（页面坐标系），
   * 然后通过 FPDF_PageToDevice 将左上角映射到设备空间。
   * 宽高直接从页面空间的差值向上取整计算。
   *
   * @param page - PDF 页面信息对象
   * @param pagePtr - FPDF 页面指针
   * @param textPagePtr - FPDF 文本页面指针
   * @param charIndex - 字符索引
   * @returns 设备空间中的 Rect 矩形对象
   *
   * @private
   */
  private readCharBox(
    page: PdfPageObject,
    pagePtr: number,
    textPagePtr: number,
    charIndex: number,
  ): Rect {
    const topPtr = this.memoryManager.malloc(8);
    const leftPtr = this.memoryManager.malloc(8);
    const bottomPtr = this.memoryManager.malloc(8);
    const rightPtr = this.memoryManager.malloc(8);
    let x = 0;
    let y = 0;
    let width = 0;
    let height = 0;
    if (
      this.pdfiumModule.FPDFText_GetCharBox(
        textPagePtr,
        charIndex,
        leftPtr,
        rightPtr,
        bottomPtr,
        topPtr,
      )
    ) {
      const top = this.pdfiumModule.pdfium.getValue(topPtr, 'double');
      const left = this.pdfiumModule.pdfium.getValue(leftPtr, 'double');
      const bottom = this.pdfiumModule.pdfium.getValue(bottomPtr, 'double');
      const right = this.pdfiumModule.pdfium.getValue(rightPtr, 'double');

      const deviceXPtr = this.memoryManager.malloc(4);
      const deviceYPtr = this.memoryManager.malloc(4);
      this.pdfiumModule.FPDF_PageToDevice(
        pagePtr,
        0,
        0,
        page.size.width,
        page.size.height,
        0,
        left,
        top,
        deviceXPtr,
        deviceYPtr,
      );
      x = this.pdfiumModule.pdfium.getValue(deviceXPtr, 'i32');
      y = this.pdfiumModule.pdfium.getValue(deviceYPtr, 'i32');
      this.memoryManager.free(deviceXPtr);
      this.memoryManager.free(deviceYPtr);

      width = Math.ceil(Math.abs(right - left));
      height = Math.ceil(Math.abs(top - bottom));
    }
    this.memoryManager.free(topPtr);
    this.memoryManager.free(leftPtr);
    this.memoryManager.free(bottomPtr);
    this.memoryManager.free(rightPtr);

    return {
      origin: {
        x,
        y,
      },
      size: {
        width,
        height,
      },
    };
  }

  /**
   * 读取页面上的所有注释（annotations）。
   *
   * 通过页面上下文借用页面句柄，并初始化表单环境（FormHandle），
   * 然后遍历所有注释，逐个解析为 PdfAnnotationObject 对象。
   *
   * @param doc - PDF 文档对象
   * @param ctx - 文档上下文（包含文档指针和缓存）
   * @param page - PDF 页面信息对象
   * @returns 页面上所有注释对象的数组
   *
   * @private
   */
  private readPageAnnotations(doc: PdfDocumentObject, ctx: DocumentContext, page: PdfPageObject) {
    return ctx.borrowPage(page.index, (pageCtx) => {
      return pageCtx.withFormHandle((formHandle) => {
        const annotationCount = this.pdfiumModule.FPDFPage_GetAnnotCount(pageCtx.pagePtr);

        const annotations: PdfAnnotationObject[] = [];
        for (let i = 0; i < annotationCount; i++) {
          pageCtx.withAnnotation(i, (annotPtr) => {
            const anno = this.readPageAnnotation(doc, ctx.docPtr, page, annotPtr, formHandle);
            if (anno) annotations.push(anno);
          });
        }
        return annotations;
      });
    });
  }

  /**
   * 读取页面上的所有表单控件（Widget）注释。
   *
   * 仅提取类型为 WIDGET（表单控件）的注释，用于表单字段的读取和交互。
   * 与 readPageAnnotations 类似，需要初始化表单环境。
   *
   * @param doc - PDF 文档对象
   * @param ctx - 文档上下文（包含文档指针和缓存）
   * @param page - PDF 页面信息对象
   * @returns 页面上所有 Widget 注释对象的数组
   *
   * @private
   */
  private readPageAnnoWidgets(
    doc: PdfDocumentObject,
    ctx: DocumentContext,
    page: PdfPageObject,
  ): PdfWidgetAnnoObject[] {
    return ctx.borrowPage(page.index, (pageCtx) => {
      return pageCtx.withFormHandle((formHandle) => {
        const annotationCount = this.pdfiumModule.FPDFPage_GetAnnotCount(pageCtx.pagePtr);

        const annotations: PdfWidgetAnnoObject[] = [];
        for (let i = 0; i < annotationCount; i++) {
          pageCtx.withAnnotation(i, (annotPtr) => {
            const anno = this.readPageAnnoWidget(doc, page, annotPtr, formHandle);
            if (anno) annotations.push(anno);
          });
        }
        return annotations;
      });
    });
  }

  /**
   * 直接从文档层面读取页面注释（无需加载页面）。
   *
   * 使用 EPDFPage_GetAnnotCountRaw / EPDFPage_GetAnnotRaw 等
   * 扩展 API 直接通过文档指针和页面索引访问注释，
   * 避免了 acquirePage 的开销。适用于只需要注释信息的场景。
   *
   * @param doc - PDF 文档对象
   * @param ctx - 文档上下文
   * @param page - PDF 页面信息对象
   * @param formHandle - 可选的表单填充环境句柄（用于 Widget 注释）
   * @returns 页面上所有注释对象的数组
   *
   * @private
   */
  private readPageAnnotationsRaw(
    doc: PdfDocumentObject,
    ctx: DocumentContext,
    page: PdfPageObject,
    formHandle?: number,
  ): PdfAnnotationObject[] {
    const count = this.pdfiumModule.EPDFPage_GetAnnotCountRaw(ctx.docPtr, page.index);
    if (count <= 0) return [];

    const out: PdfAnnotationObject[] = [];

    for (let i = 0; i < count; ++i) {
      const annotPtr = this.pdfiumModule.EPDFPage_GetAnnotRaw(ctx.docPtr, page.index, i);
      if (!annotPtr) continue;

      try {
        const anno = this.readPageAnnotation(doc, ctx.docPtr, page, annotPtr, formHandle);
        if (anno) out.push(anno);
      } finally {
        this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);
      }
    }
    return out;
  }

  /**
   * 读取单个注释的表单控件（Widget）信息。
   *
   * 首先检查注释是否为 Widget 类型（FPDFAnnot_GetSubtype），
   * 如果不是则返回 undefined。对于 Widget 类型的注释，
   * 委托给 readPdfWidgetAnno 进一步解析表单字段信息。
   * 如果注释缺少 NM（名称）标识，会自动生成 UUID 并写回。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotationPtr - FPDF 注释指针
   * @param formHandle - 表单填充环境句柄
   * @returns Widget 注释对象，如果非 Widget 类型则返回 undefined
   *
   * @private
   */
  private readPageAnnoWidget(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    formHandle: number,
  ): PdfWidgetAnnoObject | undefined {
    let index = this.getAnnotString(annotationPtr, 'NM');
    if (!index) {
      index = uuidV4();
      this.setAnnotString(annotationPtr, 'NM', index);
    }
    const subType = this.pdfiumModule.FPDFAnnot_GetSubtype(
      annotationPtr,
    ) as PdfAnnotationObject['type'];

    if (subType !== PdfAnnotationSubtype.WIDGET) return;

    return this.readPdfWidgetAnno(doc, page, annotationPtr, formHandle, index);
  }

  /**
   * 获取页面上的所有注释（公共 API，直接读取模式）。
   *
   * 与 readPageAnnotations 不同，此方法无需加载页面，
   * 直接通过文档指针和页面索引访问注释数据。
   * 内部会初始化/销毁表单填充环境。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @returns 包含注释对象数组的异步任务
   *
   * @public
   */
  getPageAnnotationsRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
  ): PdfTask<PdfAnnotationObject[]> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getPageAnnotationsRaw', doc, page);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `GetPageAnnotationsRaw`,
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const formInfoPtr = this.pdfiumModule.PDFiumExt_OpenFormFillInfo();
    const formHandle = this.pdfiumModule.PDFiumExt_InitFormFillEnvironment(ctx.docPtr, formInfoPtr);

    try {
      const out = this.readPageAnnotationsRaw(doc, ctx, page, formHandle);

      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `GetPageAnnotationsRaw`,
        'End',
        `${doc.id}-${page.index}`,
      );
      this.logger.debug(
        LOG_SOURCE,
        LOG_CATEGORY,
        'getPageAnnotationsRaw',
        `${doc.id}-${page.index}`,
        out,
      );
      return PdfTaskHelper.resolve(out);
    } finally {
      this.pdfiumModule.PDFiumExt_ExitFormFillEnvironment(formHandle);
      this.pdfiumModule.PDFiumExt_CloseFormFillInfo(formInfoPtr);
    }
  }

  /**
   * 读取单个 PDF 注释，根据注释子类型分发到对应的读取方法。
   *
   * 支持的注释类型包括：
   *   TEXT（文本标注）、FREETEXT（自由文本）、LINK（链接）、
   *   WIDGET（表单控件）、FILEATTACHMENT（文件附件）、INK（墨迹）、
   *   POLYGON（多边形）、POLYLINE（折线）、LINE（直线）、
   *   HIGHLIGHT（高亮）、STAMP（图章）、SQUARE（方形）、
   *   CIRCLE（圆形）、UNDERLINE（下划线）、SQUIGGLY（波浪线）、
   *   STRIKEOUT（删除线）、CARET（插入符号）、REDACT（标记密文）。
   *
   * 读取后还会执行后处理：
   *   - 对带旋转元数据的顶点类注释执行反向旋转
   *   - 读取可用的外观流模式位掩码
   *
   * @param doc - PDF 文档对象
   * @param docPtr - FPDF 文档指针
   * @param page - PDF 页面信息对象
   * @param annotationPtr - FPDF 注释指针
   * @param formHandle - 可选的表单填充环境句柄（Widget 注释需要）
   * @returns 解析后的 PdfAnnotationObject，失败时返回 undefined
   *
   * @private
   */
  private readPageAnnotation(
    doc: PdfDocumentObject,
    docPtr: number,
    page: PdfPageObject,
    annotationPtr: number,
    formHandle?: number,
  ) {
    let index = this.getAnnotString(annotationPtr, 'NM');
    if (!index) {
      index = uuidV4();
      this.setAnnotString(annotationPtr, 'NM', index);
    }
    const subType = this.pdfiumModule.FPDFAnnot_GetSubtype(
      annotationPtr,
    ) as PdfAnnotationObject['type'];
    let annotation: PdfAnnotationObject | undefined;
    switch (subType) {
      case PdfAnnotationSubtype.TEXT:
        {
          annotation = this.readPdfTextAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.FREETEXT:
        {
          annotation = this.readPdfFreeTextAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.LINK:
        {
          annotation = this.readPdfLinkAnno(doc, page, docPtr, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.WIDGET:
        {
          if (formHandle !== undefined) {
            return this.readPdfWidgetAnno(doc, page, annotationPtr, formHandle, index);
          }
        }
        break;
      case PdfAnnotationSubtype.FILEATTACHMENT:
        {
          annotation = this.readPdfFileAttachmentAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.INK:
        {
          annotation = this.readPdfInkAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.POLYGON:
        {
          annotation = this.readPdfPolygonAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.POLYLINE:
        {
          annotation = this.readPdfPolylineAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.LINE:
        {
          annotation = this.readPdfLineAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.HIGHLIGHT:
        annotation = this.readPdfHighlightAnno(doc, page, annotationPtr, index);
        break;
      case PdfAnnotationSubtype.STAMP:
        {
          annotation = this.readPdfStampAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.SQUARE:
        {
          annotation = this.readPdfSquareAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.CIRCLE:
        {
          annotation = this.readPdfCircleAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.UNDERLINE:
        {
          annotation = this.readPdfUnderlineAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.SQUIGGLY:
        {
          annotation = this.readPdfSquigglyAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.STRIKEOUT:
        {
          annotation = this.readPdfStrikeOutAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.CARET:
        {
          annotation = this.readPdfCaretAnno(doc, page, annotationPtr, index);
        }
        break;
      case PdfAnnotationSubtype.REDACT:
        {
          annotation = this.readPdfRedactAnno(doc, page, annotationPtr, index);
        }
        break;
      default:
        {
          annotation = this.readPdfAnno(doc, page, subType, annotationPtr, index);
        }
        break;
    }

    // 后处理：对带旋转元数据的顶点类注释执行反向旋转
    if (annotation) {
      annotation = this.reverseRotateAnnotationOnLoad(annotation);

      // 填充可用的外观流模式位掩码
      const apModes = this.pdfiumModule.EPDFAnnot_GetAvailableAppearanceModes(annotationPtr);
      if (apModes) {
        annotation.appearanceModes = apModes;
      }
    }

    return annotation;
  }

  /**
   * 加载时对带旋转元数据的顶点类注释执行反向旋转。
   *
   * PDF 文件中，带有 /Rotate 条目的注释的顶点坐标已经过物理旋转。
   * 为了在运行时编辑时使用未旋转的坐标，需要将所有顶点按 -rotation
   * 角度绕未旋转矩形的中心点旋转回来。
   *
   * 支持的注释类型：INK（墨迹路径）、LINE（线段端点）、
   * POLYGON（多边形顶点）、POLYLINE（折线顶点）。
   *
   * @param annotation - 原始注释对象
   * @returns 旋转校正后的注释对象（如果无需旋转则原样返回）
   *
   * @private
   */
  private reverseRotateAnnotationOnLoad(annotation: PdfAnnotationObject): PdfAnnotationObject {
    const rotation = annotation.rotation;
    const unrotatedRect = annotation.unrotatedRect;

    // 仅处理带旋转元数据的顶点类注释
    if (!rotation || rotation === 0 || !unrotatedRect) {
      return annotation;
    }

    const center: Position = {
      x: unrotatedRect.origin.x + unrotatedRect.size.width / 2,
      y: unrotatedRect.origin.y + unrotatedRect.size.height / 2,
    };

    // 按 -rotation 反向旋转，恢复未旋转状态的顶点坐标
    switch (annotation.type) {
      case PdfAnnotationSubtype.INK: {
        const ink = annotation as PdfInkAnnoObject;
        const unrotatedInkList = ink.inkList.map((stroke) => ({
          points: stroke.points.map((p) => this.rotatePointForSave(p, center, -rotation)),
        }));
        return { ...ink, inkList: unrotatedInkList };
      }

      case PdfAnnotationSubtype.LINE: {
        const line = annotation as PdfLineAnnoObject;
        return {
          ...line,
          linePoints: {
            start: this.rotatePointForSave(line.linePoints.start, center, -rotation),
            end: this.rotatePointForSave(line.linePoints.end, center, -rotation),
          },
        };
      }

      case PdfAnnotationSubtype.POLYGON: {
        const poly = annotation as PdfPolygonAnnoObject;
        return {
          ...poly,
          vertices: poly.vertices.map((v) => this.rotatePointForSave(v, center, -rotation)),
        };
      }

      case PdfAnnotationSubtype.POLYLINE: {
        const polyline = annotation as PdfPolylineAnnoObject;
        return {
          ...polyline,
          vertices: polyline.vertices.map((v) => this.rotatePointForSave(v, center, -rotation)),
        };
      }

      default:
        return annotation;
    }
  }

  /**
   * 读取注释字典中 `/C` 条目存储的颜色。
   *
   * 大多数由 Acrobat、Microsoft Office、LaTeX 等工具创建的 PDF 都包含此条目。
   * 当条目不存在时（常见于 macOS Preview、Chrome、Drawboard 等工具），
   * 调用会失败并返回 `undefined`。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param colorType - 颜色类型（0=填充/描边色，1=内部色，2=文本色）
   * @returns RGB 颜色对象（各通道 0-255），不存在时返回 `undefined`
   *
   * @private
   */
  private readAnnotationColor(
    annotationPtr: number,
    colorType: PdfAnnotationColorType = PdfAnnotationColorType.Color,
  ): PdfColor | undefined {
    const rPtr = this.memoryManager.malloc(4);
    const gPtr = this.memoryManager.malloc(4);
    const bPtr = this.memoryManager.malloc(4);

    // colorType 0 = "color"（描边/填充色）；其他类型为内部/边框色
    const ok = this.pdfiumModule.EPDFAnnot_GetColor(annotationPtr, colorType, rPtr, gPtr, bPtr);

    let colour: PdfColor | undefined;

    if (ok) {
      colour = {
        red: this.pdfiumModule.pdfium.getValue(rPtr, 'i32') & 0xff,
        green: this.pdfiumModule.pdfium.getValue(gPtr, 'i32') & 0xff,
        blue: this.pdfiumModule.pdfium.getValue(bPtr, 'i32') & 0xff,
      };
    }

    this.memoryManager.free(rPtr);
    this.memoryManager.free(gPtr);
    this.memoryManager.free(bPtr);

    return colour;
  }

  /**
   * 获取注释的填充/描边颜色，返回 Web 颜色格式（十六进制字符串）。
   *
   * 内部调用 readAnnotationColor 获取 RGB 值，然后转换为 WebColor。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param colorType - 颜色类型（0=填充色，1=描边色）
   * @returns WebColor 十六进制颜色字符串，不存在时返回 `undefined`
   *
   * @private
   */
  private getAnnotationColor(
    annotationPtr: number,
    colorType: PdfAnnotationColorType = PdfAnnotationColorType.Color,
  ): WebColor | undefined {
    const annotationColor = this.readAnnotationColor(annotationPtr, colorType);

    return annotationColor ? pdfColorToWebColor(annotationColor) : undefined;
  }

  /**
   * 设置注释的填充/描边颜色。
   *
   * 将 WebColor（十六进制字符串）转换为 PDF 颜色格式后，
   * 通过 EPDFAnnot_SetColor 写入注释的 `/C` 条目。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param webColor - WebColor 十六进制颜色字符串
   * @param colorType - 颜色类型（0=填充色，1=描边色）
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotationColor(
    annotationPtr: number,
    webColor: WebColor,
    colorType: PdfAnnotationColorType = PdfAnnotationColorType.Color,
  ): boolean {
    const pdfColor = webColorToPdfColor(webColor);

    return this.pdfiumModule.EPDFAnnot_SetColor(
      annotationPtr,
      colorType,
      pdfColor.red & 0xff,
      pdfColor.green & 0xff,
      pdfColor.blue & 0xff,
    );
  }

  /**
   * 获取注释的透明度。
   *
   * 通过 EPDFAnnot_GetOpacity 读取注释的 `/CA` 条目（PDF 内部 0-255），
   * 然后转换为 Web 透明度范围（0-1）。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns 透明度值（0-1，1 为完全不透明）
   *
   * @private
   */
  private getAnnotationOpacity(annotationPtr: number): number {
    const opacityPtr = this.memoryManager.malloc(4);
    const ok = this.pdfiumModule.EPDFAnnot_GetOpacity(annotationPtr, opacityPtr);
    const opacity = ok ? this.pdfiumModule.pdfium.getValue(opacityPtr, 'i32') : 255;
    this.memoryManager.free(opacityPtr);
    return pdfAlphaToWebOpacity(opacity);
  }

  /**
   * 设置注释的透明度。
   *
   * 将 Web 透明度（0-1）转换为 PDF 内部的 0-255 格式，
   * 然后通过 EPDFAnnot_SetOpacity 写入注释的 `/CA` 条目。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param opacity - 透明度值（0-1，1 为完全不透明）
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotationOpacity(annotationPtr: number, opacity: number): boolean {
    const pdfOpacity = webOpacityToPdfAlpha(opacity);
    return this.pdfiumModule.EPDFAnnot_SetOpacity(annotationPtr, pdfOpacity & 0xff);
  }

  /**
   * 获取注释的旋转角度（`/Rotate` 条目）。
   *
   * 从注释字典中读取旋转角度（度数），未设置时返回 0。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns 旋转角度（度数），未设置时为 0
   *
   * @private
   */
  private getAnnotationRotation(annotationPtr: number): number {
    const rotationPtr = this.memoryManager.malloc(4);
    const ok = this.pdfiumModule.EPDFAnnot_GetRotate(annotationPtr, rotationPtr);
    if (!ok) {
      this.memoryManager.free(rotationPtr);
      return 0;
    }
    const rotation = this.pdfiumModule.pdfium.getValue(rotationPtr, 'float');
    this.memoryManager.free(rotationPtr);
    return rotation;
  }

  /**
   * 设置注释的旋转角度（`/Rotate` 条目）。
   *
   * 写入旋转角度到注释字典。传入 0 时会移除 `/Rotate` 键。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param rotation - 旋转角度（度数，顺时针方向）
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotationRotation(annotationPtr: number, rotation: number): boolean {
    return !!this.pdfiumModule.EPDFAnnot_SetRotate(annotationPtr, rotation);
  }

  /**
   * 获取 EmbedPDF 扩展旋转角度（`/EPDFRotate` 条目）。
   *
   * EmbedPDF 自定义的旋转角度，用于记录运行时用户施加的旋转，
   * 与 PDF 标准的 `/Rotate` 条目分离。未设置时返回 0。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns 扩展旋转角度（度数），未设置时为 0
   *
   * @private
   */
  private getAnnotExtendedRotation(annotationPtr: number): number {
    const rotationPtr = this.memoryManager.malloc(4);
    const ok = this.pdfiumModule.EPDFAnnot_GetExtendedRotation(annotationPtr, rotationPtr);
    if (!ok) {
      this.memoryManager.free(rotationPtr);
      return 0;
    }
    const rotation = this.pdfiumModule.pdfium.getValue(rotationPtr, 'float');
    this.memoryManager.free(rotationPtr);
    return rotation;
  }

  /**
   * 设置 EmbedPDF 扩展旋转角度（`/EPDFRotate` 条目）。
   *
   * 传入 0 时移除该键。用于在保存时记录运行时用户施加的旋转。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param rotation - 旋转角度（度数）
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotExtendedRotation(annotationPtr: number, rotation: number): boolean {
    return !!this.pdfiumModule.EPDFAnnot_SetExtendedRotation(annotationPtr, rotation);
  }

  /**
   * 读取 EmbedPDF 未旋转矩形（`/EPDFUnrotatedRect` 条目）。
   *
   * 返回页面坐标系中的原始矩形（格式与 readPageAnnoRect 相同），
   * 即注释在旋转之前的矩形位置。用于在编辑时恢复注释的初始位置。
   * 当条目未设置或全部为零时返回 null。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns 页面坐标的 `{ left, top, right, bottom }` 矩形，未设置时为 null
   *
   * @private
   */
  private readAnnotUnrotatedRect(
    annotationPtr: number,
  ): { left: number; top: number; right: number; bottom: number } | null {
    const rectPtr = this.memoryManager.malloc(4 * 4);
    const ok = this.pdfiumModule.EPDFAnnot_GetUnrotatedRect(annotationPtr, rectPtr);
    if (!ok) {
      this.memoryManager.free(rectPtr);
      return null;
    }
    // FS_RECTF 布局：left, top, right, bottom（与 FPDFAnnot_GetRect 相同）
    const left = this.pdfiumModule.pdfium.getValue(rectPtr, 'float');
    const top = this.pdfiumModule.pdfium.getValue(rectPtr + 4, 'float');
    const right = this.pdfiumModule.pdfium.getValue(rectPtr + 8, 'float');
    const bottom = this.pdfiumModule.pdfium.getValue(rectPtr + 12, 'float');
    this.memoryManager.free(rectPtr);

    // 全部为零表示该条目未设置
    if (left === 0 && top === 0 && right === 0 && bottom === 0) {
      return null;
    }

    return { left, top, right, bottom };
  }

  /**
   * 写入 EmbedPDF 未旋转矩形（`/EPDFUnrotatedRect` 条目）。
   *
   * 接受设备空间的 Rect，内部转换为页面坐标后写入。
   * 遵循与 setPageAnnoRect 相同的坐标转换模式：
   * 将四个角点分别映射到页面空间，取轴对齐包围盒（AABB）。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotPtr - FPDF 注释指针
   * @param rect - 设备空间的矩形
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotUnrotatedRect(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotPtr: number,
    rect: Rect,
  ): boolean {
    const x0d = rect.origin.x;
    const y0d = rect.origin.y;
    const x1d = rect.origin.x + rect.size.width;
    const y1d = rect.origin.y + rect.size.height;

    // 将矩形的 4 个角点全部映射到页面空间（处理任意旋转角度）
    const TL = this.convertDevicePointToPagePoint(doc, page, { x: x0d, y: y0d });
    const TR = this.convertDevicePointToPagePoint(doc, page, { x: x1d, y: y0d });
    const BR = this.convertDevicePointToPagePoint(doc, page, { x: x1d, y: y1d });
    const BL = this.convertDevicePointToPagePoint(doc, page, { x: x0d, y: y1d });

    // 计算页面空间中的轴对齐包围盒（AABB）
    let left = Math.min(TL.x, TR.x, BR.x, BL.x);
    let right = Math.max(TL.x, TR.x, BR.x, BL.x);
    let bottom = Math.min(TL.y, TR.y, BR.y, BL.y);
    let top = Math.max(TL.y, TR.y, BR.y, BL.y);
    if (left > right) [left, right] = [right, left];
    if (bottom > top) [bottom, top] = [top, bottom];

    // 按 FS_RECTF 结构体的内存顺序写入：L, T, R, B
    const ptr = this.memoryManager.malloc(16);
    const pdf = this.pdfiumModule.pdfium;
    pdf.setValue(ptr + 0, left, 'float'); // L
    pdf.setValue(ptr + 4, top, 'float'); // T
    pdf.setValue(ptr + 8, right, 'float'); // R
    pdf.setValue(ptr + 12, bottom, 'float'); // B

    // 写入 /EPDFUnrotatedRect 条目并释放临时内存
    const ok = this.pdfiumModule.EPDFAnnot_SetUnrotatedRect(annotPtr, ptr);
    this.memoryManager.free(ptr);
    return !!ok;
  }

  /**
   * 获取 FreeText 注释的文本对齐方式（`/Q` 条目）。
   *
   * @param annotationPtr - FPDF 注释指针（由 FPDFPage_GetAnnot 返回）
   * @returns PdfTextAlignment 枚举值
   *
   * @private
   */
  private getAnnotationTextAlignment(annotationPtr: number): PdfTextAlignment {
    return this.pdfiumModule.EPDFAnnot_GetTextAlignment(annotationPtr);
  }

  /**
   * 设置 FreeText 注释的文本对齐方式（`/Q` 条目），
   * 并清除现有的外观流（/AP）以便重新生成。
   *
   * @param annotationPtr - FPDF 注释指针（由 FPDFPage_GetAnnot 返回）
   * @param alignment - PdfTextAlignment 枚举值
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotationTextAlignment(annotationPtr: number, alignment: PdfTextAlignment): boolean {
    return !!this.pdfiumModule.EPDFAnnot_SetTextAlignment(annotationPtr, alignment);
  }

  /**
   * 获取 FreeText 注释的垂直对齐方式（`/EPDF:VerticalAlignment` 条目）。
   *
   * EmbedPDF 自定义的扩展属性，用于控制自由文本框内的垂直文本对齐。
   *
   * @param annotationPtr - FPDF 注释指针（由 FPDFPage_GetAnnot 返回）
   * @returns PdfVerticalAlignment 枚举值
   *
   * @private
   */
  private getAnnotationVerticalAlignment(annotationPtr: number): PdfVerticalAlignment {
    return this.pdfiumModule.EPDFAnnot_GetVerticalAlignment(annotationPtr);
  }

  /**
   * 设置 FreeText 注释的垂直对齐方式（`/EPDF:VerticalAlignment` 条目），
   * 并清除现有的外观流以便重新生成。
   *
   * @param annotationPtr - FPDF 注释指针（由 FPDFPage_GetAnnot 返回）
   * @param alignment - PdfVerticalAlignment 枚举值
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotationVerticalAlignment(
    annotationPtr: number,
    alignment: PdfVerticalAlignment,
  ): boolean {
    return !!this.pdfiumModule.EPDFAnnot_SetVerticalAlignment(annotationPtr, alignment);
  }

  /**
   * 获取标记密文（Redact）注释的覆盖文本。
   *
   * 覆盖文本是在应用密文标记后显示在空白区域上的文本内容。
   *
   * @param annotationPtr - FPDF 注释指针（由 FPDFPage_GetAnnot 返回）
   * @returns 覆盖文本字符串，未设置时返回 `undefined`
   *
   * @private
   */
  private getOverlayText(annotationPtr: number): string | undefined {
    const len = this.pdfiumModule.EPDFAnnot_GetOverlayText(annotationPtr, 0, 0);
    if (len === 0) return undefined;

    const bytes = (len + 1) * 2;
    const ptr = this.memoryManager.malloc(bytes);

    this.pdfiumModule.EPDFAnnot_GetOverlayText(annotationPtr, ptr, bytes);
    const value = this.pdfiumModule.pdfium.UTF16ToString(ptr);
    this.memoryManager.free(ptr);

    return value || undefined;
  }

  /**
   * 设置标记密文（Redact）注释的覆盖文本。
   *
   * @param annotationPtr - FPDF 注释指针（由 FPDFPage_GetAnnot 返回）
   * @param text - 覆盖文本，传入 undefined/空字符串则清除
   * @returns 操作是否成功
   *
   * @private
   */
  private setOverlayText(annotationPtr: number, text: string | undefined): boolean {
    if (!text) {
      return this.pdfiumModule.EPDFAnnot_SetOverlayText(annotationPtr, 0);
    }
    return this.withWString(text, (wPtr) => {
      return this.pdfiumModule.EPDFAnnot_SetOverlayText(annotationPtr, wPtr);
    });
  }

  /**
   * 获取标记密文（Redact）注释的覆盖文本是否重复显示。
   *
   * @param annotationPtr - FPDF 注释指针（由 FPDFPage_GetAnnot 返回）
   * @returns 是否重复显示覆盖文本
   *
   * @private
   */
  private getOverlayTextRepeat(annotationPtr: number): boolean {
    return this.pdfiumModule.EPDFAnnot_GetOverlayTextRepeat(annotationPtr);
  }

  /**
   * 设置标记密文（Redact）注释的覆盖文本是否重复显示。
   *
   * @param annotationPtr - FPDF 注释指针（由 FPDFPage_GetAnnot 返回）
   * @param repeat - 是否重复显示覆盖文本
   * @returns 操作是否成功
   *
   * @private
   */
  private setOverlayTextRepeat(annotationPtr: number, repeat: boolean): boolean {
    return this.pdfiumModule.EPDFAnnot_SetOverlayTextRepeat(annotationPtr, repeat);
  }

  /**
   * 获取注释的默认外观（Default Appearance，`/DA` 条目）。
   *
   * 从 FreeText 注释的 `/DA` 字符串中解析字体、字号和颜色。
   * 返回的对象包含：
   *   fontFamily - FPDF_STANDARD_FONT 枚举值（0=Courier, 12=ZapfDingbats 等）
   *   fontSize   - 字号（磅值）
   *   fontColor  - 字体颜色（WebColor 十六进制格式）
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns 默认外观信息，PDFium 返回 false 时为 `undefined`
   *
   * @private
   */
  private getAnnotationDefaultAppearance(
    annotationPtr: number,
  ): { fontFamily: PdfStandardFont; fontSize: number; fontColor: WebColor } | undefined {
    const fontPtr = this.memoryManager.malloc(4);
    const sizePtr = this.memoryManager.malloc(4);
    const rPtr = this.memoryManager.malloc(4);
    const gPtr = this.memoryManager.malloc(4);
    const bPtr = this.memoryManager.malloc(4);

    const ok = !!this.pdfiumModule.EPDFAnnot_GetDefaultAppearance(
      annotationPtr,
      fontPtr,
      sizePtr,
      rPtr,
      gPtr,
      bPtr,
    );

    if (!ok) {
      [fontPtr, sizePtr, rPtr, gPtr, bPtr].forEach((p) => this.memoryManager.free(p));
      return; // 返回 undefined，由调用方决定如何处理
    }

    const pdf = this.pdfiumModule.pdfium;
    const font = pdf.getValue(fontPtr, 'i32');
    const fontSize = pdf.getValue(sizePtr, 'float');
    const red = pdf.getValue(rPtr, 'i32') & 0xff;
    const green = pdf.getValue(gPtr, 'i32') & 0xff;
    const blue = pdf.getValue(bPtr, 'i32') & 0xff;

    [fontPtr, sizePtr, rPtr, gPtr, bPtr].forEach((p) => this.memoryManager.free(p));

    return {
      fontFamily: font,
      fontSize,
      fontColor: pdfColorToWebColor({ red, green, blue }),
    };
  }

  /**
   * 设置注释的默认外观（Default Appearance，`/DA` 条目）。
   *
   * 将字体、字号和颜色写入 FreeText 注释的 `/DA` 字符串。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param font - FPDF_STANDARD_FONT 枚举值
   * @param fontSize - 字号（磅值，≥ 0）
   * @param color - WebColor 十六进制颜色（忽略透明度）
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotationDefaultAppearance(
    annotationPtr: number,
    font: PdfStandardFont,
    fontSize: number,
    color: WebColor,
  ): boolean {
    const { red, green, blue } = webColorToPdfColor(color);

    return !!this.pdfiumModule.EPDFAnnot_SetDefaultAppearance(
      annotationPtr,
      font,
      fontSize,
      red & 0xff,
      green & 0xff,
      blue & 0xff,
    );
  }

  /**
   * 获取注释的边框样式和宽度（`/BS` 条目）。
   *
   * 通过 EPDFAnnot_GetBorderStyle 读取边框样式枚举和线宽。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns 包含以下字段的对象：
   *   ok     - 调用是否成功
   *   style  - PdfAnnotationBorderStyle 边框样式枚举（SOLID/DASHED/BEVELED/INSET/UNDERLINE）
   *   width  - 线宽（磅值），默认为 0
   *
   * @private
   */
  private getBorderStyle(annotationPtr: number): {
    ok: boolean;
    style: PdfAnnotationBorderStyle;
    width: number;
  } {
    /* 步骤 1 ── 为返回的线宽值分配临时存储 ─────────────────────── */
    const widthPtr = this.memoryManager.malloc(4);
    let width = 0;
    let style: PdfAnnotationBorderStyle = PdfAnnotationBorderStyle.UNKNOWN;
    let ok = false;

    style = this.pdfiumModule.EPDFAnnot_GetBorderStyle(annotationPtr, widthPtr);
    width = this.pdfiumModule.pdfium.getValue(widthPtr, 'float');
    ok = style !== PdfAnnotationBorderStyle.UNKNOWN;
    this.memoryManager.free(widthPtr);
    return { ok, style, width };
  }

  /**
   * 设置注释的边框样式和宽度（`/BS` 条目）。
   *
   * 注意：CLOUDY（云状边框）不是 PDF 规范中的 /BS /S 值，
   * 它通过单独的 /BE（边框效果）字典传递。
   * 此处将 CLOUDY 规范化为 SOLID，避免 PDFium 拒绝调用。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param style - 边框样式枚举
   * @param width - 线宽（磅值）
   * @returns 操作是否成功
   *
   * @private
   */
  private setBorderStyle(
    annotationPtr: number,
    style: PdfAnnotationBorderStyle,
    width: number,
  ): boolean {
    // PDFium 的 EPDFAnnot_SetBorderStyle（以及 PDF 规范）只接受
    // /BS /S 的以下名称：S(Solid)/D(Dashed)/B(Beveled)/I(Inset)/U(Underline)。
    // CLOUDY 不是 /BS/S 的值，它通过单独的 /BE（边框效果）字典传递，
    // setBorderEffect() 会在后续为 polygon/square/circle 写入。
    // 此处将 CLOUDY 规范化为 SOLID，避免调用方传入 strokeStyle: CLOUDY
    // 导致 PDFium 拒绝调用并中断后续保存操作。
    const effectiveStyle =
      style === PdfAnnotationBorderStyle.CLOUDY ? PdfAnnotationBorderStyle.SOLID : style;
    return this.pdfiumModule.EPDFAnnot_SetBorderStyle(annotationPtr, effectiveStyle, width);
  }

  /**
   * 获取注释的 `/Name` 条目（图标名称，如 Note、Comment 等）。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns PdfAnnotationName 枚举值
   *
   * @private
   */
  private getAnnotationName(annotationPtr: number): PdfAnnotationName {
    return this.pdfiumModule.EPDFAnnot_GetName(annotationPtr);
  }

  /**
   * 设置注释的 `/Name` 条目（图标名称）。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param name - PdfAnnotationName 枚举值
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotationName(annotationPtr: number, name: PdfAnnotationName): boolean {
    return this.pdfiumModule.EPDFAnnot_SetName(annotationPtr, name);
  }

  /**
   * 获取注释的回复类型（`/RT` 条目，ISO 32000-2 标准属性）。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns PdfAnnotationReplyType 枚举值
   *
   * @private
   */
  private getReplyType(annotationPtr: number): PdfAnnotationReplyType {
    return this.pdfiumModule.EPDFAnnot_GetReplyType(annotationPtr);
  }

  /**
   * 设置注释的回复类型（`/RT` 条目，ISO 32000-2 标准属性）。
   *
   * 传入 undefined 或 Unknown 时会清除 `/RT` 键。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param replyType - PdfAnnotationReplyType 枚举值
   * @returns 操作是否成功
   *
   * @private
   */
  private setReplyType(annotationPtr: number, replyType?: PdfAnnotationReplyType): boolean {
    // 如果 undefined 或 Unknown，清除 /RT 键（Unknown = 0 表示移除该键）
    return this.pdfiumModule.EPDFAnnot_SetReplyType(
      annotationPtr,
      replyType ?? PdfAnnotationReplyType.Unknown,
    );
  }

  /**
   * 获取注释的边框效果（云状边框，`/BE` 条目）。
   *
   * 读取 /BE 字典中的强度/半径值。当注释拥有有效的云状边框效果时 ok 为 true。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns 包含以下字段的对象：
   *   ok        - 是否拥有有效的云状边框效果
   *   intensity - 强度/半径值（ok 为 false 时为 0）
   *
   * @private
   */
  private getBorderEffect(annotationPtr: number): { ok: boolean; intensity: number } {
    const intensityPtr = this.memoryManager.malloc(4);

    const ok = !!this.pdfiumModule.EPDFAnnot_GetBorderEffect(annotationPtr, intensityPtr);

    const intensity = ok ? this.pdfiumModule.pdfium.getValue(intensityPtr, 'float') : 0;

    this.memoryManager.free(intensityPtr);
    return { ok, intensity };
  }

  /**
   * 获取注释的矩形差异（`/RD` 条目，用于 Square/Circle 注释）。
   *
   * 读取 /RD 数组中的四个内缩值（left, top, right, bottom）。
   * PDFium 原生接口返回 [left, bottom, right, top] 顺序，
   * 此处重新映射为模型中稳定的 { left, top, right, bottom } 格式。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns 包含以下字段的对象：
   *   ok     - 是否存在 /RD 条目
   *   left/top/right/bottom - 内缩值（ok 为 false 时全部为 0）
   *
   * @private
   */
  private getRectangleDifferences(annotationPtr: number): {
    ok: boolean;
    left: number;
    top: number;
    right: number;
    bottom: number;
  } {
    /* 临时存储 ─────────────────────────────────────────── */
    const lPtr = this.memoryManager.malloc(4);
    const bPtr = this.memoryManager.malloc(4);
    const rPtr = this.memoryManager.malloc(4);
    const tPtr = this.memoryManager.malloc(4);

    const ok = !!this.pdfiumModule.EPDFAnnot_GetRectangleDifferences(
      annotationPtr,
      lPtr,
      bPtr,
      rPtr,
      tPtr,
    );

    const pdf = this.pdfiumModule.pdfium;
    const left = pdf.getValue(lPtr, 'float');
    const bottom = pdf.getValue(bPtr, 'float');
    const right = pdf.getValue(rPtr, 'float');
    const top = pdf.getValue(tPtr, 'float');

    /* 清理 ─────────────────────────────────────────────── */
    this.memoryManager.free(lPtr);
    this.memoryManager.free(bPtr);
    this.memoryManager.free(rPtr);
    this.memoryManager.free(tPtr);

    return { ok, left, top, right, bottom };
  }

  /**
   * 设置注释的矩形差异（`/RD` 数组）。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param rd - 四个内缩值 { left, top, right, bottom }，传入 undefined 则清除
   * @returns 操作是否成功
   *
   * @private
   */
  private setRectangleDifferences(
    annotationPtr: number,
    rd: { left: number; top: number; right: number; bottom: number } | undefined,
  ): boolean {
    if (!rd) {
      return this.pdfiumModule.EPDFAnnot_ClearRectangleDifferences(annotationPtr);
    }
    return this.pdfiumModule.EPDFAnnot_SetRectangleDifferences(
      annotationPtr,
      rd.left,
      rd.bottom,
      rd.right,
      rd.top,
    );
  }

  /**
   * 设置或清除注释的边框效果（`/BE` 字典）。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param intensity - 云状边框强度值，传入 undefined 或 ≤ 0 则清除
   * @returns 操作是否成功
   *
   * @private
   */
  private setBorderEffect(annotationPtr: number, intensity: number | undefined): boolean {
    if (intensity === undefined || intensity <= 0) {
      return this.pdfiumModule.EPDFAnnot_ClearBorderEffect(annotationPtr);
    }
    return this.pdfiumModule.EPDFAnnot_SetBorderEffect(annotationPtr, intensity);
  }

  /**
   * 获取注释的日期。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param key - 'M' 表示修改日期，'CreationDate' 表示创建日期
   * @returns Date 对象，PDFium 无法读取时返回 `undefined`
   *
   * @private
   */
  private getAnnotationDate(annotationPtr: number, key: 'M' | 'CreationDate'): Date | undefined {
    const raw = this.getAnnotString(annotationPtr, key);
    return raw ? pdfDateToDate(raw) : undefined;
  }

  /**
   * 设置注释的日期。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param key - 'M' 表示修改日期，'CreationDate' 表示创建日期
   * @param date - 要设置的 Date 对象
   * @returns 操作是否成功
   *
   * @private
   */
  private setAnnotationDate(annotationPtr: number, key: 'M' | 'CreationDate', date: Date): boolean {
    const raw = dateToPdfDate(date);
    return this.setAnnotString(annotationPtr, key, raw);
  }

  /**
   * 获取附件的日期。
   *
   * @param attachmentPtr - FPDF 附件指针
   * @param key - 'ModDate' 表示修改日期，'CreationDate' 表示创建日期
   * @returns Date 对象，PDFium 无法读取时返回 `undefined`
   *
   * @private
   */
  private getAttachmentDate(
    attachmentPtr: number,
    key: 'ModDate' | 'CreationDate',
  ): Date | undefined {
    const raw = this.getAttachmentString(attachmentPtr, key);
    return raw ? pdfDateToDate(raw) : undefined;
  }

  /**
   * 设置附件的日期。
   *
   * @param attachmentPtr - FPDF 附件指针
   * @param key - 'ModDate' 表示修改日期，'CreationDate' 表示创建日期
   * @param date - 要设置的 Date 对象
   * @returns 操作是否成功
   *
   * @private
   */
  private setAttachmentDate(
    attachmentPtr: number,
    key: 'ModDate' | 'CreationDate',
    date: Date,
  ): boolean {
    const raw = dateToPdfDate(date);
    return this.setAttachmentString(attachmentPtr, key, raw);
  }

  /**
   * 获取注释的虚线模式（`/BS` → `/D` 数组，仅适用于虚线边框）。
   *
   * 使用 EPDFAnnot_GetBorderDashPatternCount 和 EPDFAnnot_GetBorderDashPattern
   * 两个 API 读取虚线/间距长度数组。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns 包含以下字段的对象：
   *   ok      - 是否为虚线边框且成功获取数组
   *   pattern - 虚线/间距长度数值数组（ok 为 false 时为空数组）
   *
   * @private
   */
  private getBorderDashPattern(annotationPtr: number): { ok: boolean; pattern: number[] } {
    const count = this.pdfiumModule.EPDFAnnot_GetBorderDashPatternCount(annotationPtr);
    if (count === 0) {
      return { ok: false, pattern: [] };
    }

    /* 在 WASM 堆上分配 count 个 float 的空间 */
    const arrPtr = this.memoryManager.malloc(4 * count);
    const okNative = !!this.pdfiumModule.EPDFAnnot_GetBorderDashPattern(
      annotationPtr,
      arrPtr,
      count,
    );

    /* 从 WASM 内存拷贝到 JS 数组 */
    const pattern: number[] = [];
    if (okNative) {
      const pdf = this.pdfiumModule.pdfium;
      for (let i = 0; i < count; i++) {
        pattern.push(pdf.getValue(arrPtr + 4 * i, 'float'));
      }
    }

    this.memoryManager.free(arrPtr);
    return { ok: okNative, pattern };
  }

  /**
   * 设置注释边框的虚线模式（`/BS` → `/D` 数组）。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param pattern - 虚线/间距长度数组（磅值，如 [3, 2]），空数组则清除（实线）
   * @returns 操作是否成功
   *
   * @private
   */
  private setBorderDashPattern(annotationPtr: number, pattern: number[]): boolean {
    // 空数组 → 清除虚线模式（PDF 规范：无 /D 表示实线）
    if (!pattern || pattern.length === 0) {
      return this.pdfiumModule.EPDFAnnot_SetBorderDashPattern(annotationPtr, 0, 0);
    }

    // 校验并清理数值（必须为正浮点数，PDF 规范通常允许 1-8 个数值）
    const clean = pattern.map((n) => (Number.isFinite(n) && n > 0 ? n : 0)).filter((n) => n > 0);
    if (clean.length === 0) {
      // 无有效数值 → 视为清除
      return this.pdfiumModule.EPDFAnnot_SetBorderDashPattern(annotationPtr, 0, 0);
    }

    const bytes = 4 * clean.length;
    const bufPtr = this.memoryManager.malloc(bytes);
    for (let i = 0; i < clean.length; i++) {
      this.pdfiumModule.pdfium.setValue(bufPtr + 4 * i, clean[i], 'float');
    }

    const ok = !!this.pdfiumModule.EPDFAnnot_SetBorderDashPattern(
      annotationPtr,
      bufPtr,
      clean.length,
    );

    this.memoryManager.free(bufPtr);
    return ok;
  }

  /**
   * 获取注释的线端样式（`/LE` 数组，起始和结束端点的箭头/样式）。
   *
   * 适用于 LINE（直线）和 POLYLINE（折线）注释。
   *
   * @param annotationPtr - FPDF 注释指针
   * @returns `{ start, end }` 线端样式对，PDFium 无法读取时返回 `undefined`
   *
   * @private
   */
  private getLineEndings(annotationPtr: number): LineEndings | undefined {
    const startPtr = this.memoryManager.malloc(4);
    const endPtr = this.memoryManager.malloc(4);

    const ok = !!this.pdfiumModule.EPDFAnnot_GetLineEndings(annotationPtr, startPtr, endPtr);
    if (!ok) {
      this.memoryManager.free(startPtr);
      this.memoryManager.free(endPtr);
      return undefined;
    }

    const start = this.pdfiumModule.pdfium.getValue(startPtr, 'i32');
    const end = this.pdfiumModule.pdfium.getValue(endPtr, 'i32');

    this.memoryManager.free(startPtr);
    this.memoryManager.free(endPtr);

    return { start, end };
  }

  /**
   * 设置注释的线端样式（`/LE` 数组）。
   *
   * @param annotationPtr - FPDF 注释指针
   * @param start - 起始端点样式
   * @param end - 结束端点样式
   * @returns 操作是否成功
   *
   * @private
   */
  private setLineEndings(
    annotationPtr: number,
    start: PdfAnnotationLineEnding,
    end: PdfAnnotationLineEnding,
  ): boolean {
    return !!this.pdfiumModule.EPDFAnnot_SetLineEndings(annotationPtr, start, end);
  }

  /**
   * 获取 LINE/POLYLINE 注释的起始和结束端点坐标。
   *
   * 从注释的 `/L` 数组中读取页面坐标系的两个 FS_POINTF 端点，
   * 然后转换为设备空间坐标。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotationPtr - FPDF 注释指针
   * @returns `{ start, end }` 端点对，PDFium 无法读取时返回 `undefined`
   *
   * @private
   */
  private getLinePoints(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
  ): LinePoints | undefined {
    const startPtr = this.memoryManager.malloc(8); // FS_POINTF: x(float) + y(float) = 8 字节
    const endPtr = this.memoryManager.malloc(8);

    const ok = this.pdfiumModule.FPDFAnnot_GetLine(annotationPtr, startPtr, endPtr);
    if (!ok) {
      this.memoryManager.free(startPtr);
      this.memoryManager.free(endPtr);
      return undefined;
    }

    const pdf = this.pdfiumModule.pdfium;

    const sx = pdf.getValue(startPtr + 0, 'float');
    const sy = pdf.getValue(startPtr + 4, 'float');
    const ex = pdf.getValue(endPtr + 0, 'float');
    const ey = pdf.getValue(endPtr + 4, 'float');

    this.memoryManager.free(startPtr);
    this.memoryManager.free(endPtr);

    // 页面坐标 → 设备坐标（统一处理旋转和缩放）
    const start = this.convertPagePointToDevicePoint(doc, page, { x: sx, y: sy });
    const end = this.convertPagePointToDevicePoint(doc, page, { x: ex, y: ey });

    return { start, end };
  }

  /**
   * 设置 LINE 注释的两个端点（写入新的 `/L` 数组 `[x1 y1 x2 y2]`）。
   *
   * 将设备空间坐标转换为页面空间后，打包为两个 FS_POINTF 写入注释。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotPtr - FPDF 注释指针
   * @param start - 起始端点（设备空间）
   * @param end - 结束端点（设备空间）
   * @returns 操作是否成功
   *
   * @private
   */
  private setLinePoints(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotPtr: number,
    start: Position,
    end: Position,
  ): boolean {
    const p1 = this.convertDevicePointToPagePoint(doc, page, start);
    const p2 = this.convertDevicePointToPagePoint(doc, page, end);

    if (!p1 || !p2) return false;

    // 打包为两个 FS_POINTF（x, y 各 4 字节 float）
    const buf = this.memoryManager.malloc(16);
    const pdf = this.pdfiumModule.pdfium;
    pdf.setValue(buf + 0, p1.x, 'float');
    pdf.setValue(buf + 4, p1.y, 'float');
    pdf.setValue(buf + 8, p2.x, 'float');
    pdf.setValue(buf + 12, p2.y, 'float');

    const ok = this.pdfiumModule.EPDFAnnot_SetLine(annotPtr, buf, buf + 8);
    this.memoryManager.free(buf);
    return !!ok;
  }

  /**
   * 读取注释的 `/QuadPoints` 四边形数组并转换为设备空间坐标。
   *
   * 四个点按自然阅读顺序返回：
   *   `p1 → p2`（上边缘）和 `p4 → p3`（下边缘）。
   * 这保留了旋转/倾斜文本的真实形状，调用方如只需轴对齐包围盒可自行折叠。
   *
   * 适用于高亮、下划线、删除线、波浪线等标记注释。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotationPtr - FPDF 注释指针
   * @returns Rect 对象数组（无四边形时返回空数组）
   *
   * @private
   */
  private getQuadPointsAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
  ): Rect[] {
    const quadCount = this.pdfiumModule.FPDFAnnot_CountAttachmentPoints(annotationPtr);
    if (quadCount === 0) return [];

    const FS_QUADPOINTSF_SIZE = 8 * 4; // 8 个 float（4 个点，每点 x,y）= 32 字节
    const quads: Quad[] = [];

    // 逐个读取每个四边形的 4 个顶点
    for (let qi = 0; qi < quadCount; qi++) {
      const quadPtr = this.memoryManager.malloc(FS_QUADPOINTSF_SIZE);

      const ok = this.pdfiumModule.FPDFAnnot_GetAttachmentPoints(annotationPtr, qi, quadPtr);

      if (ok) {
        // 读取 4 个 FS_POINTF 的 x, y 坐标（页面空间）
        const xs: number[] = [];
        const ys: number[] = [];
        for (let i = 0; i < 4; i++) {
          const base = quadPtr + i * 8; // 每个点占 8 字节（x + y 各 4 字节）
          xs.push(this.pdfiumModule.pdfium.getValue(base, 'float'));
          ys.push(this.pdfiumModule.pdfium.getValue(base + 4, 'float'));
        }

        // 将 4 个顶点从页面空间转换为设备空间
        const p1 = this.convertPagePointToDevicePoint(doc, page, { x: xs[0], y: ys[0] });
        const p2 = this.convertPagePointToDevicePoint(doc, page, { x: xs[1], y: ys[1] });
        const p3 = this.convertPagePointToDevicePoint(doc, page, { x: xs[2], y: ys[2] });
        const p4 = this.convertPagePointToDevicePoint(doc, page, { x: xs[3], y: ys[3] });

        quads.push({ p1, p2, p3, p4 });
      }

      this.memoryManager.free(quadPtr);
    }

    return quads.map(quadToRect);
  }

  /**
   * 同步设置注释的四边形（`/QuadPoints` 数组）。
   *
   * 适用于高亮（Highlight）、下划线（Underline）、删除线（StrikeOut）、
   * 波浪线（Squiggly）等标记注释。
   *
   * 操作分两步：
   *   1. 覆盖已存在的四边形
   *   2. 追加新增的四边形（如果 rects 数量多于现有四边形数量）
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotPtr - FPDF 注释指针
   * @param rects - Rect 对象数组（每个 Rect 对应一个四边形）
   * @returns 操作是否成功
   *
   * @private
   */
  private syncQuadPointsAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotPtr: number,
    rects: Rect[],
  ): boolean {
    const FS_QUADPOINTSF_SIZE = 8 * 4; // 8 个 float（4 个点 x,y）= 32 字节
    const pdf = this.pdfiumModule.pdfium;
    const count = this.pdfiumModule.FPDFAnnot_CountAttachmentPoints(annotPtr);
    const buf = this.memoryManager.malloc(FS_QUADPOINTSF_SIZE);

    /** 将一个 Rect 转换为页面空间的四边形并写入 `buf` */
    const writeQuad = (r: Rect) => {
      const q = rectToQuad(r); // TL(左上), TR(右上), BR(右下), BL(左下)
      const p1 = this.convertDevicePointToPagePoint(doc, page, q.p1);
      const p2 = this.convertDevicePointToPagePoint(doc, page, q.p2);
      const p3 = this.convertDevicePointToPagePoint(doc, page, q.p3); // BR
      const p4 = this.convertDevicePointToPagePoint(doc, page, q.p4); // BL

      // PDF QuadPoints 顺序：BL(左下), BR(右下), TL(左上), TR(右上)
      pdf.setValue(buf + 0, p1.x, 'float'); // BL（左下）x
      pdf.setValue(buf + 4, p1.y, 'float'); // BL（左下）y

      pdf.setValue(buf + 8, p2.x, 'float'); // BR（右下）x
      pdf.setValue(buf + 12, p2.y, 'float'); // BR（右下）y

      pdf.setValue(buf + 16, p4.x, 'float'); // TL（左上）x
      pdf.setValue(buf + 20, p4.y, 'float'); // TL（左上）y

      pdf.setValue(buf + 24, p3.x, 'float'); // TR（右上）x
      pdf.setValue(buf + 28, p3.y, 'float'); // TR（右上）y
    };

    /* ─────────────────────────────────────────────────────────────────────── */
    /* 步骤 1：覆盖已存在的四边形                                               */
    const min = Math.min(count, rects.length);
    for (let i = 0; i < min; i++) {
      writeQuad(rects[i]);
      if (!this.pdfiumModule.FPDFAnnot_SetAttachmentPoints(annotPtr, i, buf)) {
        this.memoryManager.free(buf);
        return false;
      }
    }

    /* 步骤 2：追加新增的四边形（rects 数量 > 已有四边形数量）                    */
    for (let i = count; i < rects.length; i++) {
      writeQuad(rects[i]);
      if (!this.pdfiumModule.FPDFAnnot_AppendAttachmentPoints(annotPtr, buf)) {
        this.memoryManager.free(buf);
        return false;
      }
    }

    this.memoryManager.free(buf);
    return true;
  }

  /**
   * 对与指定矩形区域（设备空间）相交的文本执行密文标记（涂黑）操作。
   *
   * 使用 EPDFText_RedactInQuads 将矩形区域内的文本内容永久删除，
   * 并重新生成页面内容流。可选择是否递归处理表单字段内容。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param rects - 设备空间矩形数组（与这些矩形相交的文本将被涂黑）
   * @param options - 可选参数：
   *   recurseForms - 是否递归处理表单字段（默认 true）
   *   drawBlackBoxes - 是否绘制黑色矩形（默认 false）
   * @returns 页面内容是否发生了变化的异步任务
   *
   * @public
   */
  public redactTextInRects(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rects: Rect[],
    options?: PdfRedactTextOptions,
  ): Task<boolean, PdfErrorReason> {
    const { recurseForms = true, drawBlackBoxes = false } = options ?? {};

    this.logger.debug(
      'PDFiumEngine',
      'Engine',
      'redactTextInQuads',
      doc.id,
      page.index,
      rects.length,
    );
    const label = 'RedactTextInQuads';
    this.logger.perf('PDFiumEngine', 'Engine', label, 'Begin', `${doc.id}-${page.index}`);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf('PDFiumEngine', 'Engine', label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<boolean>({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // 清理输入：过滤无效的矩形（非有限数值或零尺寸）
    const clean = (rects ?? []).filter(
      (r) =>
        r &&
        Number.isFinite(r.origin?.x) &&
        Number.isFinite(r.origin?.y) &&
        Number.isFinite(r.size?.width) &&
        Number.isFinite(r.size?.height) &&
        r.size.width > 0 &&
        r.size.height > 0,
    );

    if (clean.length === 0) {
      this.logger.perf('PDFiumEngine', 'Engine', label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.resolve<boolean>(false);
    }

    const pageCtx = ctx.acquirePage(page.index);

    // 将设备空间矩形打包为 WASM 缓冲区 → 调用原生涂黑 API
    const { ptr, count } = this.allocFSQuadsBufferFromRects(doc, page, clean);
    let ok = false;
    try {
      // 调用扩展 API：涂黑指定四边形区域内的文本内容
      ok = !!this.pdfiumModule.EPDFText_RedactInQuads(
        pageCtx.pagePtr,
        ptr,
        count,
        recurseForms ? true : false,
        false,
      );
    } finally {
      this.memoryManager.free(ptr);
    }

    if (ok) {
      ok = !!this.pdfiumModule.FPDFPage_GenerateContent(pageCtx.pagePtr);
    }

    pageCtx.disposeImmediate();
    this.logger.perf('PDFiumEngine', 'Engine', label, 'End', `${doc.id}-${page.index}`);

    return PdfTaskHelper.resolve<boolean>(!!ok);
  }

  /**
   * 应用单个标记密文（Redact）注释，永久删除其下方的内容。
   *
   * 操作流程：
   *   1. 通过注释 ID 查找注释指针
   *   2. 调用 EPDFAnnot_ApplyRedaction 执行涂黑（删除内容 + 展平 RO 外观流 + 移除注释）
   *   3. 重新生成页面内容流
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotation - 要应用的标记密文注释对象
   * @returns 操作是否成功的异步任务
   *
   * @public
   */
  public applyRedaction(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<boolean> {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'applyRedaction',
      doc.id,
      page.index,
      annotation.id,
    );
    const label = 'ApplyRedaction';
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'Begin', `${doc.id}-${page.index}`);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<boolean>({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    const annotPtr = this.getAnnotationByName(pageCtx.pagePtr, annotation.id);
    if (!annotPtr) {
      pageCtx.release();
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<boolean>({
        code: PdfErrorCode.NotFound,
        message: 'annotation not found',
      });
    }

    // 执行密文标记（删除内容、展平 RO 外观流、移除注释）
    const ok = this.pdfiumModule.EPDFAnnot_ApplyRedaction(pageCtx.pagePtr, annotPtr);
    this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);

    if (ok) {
      // 重新生成页面内容流
      this.pdfiumModule.FPDFPage_GenerateContent(pageCtx.pagePtr);
    }

    pageCtx.disposeImmediate();
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);

    return PdfTaskHelper.resolve<boolean>(!!ok);
  }

  /**
   * 应用页面上所有标记密文（Redact）注释。
   *
   * 一次性处理页面上的所有 Redact 注释，永久删除每个注释下方的内容，
   * 展平 RO（Redact Overlay）外观流，并移除所有 Redact 注释。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @returns 是否有密文标记被应用的异步任务
   *
   * @public
   */
  public applyAllRedactions(doc: PdfDocumentObject, page: PdfPageObject): PdfTask<boolean> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'applyAllRedactions', doc.id, page.index);
    const label = 'ApplyAllRedactions';
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'Begin', `${doc.id}-${page.index}`);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<boolean>({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);

    // 一次性应用所有密文标记（内容删除 + RO 展平 + 注释移除）
    const ok = this.pdfiumModule.EPDFPage_ApplyRedactions(pageCtx.pagePtr);

    if (ok) {
      // 重新生成页面内容流
      this.pdfiumModule.FPDFPage_GenerateContent(pageCtx.pagePtr);
    }

    pageCtx.disposeImmediate();
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);

    return PdfTaskHelper.resolve<boolean>(!!ok);
  }

  /**
   * 将注释的外观流（AP/N）展平为页面内容。
   *
   * 注释的视觉效果会变为页面本身的一部分（不可再编辑），
   * 原注释在展平后会被自动移除。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotation - 要展平的注释对象
   * @returns 操作是否成功的异步任务
   *
   * @public
   */
  public flattenAnnotation(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<boolean> {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'flattenAnnotation',
      doc.id,
      page.index,
      annotation.id,
    );
    const label = 'FlattenAnnotation';
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'Begin', `${doc.id}-${page.index}`);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<boolean>({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    const annotPtr = this.getAnnotationByName(pageCtx.pagePtr, annotation.id);
    if (!annotPtr) {
      pageCtx.release();
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<boolean>({
        code: PdfErrorCode.NotFound,
        message: 'annotation not found',
      });
    }

    // 将注释的外观流展平到页面内容并移除注释
    const ok = this.pdfiumModule.EPDFAnnot_Flatten(pageCtx.pagePtr, annotPtr);
    this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);

    if (ok) {
      // 重新生成页面内容流
      this.pdfiumModule.FPDFPage_GenerateContent(pageCtx.pagePtr);
    }

    pageCtx.disposeImmediate();
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);

    return PdfTaskHelper.resolve<boolean>(!!ok);
  }

  /**
   * 将单个注释的外观导出为独立的单页 PDF 文档。
   *
   * 通过 EPDFAnnot_ExportAppearanceAsDocument 创建临时文档，
   * 然后使用 saveDocument 序列化为 ArrayBuffer。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotation - 要导出的注释对象
   * @returns 包含注释外观的 PDF 缓冲区的异步任务
   *
   * @public
   */
  public exportAnnotationAppearanceAsPdf(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
  ): PdfTask<ArrayBuffer> {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'exportAnnotationAppearanceAsPdf',
      doc.id,
      page.index,
      annotation.id,
    );
    const label = 'ExportAnnotationAppearanceAsPdf';
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'Begin', `${doc.id}-${page.index}`);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<ArrayBuffer>({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    const annotPtr = this.getAnnotationByName(pageCtx.pagePtr, annotation.id);
    if (!annotPtr) {
      pageCtx.release();
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<ArrayBuffer>({
        code: PdfErrorCode.NotFound,
        message: 'annotation not found',
      });
    }

    const exportedDocPtr = this.pdfiumModule.EPDFAnnot_ExportAppearanceAsDocument(annotPtr);
    this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);

    if (!exportedDocPtr) {
      pageCtx.release();
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<ArrayBuffer>({
        code: PdfErrorCode.CantCreateNewDoc,
        message: 'can not export annotation as pdf',
      });
    }

    try {
      return PdfTaskHelper.resolve(this.saveDocument(exportedDocPtr));
    } finally {
      this.pdfiumModule.FPDF_CloseDocument(exportedDocPtr);
      pageCtx.release();
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
    }
  }

  /**
   * 将多个注释的外观导出为独立的单页 PDF 文档。
   *
   * 所有注释必须在同一页面上。生成的页面大小为所有注释矩形的并集，
   * 每个注释外观正确定位。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotations - 要导出的注释对象数组
   * @returns 包含合并外观的 PDF 缓冲区的异步任务
   *
   * @public
   */
  public exportAnnotationsAppearanceAsPdf(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotations: PdfAnnotationObject[],
  ): PdfTask<ArrayBuffer> {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'exportAnnotationsAppearanceAsPdf',
      doc.id,
      page.index,
      annotations.map((a) => a.id),
    );
    const label = 'ExportAnnotationsAppearanceAsPdf';
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'Begin', `${doc.id}-${page.index}`);

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<ArrayBuffer>({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    if (annotations.length === 0) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<ArrayBuffer>({
        code: PdfErrorCode.NotFound,
        message: 'no annotations provided',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);

    // 收集所有注释的原生指针（任一注释查找失败则全部释放并返回错误）
    const annotPtrs: number[] = [];
    for (const annotation of annotations) {
      const annotPtr = this.getAnnotationByName(pageCtx.pagePtr, annotation.id);
      if (!annotPtr) {
        for (const ptr of annotPtrs) {
          this.pdfiumModule.FPDFPage_CloseAnnot(ptr);
        }
        pageCtx.release();
        this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
        return PdfTaskHelper.reject<ArrayBuffer>({
          code: PdfErrorCode.NotFound,
          message: `annotation not found: ${annotation.id}`,
        });
      }
      annotPtrs.push(annotPtr);
    }

    // 将注释指针数组打包到 WASM 内存中（int32 数组）
    const ptrArraySize = annotPtrs.length * 4;
    const ptrArrayPtr = this.memoryManager.malloc(ptrArraySize);
    for (let i = 0; i < annotPtrs.length; i++) {
      this.pdfiumModule.pdfium.setValue(ptrArrayPtr + i * 4, annotPtrs[i], 'i32');
    }

    const exportedDocPtr = this.pdfiumModule.EPDFAnnot_ExportMultipleAppearancesAsDocument(
      ptrArrayPtr,
      annotPtrs.length,
    );

    this.memoryManager.free(ptrArrayPtr);
    for (const ptr of annotPtrs) {
      this.pdfiumModule.FPDFPage_CloseAnnot(ptr);
    }

    if (!exportedDocPtr) {
      pageCtx.release();
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
      return PdfTaskHelper.reject<ArrayBuffer>({
        code: PdfErrorCode.CantCreateNewDoc,
        message: 'can not export annotations as pdf',
      });
    }

    try {
      return PdfTaskHelper.resolve(this.saveDocument(exportedDocPtr));
    } finally {
      this.pdfiumModule.FPDF_CloseDocument(exportedDocPtr);
      pageCtx.release();
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, label, 'End', `${doc.id}-${page.index}`);
    }
  }

  /**
   * 将设备空间的 Rect 数组打包为 WASM 堆上的 FS_QUADPOINTSF 缓冲区（页面空间）。
   *
   * 每个 Rect 被转换为 4 个页面空间的 FS_POINTF 点，按 PDF QuadPoints 顺序排列。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param rects - 设备空间的 Rect 数组
   * @returns `{ ptr, count }` WASM 缓冲区指针和四边形数量（调用方负责释放 ptr）
   *
   * @private
   */
  private allocFSQuadsBufferFromRects(doc: PdfDocumentObject, page: PdfPageObject, rects: Rect[]) {
    const STRIDE = 32; // 8 floats × 4 bytes
    const count = rects.length;
    const ptr = this.memoryManager.malloc(STRIDE * count);
    const pdf = this.pdfiumModule.pdfium;

    for (let i = 0; i < count; i++) {
      const r = rects[i];
      const q = rectToQuad(r); // TL, TR, BR, BL (device-space)

      // 转换到页面用户空间（PDFium 原生接口需要页面坐标）
      const p1 = this.convertDevicePointToPagePoint(doc, page, q.p1); // TL
      const p2 = this.convertDevicePointToPagePoint(doc, page, q.p2); // TR
      const p3 = this.convertDevicePointToPagePoint(doc, page, q.p3); // BR
      const p4 = this.convertDevicePointToPagePoint(doc, page, q.p4); // BL

      const base = ptr + i * STRIDE;

      // Keep the exact mapping you used in syncQuadPointsAnno:
      // PDF QuadPoints order comment says BL,BR,TL,TR – and you wrote:
      pdf.setValue(base + 0, p1.x, 'float');
      pdf.setValue(base + 4, p1.y, 'float');
      pdf.setValue(base + 8, p2.x, 'float');
      pdf.setValue(base + 12, p2.y, 'float');
      pdf.setValue(base + 16, p4.x, 'float');
      pdf.setValue(base + 20, p4.y, 'float');
      pdf.setValue(base + 24, p3.x, 'float');
      pdf.setValue(base + 28, p3.y, 'float');
    }

    return { ptr, count };
  }

  /**
   * 读取注释的墨迹路径列表（InkList，`/InkList` 条目）。
   *
   * 每条路径由一组 FS_POINTF 点组成（页面坐标系），逐点转换为设备空间。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotationPtr - FPDF 注释指针
   * @returns 墨迹路径数组，每条路径包含一组设备空间坐标点
   *
   * @private
   */
  private getInkList(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
  ): PdfInkListObject[] {
    const inkList: PdfInkListObject[] = [];
    const pathCount = this.pdfiumModule.FPDFAnnot_GetInkListCount(annotationPtr);
    if (pathCount <= 0) return inkList;

    const pdf = this.pdfiumModule.pdfium;
    const POINT_STRIDE = 8; // FS_POINTF: 2 个 float（x,y）= 8 字节

    // 逐条路径读取
    for (let i = 0; i < pathCount; i++) {
      const points: Position[] = [];

      // 先查询该路径的点数
      const n = this.pdfiumModule.FPDFAnnot_GetInkListPath(annotationPtr, i, 0, 0);
      if (n > 0) {
        const buf = this.memoryManager.malloc(n * POINT_STRIDE);

        // 加载 FS_POINTF 数组（页面坐标系）
        this.pdfiumModule.FPDFAnnot_GetInkListPath(annotationPtr, i, buf, n);

        // 逐点转换为设备空间坐标
        for (let j = 0; j < n; j++) {
          const base = buf + j * POINT_STRIDE;
          const px = pdf.getValue(base + 0, 'float');
          const py = pdf.getValue(base + 4, 'float');
          const d = this.convertPagePointToDevicePoint(doc, page, { x: px, y: py });
          points.push({ x: d.x, y: d.y });
        }

        this.memoryManager.free(buf);
      }

      inkList.push({ points });
    }

    return inkList;
  }

  /**
   * 向注释写入墨迹路径列表（InkList）。
   *
   * 将每条路径的设备空间坐标逐点转换为页面空间后，
   * 通过 FPDFAnnot_AddInkStroke 写入注释。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotationPtr - FPDF 注释指针
   * @param inkList - 墨迹路径数组，每条路径包含一组设备空间坐标点
   * @returns 操作是否成功
   *
   * @private
   */
  private setInkList(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    inkList: PdfInkListObject[],
  ): boolean {
    const pdf = this.pdfiumModule.pdfium;
    const POINT_STRIDE = 8; // FS_POINTF: float x + float y = 8 字节

    // 逐条路径写入
    for (const stroke of inkList) {
      const n = stroke.points.length;
      if (n === 0) continue;

      const buf = this.memoryManager.malloc(n * POINT_STRIDE);

      // 将每个顶点从设备空间转换为页面空间后写入 WASM 缓冲区
      for (let i = 0; i < n; i++) {
        const pDev = stroke.points[i];
        const pPage = this.convertDevicePointToPagePoint(doc, page, pDev);

        pdf.setValue(buf + i * POINT_STRIDE + 0, pPage.x, 'float');
        pdf.setValue(buf + i * POINT_STRIDE + 4, pPage.y, 'float');
      }

      const idx = this.pdfiumModule.FPDFAnnot_AddInkStroke(annotationPtr, buf, n);
      this.memoryManager.free(buf);

      if (idx === -1) {
        return false;
      }
    }

    return true;
  }

  /**
   * 读取 PDF 文本标注（Text）注释。
   *
   * 提取注释的位置、颜色（默认黄色 #FFFF00）、透明度、
   * 状态/状态模型（State/StateModel）以及图标名称。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotationPtr - FPDF 注释指针
   * @param index - 注释的唯一标识符（NM 条目或自动生成的 UUID）
   * @returns PdfTextAnnoObject 文本标注注释对象
   *
   * @private
   */
  private readPdfTextAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfTextAnnoObject | undefined {
    const annoRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, annoRect);

    // 文本标注特有属性：状态和状态模型（用于审阅流程）
    const state = this.getAnnotString(annotationPtr, 'State') as PdfAnnotationState;
    const stateModel = this.getAnnotString(annotationPtr, 'StateModel') as PdfAnnotationStateModel;
    const color = this.getAnnotationColor(annotationPtr);
    const opacity = this.getAnnotationOpacity(annotationPtr);
    const name = this.getAnnotationName(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.TEXT,
      rect,
      strokeColor: color ?? '#FFFF00',
      color: color ?? '#FFFF00',
      opacity,
      state,
      stateModel,
      name,
      icon: name,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 自由文本（FreeText）注释。
   *
   * 提取注释的位置、字体/字号/颜色（默认外观 /DA）、
   * 文本对齐方式、垂直对齐、背景色、边框样式、
   * 富文本内容、矩形差异（/RD）、意图（Intent）、
   * 标注线（CalloutLine）以及线端样式。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面信息对象
   * @param annotationPtr - FPDF 注释指针
   * @param index - 注释的唯一标识符
   * @returns PdfFreeTextAnnoObject 自由文本注释对象
   *
   * @private
   */
  private readPdfFreeTextAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfFreeTextAnnoObject | undefined {
    const annoRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, annoRect);

    // 自由文本特有属性
    const defaultStyle = this.getAnnotString(annotationPtr, 'DS');
    const da = this.getAnnotationDefaultAppearance(annotationPtr);
    const bgColor = this.getAnnotationColor(annotationPtr);
    const textColor = this.getAnnotationColor(annotationPtr, PdfAnnotationColorType.TextColor);
    const borderStyle = this.getBorderStyle(annotationPtr);
    const textAlign = this.getAnnotationTextAlignment(annotationPtr);
    const verticalAlign = this.getAnnotationVerticalAlignment(annotationPtr);
    const opacity = this.getAnnotationOpacity(annotationPtr);
    const richContent = this.getAnnotRichContent(annotationPtr);
    const rd = this.getRectangleDifferences(annotationPtr);
    const intent = this.getAnnotIntent(annotationPtr);
    const calloutLine = this.getCalloutLine(doc, page, annotationPtr);
    const lineEndings = this.getLineEndings(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.FREETEXT,
      rect,
      fontFamily: da?.fontFamily ?? PdfStandardFont.Unknown,
      fontSize: da?.fontSize ?? 12,
      fontColor: textColor ?? da?.fontColor ?? '#000000',
      verticalAlign,
      color: bgColor,
      backgroundColor: bgColor,
      opacity,
      textAlign,
      defaultStyle,
      richContent,
      ...(rd.ok && {
        rectangleDifferences: {
          left: rd.left,
          top: rd.top,
          right: rd.right,
          bottom: rd.bottom,
        },
      }),
      ...(intent && { intent }),
      ...(calloutLine && { calloutLine }),
      ...(lineEndings && { lineEnding: lineEndings.end }),
      ...(borderStyle.width > 0
        ? { strokeWidth: borderStyle.width, strokeColor: da?.fontColor ?? '#000000' }
        : intent === 'FreeTextCallout'
          ? { strokeWidth: 1, strokeColor: da?.fontColor ?? '#000000' }
          : {}),
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 链接注解（Link Annotation）
   *
   * 链接注解是 PDF 中最常见的交互元素之一，用于表示可点击的超链接区域。
   * 本方法从 PDFium 底层注解指针中提取链接的完整信息，包括：
   *   - 矩形区域（从页面坐标转换为设备坐标）
   *   - 边框样式（实线、虚线等）和边框宽度
   *   - 描边颜色
   *   - 虚线模式（仅当边框样式为 DASHED 时读取）
   *   - 链接目标（Action 或 Destination）
   *
   * 设计决策：优先通过 FPDFAnnot_GetLink 获取底层 link 指针，
   * 若注解不含链接信息则返回 undefined，避免后续处理空指针。
   *
   * @param doc - PDF 文档对象，包含文档级元信息
   * @param page - PDF 页面对象，包含页面尺寸和旋转信息
   * @param docPtr - PDFium 文档指针（FPDF_DOCUMENT）
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解在页面中的唯一标识（通常为 NM 字典值）
   * @returns 链接注解对象，若注解不含链接信息则返回 undefined
   *
   * @private
   */
  private readPdfLinkAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    docPtr: number,
    annotationPtr: number,
    index: string,
  ): PdfLinkAnnoObject | undefined {
    // 从注解中提取底层链接指针，若注解类型不是 Link 或不包含链接则返回 0
    const linkPtr = this.pdfiumModule.FPDFAnnot_GetLink(annotationPtr);
    if (!linkPtr) {
      return;
    }

    // 读取注解在页面坐标系中的矩形范围，然后转换为设备坐标系
    const annoRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, annoRect);

    // ========== 类型特有属性 ==========
    // 读取边框样式（实线/虚线/点线等）和边框宽度
    const { style: strokeStyle, width: strokeWidth } = this.getBorderStyle(annotationPtr);

    // 读取描边颜色（/C 数组）
    const strokeColor = this.getAnnotationColor(annotationPtr, PdfAnnotationColorType.Color);

    // 仅当边框样式为虚线时读取虚线模式数组（/D 数组）
    let strokeDashArray: number[] | undefined;
    if (strokeStyle === PdfAnnotationBorderStyle.DASHED) {
      const { ok, pattern } = this.getBorderDashPattern(annotationPtr);
      if (ok) {
        strokeDashArray = pattern;
      }
    }

    // 读取链接目标：优先尝试获取 Action（/A），其次获取 Destination（/Dest）
    const target = this.readPdfLinkAnnoTarget(
      docPtr,
      () => {
        return this.pdfiumModule.FPDFLink_GetAction(linkPtr);
      },
      () => {
        return this.pdfiumModule.FPDFLink_GetDest(docPtr, linkPtr);
      },
    );

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.LINK,
      rect,
      target,
      strokeColor,
      strokeWidth,
      strokeStyle,
      strokeDashArray,
      // 展开基础注解属性（作者、内容、修改时间、标志位等）
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 表单控件注解（Widget Annotation）
   *
   * Widget 注解是 PDF 表单系统的核心，代表各种表单字段的可视化控件，
   * 包括文本框、复选框、单选按钮、下拉列表、列表框、按钮和签名域。
   *
   * 本方法提取的信息包括：
   *   - 控件矩形区域和页面坐标转换
   *   - 默认外观（DA）：字体族、字号、字体颜色
   *   - 表单字段信息：类型、名称、值、选项等
   *   - MK 字典颜色：边框颜色（BC）和背景颜色（BG）
   *   - 边框宽度
   *   - 按钮导出值（仅对切换类控件有效）
   *
   * 设计决策：exportValue 通过读取 AP/N 字典中的非 "Off" 键名来获取，
   * 这是 PDF 规范中单选/复选框的标准导出值约定。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param formHandle - 表单环境句柄（FPDF_FORMHANDLE），用于读取字段信息
   * @param index - 注解的唯一标识
   * @returns 表单控件注解对象
   *
   * @private
   */
  private readPdfWidgetAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    formHandle: number,
    index: string,
  ): PdfWidgetAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    // 读取注解标志位（/F 字段），用于判断 Hidden/Print/ReadOnly 等状态
    const flags = this.getAnnotationFlags(annotationPtr);
    // 读取默认外观字典（/DA），包含字体、字号和颜色信息
    const da = this.getAnnotationDefaultAppearance(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 读取表单字段的完整信息（类型、名称、值、选项列表等）
    const field = this.readPdfWidgetAnnoField(formHandle, annotationPtr);

    // 读取按钮的稳定导出值：从外观流 AP/N 字典中获取非 "Off" 的键名
    // 这是 PDF 规范中单选/复选框的标准约定
    const exportValue = this.readButtonExportValue(annotationPtr);

    // 从 MK（外观特征）字典中读取边框颜色（BC）和背景颜色（BG）
    const strokeColor = this.getMKColor(annotationPtr, 0); // EPDF_MK_COLOR_BC 边框颜色
    const color = this.getMKColor(annotationPtr, 1); // EPDF_MK_COLOR_BG 背景颜色

    // 读取边框宽度（/BS 字典中的 W 值）
    const { width: strokeWidth } = this.getBorderStyle(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.WIDGET,
      fontFamily: da?.fontFamily ?? PdfStandardFont.Unknown,
      fontSize: da?.fontSize ?? 12,
      fontColor: da?.fontColor ?? '#000000',
      rect,
      field,
      ...(exportValue !== undefined && { exportValue }),
      strokeWidth,
      strokeColor: strokeColor ?? 'transparent',
      color: color ?? 'transparent',
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 文件附件注解（File Attachment Annotation）
   *
   * 文件附件注解用于在 PDF 页面上嵌入一个外部文件引用，
   * 用户点击该注解时可以打开或保存嵌入的文件。
   * 本方法提取注解的矩形区域和基础属性，文件内容通过附件 API 单独获取。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 文件附件注解对象
   *
   * @private
   */
  private readPdfFileAttachmentAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfFileAttachmentAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.FILEATTACHMENT,
      rect,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 墨迹注解（Ink Annotation）
   *
   * 墨迹注解用于表示手写或自由绘制的笔迹，由一组或多组连续路径（Stroke）组成。
   * 每条路径包含一系列连续的点，形成手写笔迹的完整轨迹。
   *
   * 本方法提取的信息包括：
   *   - 笔迹列表（inkList）：每组为一条连续路径
   *   - 描边颜色（默认红色 #FF0000）
   *   - 不透明度
   *   - 笔画宽度（默认 1pt）
   *   - 意图（Intent）：如 "InkList" 标识手写注解
   *
   * 设计决策：strokeWidth 为 0 时强制设为 1，保证笔迹可见；
   * color 字段作为 strokeColor 的兼容别名保留。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 墨迹注解对象
   *
   * @private
   */
  private readPdfInkAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfInkAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 描边颜色，默认红色（PDF 规范中 Ink 注解的默认颜色）
    const strokeColor = this.getAnnotationColor(annotationPtr) ?? '#FF0000';
    const opacity = this.getAnnotationOpacity(annotationPtr);
    // 笔画宽度为 0 时在返回值中强制修正为 1，确保笔迹始终可见
    const { width: strokeWidth } = this.getBorderStyle(annotationPtr);
    // 读取墨迹路径列表，每组为一条连续手写笔画
    const inkList = this.getInkList(doc, page, annotationPtr);
    const intent = this.getAnnotIntent(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.INK,
      rect,
      ...(intent && { intent }),
      strokeColor,
      color: strokeColor, // 兼容旧接口的别名
      opacity,
      strokeWidth: strokeWidth === 0 ? 1 : strokeWidth,
      inkList,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 多边形注解（Polygon Annotation）
   *
   * 多边形注解用于在 PDF 页面上绘制闭合多边形形状。
   * 本方法提取的信息包括：
   *   - 顶点列表（vertices）：多边形的各个顶点坐标
   *   - 描边颜色和填充颜色（InteriorColor）
   *   - 不透明度、边框样式、边框宽度
   *   - 虚线模式（仅当边框样式为 DASHED 时）
   *   - 矩形差异（RD）：内边距调整
   *   - 云状边框效果（BE）：边框模糊强度
   *
   * 设计决策：移除首尾重复的闭合顶点（PDF 规范中多边形首尾顶点相同是合法的，
   * 但前端渲染时会自动闭合，因此去除冗余点）。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 多边形注解对象
   *
   * @private
   */
  private readPdfPolygonAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfPolygonAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 读取多边形顶点坐标列表（页面坐标系），并转换为设备坐标系
    const vertices = this.readPdfAnnoVertices(doc, page, annotationPtr);
    const strokeColor = this.getAnnotationColor(annotationPtr);
    // 内部填充颜色（/IC 数组），多边形内部区域颜色
    const interiorColor = this.getAnnotationColor(
      annotationPtr,
      PdfAnnotationColorType.InteriorColor,
    );
    const opacity = this.getAnnotationOpacity(annotationPtr);
    const { style: strokeStyle, width: strokeWidth } = this.getBorderStyle(annotationPtr);

    // 仅当边框样式为虚线时读取虚线模式数组
    let strokeDashArray: number[] | undefined;
    if (strokeStyle === PdfAnnotationBorderStyle.DASHED) {
      const { ok, pattern } = this.getBorderDashPattern(annotationPtr);
      if (ok) {
        strokeDashArray = pattern;
      }
    }

    // 移除首尾重复的闭合顶点：PDF 规范中多边形首尾顶点可能相同，
    // 但前端渲染时会自动闭合路径，因此去除冗余的最后一个点
    if (vertices.length > 1) {
      const first = vertices[0];
      const last = vertices[vertices.length - 1];
      if (first.x === last.x && first.y === last.y) {
        vertices.pop();
      }
    }

    // 读取矩形差异（/RD）：注解矩形与实际绘制区域的内边距
    const rd = this.getRectangleDifferences(annotationPtr);
    // 读取边框效果（/BE）：云状边框的模糊强度
    const be = this.getBorderEffect(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.POLYGON,
      rect,
      strokeColor: strokeColor ?? '#FF0000',
      color: interiorColor ?? 'transparent',
      opacity,
      strokeWidth: strokeWidth === 0 ? 1 : strokeWidth,
      strokeStyle,
      strokeDashArray,
      vertices,
      ...(be.ok && { cloudyBorderIntensity: be.intensity }),
      ...(rd.ok && {
        rectangleDifferences: {
          left: rd.left,
          top: rd.top,
          right: rd.right,
          bottom: rd.bottom,
        },
      }),
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 折线注解（Polyline Annotation）
   *
   * 折线注解与多边形注解类似，但不自动闭合路径。
   * 由一系列相连的直线段组成开放路径，常用于标注和绘制箭头。
   *
   * 本方法提取的信息包括：
   *   - 顶点列表（vertices）：折线各顶点坐标
   *   - 描边颜色和填充颜色
   *   - 不透明度、边框样式、边框宽度
   *   - 虚线模式（仅当边框样式为 DASHED 时）
   *   - 线端样式（lineEndings）：起点和终点的箭头/圆点等装饰
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 折线注解对象
   *
   * @private
   */
  private readPdfPolylineAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfPolylineAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    const vertices = this.readPdfAnnoVertices(doc, page, annotationPtr);
    const strokeColor = this.getAnnotationColor(annotationPtr);
    // 内部填充颜色（/IC 数组）
    const interiorColor = this.getAnnotationColor(
      annotationPtr,
      PdfAnnotationColorType.InteriorColor,
    );
    const opacity = this.getAnnotationOpacity(annotationPtr);
    const { style: strokeStyle, width: strokeWidth } = this.getBorderStyle(annotationPtr);

    let strokeDashArray: number[] | undefined;
    if (strokeStyle === PdfAnnotationBorderStyle.DASHED) {
      const { ok, pattern } = this.getBorderDashPattern(annotationPtr);
      if (ok) {
        strokeDashArray = pattern;
      }
    }
    // 读取线端装饰样式（/LE 数组）：起点和终点的箭头、圆点等装饰
    const lineEndings = this.getLineEndings(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.POLYLINE,
      rect,
      strokeColor: strokeColor ?? '#FF0000',
      color: interiorColor ?? 'transparent',
      opacity,
      strokeWidth: strokeWidth === 0 ? 1 : strokeWidth,
      strokeStyle,
      strokeDashArray,
      lineEndings,
      vertices,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 直线注解（Line Annotation）
   *
   * 直线注解用于在 PDF 页面上绘制一条带装饰的直线段。
   * 常用于标注和指示，可以带有起点/终点的箭头等装饰样式。
   *
   * 本方法提取的信息包括：
   *   - 线段端点（linePoints）：起点和终点坐标
   *   - 线端样式（lineEndings）：起点和终点的装饰（箭头、圆点等）
   *   - 描边颜色和填充颜色（InteriorColor，用于线端装饰填充）
   *   - 不透明度、边框样式、边框宽度
   *   - 虚线模式（仅当边框样式为 DASHED 时）
   *
   * 设计决策：linePoints 和 lineEndings 均提供默认值，
   * 确保即使 PDF 缺少 /L 或 /LE 字段也不返回 undefined。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 直线注解对象
   *
   * @private
   */
  private readPdfLineAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfLineAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 读取线段的起点和终点坐标（/L 数组），从页面坐标转换为设备坐标
    const linePoints = this.getLinePoints(doc, page, annotationPtr);
    // 读取线端装饰样式（/LE 数组）
    const lineEndings = this.getLineEndings(annotationPtr);
    const strokeColor = this.getAnnotationColor(annotationPtr);
    // 内部填充颜色（/IC 数组），用于填充线端装饰（如箭头三角形）
    const interiorColor = this.getAnnotationColor(
      annotationPtr,
      PdfAnnotationColorType.InteriorColor,
    );
    const opacity = this.getAnnotationOpacity(annotationPtr);
    const { style: strokeStyle, width: strokeWidth } = this.getBorderStyle(annotationPtr);

    let strokeDashArray: number[] | undefined;
    if (strokeStyle === PdfAnnotationBorderStyle.DASHED) {
      const { ok, pattern } = this.getBorderDashPattern(annotationPtr);
      if (ok) {
        strokeDashArray = pattern;
      }
    }

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.LINE,
      rect,
      strokeWidth: strokeWidth === 0 ? 1 : strokeWidth,
      strokeStyle,
      strokeDashArray,
      strokeColor: strokeColor ?? '#FF0000',
      color: interiorColor ?? 'transparent',
      opacity,
      linePoints: linePoints || { start: { x: 0, y: 0 }, end: { x: 0, y: 0 } },
      lineEndings: lineEndings || {
        start: PdfAnnotationLineEnding.None,
        end: PdfAnnotationLineEnding.None,
      },
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 高亮注解（Highlight Annotation）
   *
   * 高亮注解用于标记文本内容，是文本标记注解（Text Markup Annotation）的一种。
   * 通过 QuadPoints 定义高亮区域，通常跨越多行文本。
   *
   * 本方法提取的信息包括：
   *   - 分段矩形（segmentRects）：由 QuadPoints 定义的多个高亮区域
   *   - 描边颜色（默认黄色 #FFFF00）
   *   - 不透明度
   *
   * 设计决策：color 字段作为 strokeColor 的兼容别名保留。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 高亮注解对象
   *
   * @private
   */
  private readPdfHighlightAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfHighlightAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 读取 QuadPoints 定义的高亮分段区域，转换为设备坐标矩形数组
    const segmentRects = this.getQuadPointsAnno(doc, page, annotationPtr);
    // 高亮注解默认颜色为黄色（PDF 规范推荐）
    const strokeColor = this.getAnnotationColor(annotationPtr) ?? '#FFFF00';
    const opacity = this.getAnnotationOpacity(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.HIGHLIGHT,
      rect,
      segmentRects,
      strokeColor,
      color: strokeColor, // deprecated alias
      opacity,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 下划线注解（Underline Annotation）
   *
   * 下划线注解是文本标记注解的一种，用于在文本下方添加下划线标记。
   * 通过 QuadPoints 定义下划线区域，默认颜色为红色。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 下划线注解对象
   *
   * @private
   */
  private readPdfUnderlineAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfUnderlineAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 读取 QuadPoints 定义的下划线分段区域
    const segmentRects = this.getQuadPointsAnno(doc, page, annotationPtr);
    // 下划线注解默认颜色为红色
    const strokeColor = this.getAnnotationColor(annotationPtr) ?? '#FF0000';
    const opacity = this.getAnnotationOpacity(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.UNDERLINE,
      rect,
      segmentRects,
      strokeColor,
      color: strokeColor, // deprecated alias
      opacity,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 删除线注解（StrikeOut Annotation）
   *
   * 删除线注解是文本标记注解的一种，用于在文本中间添加水平删除线。
   * 通过 QuadPoints 定义删除线区域，默认颜色为红色。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 删除线注解对象
   *
   * @private
   */
  private readPdfStrikeOutAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfStrikeOutAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 读取 QuadPoints 定义的删除线分段区域
    const segmentRects = this.getQuadPointsAnno(doc, page, annotationPtr);
    // 删除线注解默认颜色为红色
    const strokeColor = this.getAnnotationColor(annotationPtr) ?? '#FF0000';
    const opacity = this.getAnnotationOpacity(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.STRIKEOUT,
      rect,
      segmentRects,
      strokeColor,
      color: strokeColor, // deprecated alias
      opacity,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 波浪线注解（Squiggly Annotation）
   *
   * 波浪线注解是文本标记注解的一种，用于在文本下方添加波浪形下划线。
   * 常用于标记拼写错误或需要注意的文本内容，默认颜色为红色。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 波浪线注解对象
   *
   * @private
   */
  private readPdfSquigglyAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfSquigglyAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 读取 QuadPoints 定义的波浪线分段区域
    const segmentRects = this.getQuadPointsAnno(doc, page, annotationPtr);
    // 波浪线注解默认颜色为红色
    const strokeColor = this.getAnnotationColor(annotationPtr) ?? '#FF0000';
    const opacity = this.getAnnotationOpacity(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.SQUIGGLY,
      rect,
      segmentRects,
      strokeColor,
      color: strokeColor, // deprecated alias
      opacity,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 插入符注解（Caret Annotation）
   *
   * 插入符注解用于标记文本插入位置，通常显示为一个 "^" 形状的符号。
   * 常用于文档审阅场景，标识需要插入新内容的位置。
   *
   * 本方法提取的信息包括：
   *   - 矩形区域和坐标转换
   *   - 描边颜色、不透明度
   *   - 意图（Intent）：如 "Caret" 标识普通插入符
   *   - 矩形差异（RD）：实际符号相对于注解矩形的偏移
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 插入符注解对象
   *
   * @private
   */
  private readPdfCaretAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfCaretAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);
    const strokeColor = this.getAnnotationColor(annotationPtr);
    const opacity = this.getAnnotationOpacity(annotationPtr);
    const intent = this.getAnnotIntent(annotationPtr);
    const rd = this.getRectangleDifferences(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.CARET,
      rect,
      strokeColor,
      opacity,
      intent,
      ...(rd.ok && {
        rectangleDifferences: {
          left: rd.left,
          top: rd.top,
          right: rd.right,
          bottom: rd.bottom,
        },
      }),
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 涂黑注解（Redact Annotation）
   *
   * 涂黑注解用于标记需要永久删除（涂黑）的内容区域。
   * 在执行涂黑操作时，被标记区域的内容将被永久移除。
   *
   * 本方法提取的信息包括：
   *   - 分段矩形（segmentRects）：由 QuadPoints 定义的涂黑区域
   *   - 三种颜色：内部颜色（IC）、覆盖颜色（OC）、描边颜色（C）
   *   - 覆盖文本（overlayText）：涂黑后显示的替代文本
   *   - 覆盖文本重复（overlayTextRepeat）：是否重复填充覆盖文本
   *   - 字体属性：从 DA 字典中读取字体族、字号、颜色
   *   - 文本对齐方式
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 涂黑注解对象
   *
   * @private
   */
  private readPdfRedactAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfRedactAnnoObject {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // QuadPoints 定义的涂黑区域
    const segmentRects = this.getQuadPointsAnno(doc, page, annotationPtr);

    // 颜色：IC = 内部/预览色，OC = 覆盖色，C = 描边色
    const color = this.getAnnotationColor(annotationPtr, PdfAnnotationColorType.InteriorColor);
    const overlayColor = this.getAnnotationColor(
      annotationPtr,
      PdfAnnotationColorType.OverlayColor,
    );
    const strokeColor = this.getAnnotationColor(annotationPtr, PdfAnnotationColorType.Color);
    const opacity = this.getAnnotationOpacity(annotationPtr);

    // 覆盖文本属性
    const overlayText = this.getOverlayText(annotationPtr);
    const overlayTextRepeat = this.getOverlayTextRepeat(annotationPtr);

    // 从 DA（默认外观）字典中读取字体属性
    const da = this.getAnnotationDefaultAppearance(annotationPtr);
    const textAlign = this.getAnnotationTextAlignment(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.REDACT,
      rect,
      segmentRects,
      color,
      overlayColor,
      strokeColor,
      opacity,
      overlayText,
      overlayTextRepeat,
      fontFamily: da?.fontFamily,
      fontSize: da?.fontSize,
      fontColor: da?.fontColor,
      textAlign,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 图章注解（Stamp Annotation）
   *
   * 图章注解用于在 PDF 页面上放置预定义的图形标记，
   * 如 "Approved"（已批准）、"Confidential"（机密）、"Draft"（草稿）等。
   * 图章内容通过外观流（Appearance Stream）渲染。
   *
   * 本方法提取的信息包括：
   *   - 矩形区域
   *   - 名称（name）：图章的类型标识，对应 PDF 规范中的标准图章名称
   *   - icon：与 name 相同，用于前端兼容
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 图章注解对象
   *
   * @private
   */
  private readPdfStampAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfStampAnnoObject | undefined {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);
    const name = this.getAnnotationName(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.STAMP,
      rect,
      name,
      icon: name,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 页面对象
   *
   * PDF 页面内容由多种对象组成，本方法根据对象类型分发到对应的读取函数：
   *   - PATH：路径对象（矢量图形，由线段和曲线组成）
   *   - IMAGE：图像对象（嵌入的位图）
   *   - FORM：表单 XObject（可复用的内容组，可包含子对象）
   *
   * 其他类型的页面对象（如 Text）会被忽略，返回 undefined。
   *
   * @param pageObjectPtr - PDFium 页面对象指针（FPDF_PAGEOBJECT）
   * @returns 页面对象数据，类型取决于对象类型
   *
   * @private
   */
  private readPdfPageObject(pageObjectPtr: number) {
    // 获取页面对象的类型枚举值
    const type = this.pdfiumModule.FPDFPageObj_GetType(pageObjectPtr) as PdfPageObjectType;
    switch (type) {
      case PdfPageObjectType.PATH:
        return this.readPathObject(pageObjectPtr);
      case PdfPageObjectType.IMAGE:
        return this.readImageObject(pageObjectPtr);
      case PdfPageObjectType.FORM:
        return this.readFormObject(pageObjectPtr);
    }
  }

  /**
   * 读取 PDF 路径对象
   *
   * 路径对象是 PDF 中的矢量图形元素，由一系列路径段（Segment）组成。
   * 每个段可以是移动到（MoveTo）、画线到（LineTo）、贝塞尔曲线（CurveTo）
   * 或闭合路径（ClosePath）等操作。
   *
   * 本方法提取的信息包括：
   *   - 路径边界（bounds）：路径对象的包围矩形
   *   - 路径段列表（segments）：组成路径的所有线段/曲线段
   *   - 变换矩阵（matrix）：路径对象的坐标变换矩阵
   *
   * 注意：所有坐标值从 WASM 线性内存中通过指针读取（4字节 float）。
   *
   * @param pathObjectPtr - PDFium 路径对象指针（FPDF_PAGEOBJECT）
   * @returns 路径对象数据
   *
   * @private
   */
  private readPathObject(pathObjectPtr: number): PdfPathObject {
    // 获取路径段的数量
    const segmentCount = this.pdfiumModule.FPDFPath_CountSegments(pathObjectPtr);

    // 分配 WASM 内存用于接收路径对象的边界矩形（left, bottom, right, top）
    const leftPtr = this.memoryManager.malloc(4);
    const bottomPtr = this.memoryManager.malloc(4);
    const rightPtr = this.memoryManager.malloc(4);
    const topPtr = this.memoryManager.malloc(4);
    this.pdfiumModule.FPDFPageObj_GetBounds(pathObjectPtr, leftPtr, bottomPtr, rightPtr, topPtr);
    // 从 WASM 线性内存中读取 float32 值
    const left = this.pdfiumModule.pdfium.getValue(leftPtr, 'float');
    const bottom = this.pdfiumModule.pdfium.getValue(bottomPtr, 'float');
    const right = this.pdfiumModule.pdfium.getValue(rightPtr, 'float');
    const top = this.pdfiumModule.pdfium.getValue(topPtr, 'float');
    const bounds = { left, bottom, right, top };
    this.memoryManager.free(leftPtr);
    this.memoryManager.free(bottomPtr);
    this.memoryManager.free(rightPtr);
    this.memoryManager.free(topPtr);
    // 逐段读取路径的所有段信息
    const segments: PdfSegmentObject[] = [];
    for (let i = 0; i < segmentCount; i++) {
      const segment = this.readPdfSegment(pathObjectPtr, i);
      segments.push(segment);
    }

    // 读取路径对象的变换矩阵（6 个 float32 值组成的仿射变换）
    const matrix = this.readPdfPageObjectTransformMatrix(pathObjectPtr);

    return {
      type: PdfPageObjectType.PATH,
      bounds,
      segments,
      matrix,
    };
  }

  /**
   * 读取 PDF 路径中的单个段（Segment）
   *
   * 路径段是构成路径对象的基本元素，每个段描述一个绘图操作：
   *   - 类型（segmentType）：移动到/画线到/贝塞尔曲线/闭合路径等
   *   - 控制点（point）：该段操作的坐标点
   *   - 是否闭合（isClosed）：该段是否闭合当前子路径
   *
   * 注意：参数名 annotationObjectPtr 实际接收的是路径对象指针，而非注解指针。
   *
   * @param annotationObjectPtr - PDFium 路径对象指针（FPDF_PAGEOBJECT，命名有历史原因）
   * @param segmentIndex - 路径段的索引，从 0 开始
   * @returns 路径段数据
   *
   * @private
   */
  private readPdfSegment(annotationObjectPtr: number, segmentIndex: number): PdfSegmentObject {
    // 获取指定索引的路径段指针
    const segmentPtr = this.pdfiumModule.FPDFPath_GetPathSegment(annotationObjectPtr, segmentIndex);
    // 读取段的类型（MoveTo=0, LineTo=1, BezierTo=2, ClosePath=3 等）
    const segmentType = this.pdfiumModule.FPDFPathSegment_GetType(segmentPtr);
    // 判断该段是否闭合当前子路径
    const isClosed = this.pdfiumModule.FPDFPathSegment_GetClose(segmentPtr);
    // 读取段的控制点坐标（2 个 float32 值）
    const pointXPtr = this.memoryManager.malloc(4);
    const pointYPtr = this.memoryManager.malloc(4);
    this.pdfiumModule.FPDFPathSegment_GetPoint(segmentPtr, pointXPtr, pointYPtr);
    const pointX = this.pdfiumModule.pdfium.getValue(pointXPtr, 'float');
    const pointY = this.pdfiumModule.pdfium.getValue(pointYPtr, 'float');
    this.memoryManager.free(pointXPtr);
    this.memoryManager.free(pointYPtr);

    return {
      type: segmentType,
      point: { x: pointX, y: pointY },
      isClosed,
    };
  }

  /**
   * 读取 PDF 图像对象
   *
   * 图像对象是 PDF 中嵌入的位图数据。本方法从 PDFium 中提取图像的像素数据，
   * 将其从 PDFium 的内部格式转换为标准的 RGBA 格式。
   *
   * 转换过程：
   *   1. 通过 FPDFImageObj_GetBitmap 获取图像的位图句柄
   *   2. 读取位图的缓冲区指针、宽高和像素格式
   *   3. 逐像素从 WASM 内存中读取数据，按格式进行颜色通道转换
   *   4. BGR 格式转为 RGBA 格式（注意通道顺序交换和 alpha 设置）
   *
   * 注意：当前仅处理 BGR 格式，其他格式（BGRA、RGBA 等）暂未实现。
   * alpha 通道固定设为 100（半透明），这是历史遗留行为。
   *
   * @param imageObjectPtr - PDFium 图像对象指针（FPDF_PAGEOBJECT）
   * @returns 图像对象数据，包含像素数据和变换矩阵
   *
   * @private
   */
  private readImageObject(imageObjectPtr: number): PdfImageObject {
    // 获取图像对象的位图句柄
    const bitmapPtr = this.pdfiumModule.FPDFImageObj_GetBitmap(imageObjectPtr);
    // 获取位图在 WASM 内存中的像素缓冲区指针
    const bitmapBufferPtr = this.pdfiumModule.FPDFBitmap_GetBuffer(bitmapPtr);
    const bitmapWidth = this.pdfiumModule.FPDFBitmap_GetWidth(bitmapPtr);
    const bitmapHeight = this.pdfiumModule.FPDFBitmap_GetHeight(bitmapPtr);
    const format = this.pdfiumModule.FPDFBitmap_GetFormat(bitmapPtr) as BitmapFormat;

    // 逐像素转换颜色格式
    const pixelCount = bitmapWidth * bitmapHeight;
    const bytesPerPixel = 4; // RGBA
    const array = new Uint8ClampedArray(pixelCount * bytesPerPixel);
    for (let i = 0; i < pixelCount; i++) {
      switch (format) {
        case BitmapFormat.Bitmap_BGR:
          {
            // BGR 格式：每像素 3 字节，需要交换通道顺序并添加 alpha
            const blue = this.pdfiumModule.pdfium.getValue(bitmapBufferPtr + i * 3, 'i8');
            const green = this.pdfiumModule.pdfium.getValue(bitmapBufferPtr + i * 3 + 1, 'i8');
            const red = this.pdfiumModule.pdfium.getValue(bitmapBufferPtr + i * 3 + 2, 'i8');
            array[i * bytesPerPixel] = red;     // R
            array[i * bytesPerPixel + 1] = green; // G
            array[i * bytesPerPixel + 2] = blue;  // B
            array[i * bytesPerPixel + 3] = 100;   // A (半透明)
          }
          break;
      }
    }

    // 返回纯对象（ImageDataLike），不使用浏览器特定的 ImageData，
    // 以确保在 Node.js 等非浏览器环境中的兼容性
    const imageDataLike: ImageDataLike = {
      data: array,
      width: bitmapWidth,
      height: bitmapHeight,
    };
    const matrix = this.readPdfPageObjectTransformMatrix(imageObjectPtr);

    return {
      type: PdfPageObjectType.IMAGE,
      imageData: imageDataLike,
      matrix,
    };
  }

  /**
   * 读取 PDF 表单 XObject（Form XObject）
   *
   * Form XObject 是 PDF 中的可复用内容组，可以包含路径、图像甚至嵌套的表单对象。
   * 本方法递归读取表单 XObject 中的所有子对象。
   *
   * 注意：这里的 "Form" 指的是 Form XObject（PDF 内容层面的分组机制），
   * 不是交互式表单（AcroForm），两者是不同的概念。
   *
   * @param formObjectPtr - PDFium 表单 XObject 指针（FPDF_PAGEOBJECT）
   * @returns 表单 XObject 数据，包含子对象列表和变换矩阵
   *
   * @private
   */
  private readFormObject(formObjectPtr: number): PdfFormObject {
    // 获取表单 XObject 中包含的子对象数量
    const objectCount = this.pdfiumModule.FPDFFormObj_CountObjects(formObjectPtr);
    const objects: (PdfFormObject | PdfImageObject | PdfPathObject)[] = [];
    // 逐个读取子对象（递归处理，子对象也可能是 Form XObject）
    for (let i = 0; i < objectCount; i++) {
      const pageObjectPtr = this.pdfiumModule.FPDFFormObj_GetObject(formObjectPtr, i);
      const pageObj = this.readPdfPageObject(pageObjectPtr);
      if (pageObj) {
        objects.push(pageObj);
      }
    }
    // 读取表单 XObject 的变换矩阵
    const matrix = this.readPdfPageObjectTransformMatrix(formObjectPtr);

    return {
      type: PdfPageObjectType.FORM,
      objects,
      matrix,
    };
  }

  /**
   * 读取 PDF 页面对象的变换矩阵
   *
   * 变换矩阵是一个 2D 仿射变换矩阵，由 6 个 float32 值组成：
   *   [a, b, c, d, e, f]
   * 对应的变换为：
   *   | a  b  0 |
   *   | c  d  0 |
   *   | e  f  1 |
   * 其中 (a,d) 控制缩放，(b,c) 控制旋转/倾斜，(e,f) 控制平移。
   *
   * 如果获取失败，返回单位矩阵 {a:1, b:0, c:0, d:1, e:0, f:0}。
   *
   * @param pageObjectPtr - PDFium 页面对象指针
   * @returns 变换矩阵数据
   *
   * @private
   */
  private readPdfPageObjectTransformMatrix(pageObjectPtr: number): PdfTransformMatrix {
    // 分配 24 字节（6 个 float32）用于接收变换矩阵
    const matrixPtr = this.memoryManager.malloc(4 * 6);
    if (this.pdfiumModule.FPDFPageObj_GetMatrix(pageObjectPtr, matrixPtr)) {
      // 按顺序读取 6 个矩阵元素，每个占 4 字节
      const a = this.pdfiumModule.pdfium.getValue(matrixPtr, 'float');      // 水平缩放
      const b = this.pdfiumModule.pdfium.getValue(matrixPtr + 4, 'float');  // 水平倾斜
      const c = this.pdfiumModule.pdfium.getValue(matrixPtr + 8, 'float');  // 垂直倾斜
      const d = this.pdfiumModule.pdfium.getValue(matrixPtr + 12, 'float'); // 垂直缩放
      const e = this.pdfiumModule.pdfium.getValue(matrixPtr + 16, 'float'); // 水平平移
      const f = this.pdfiumModule.pdfium.getValue(matrixPtr + 20, 'float'); // 垂直平移
      this.memoryManager.free(matrixPtr);

      return { a, b, c, d, e, f };
    }

    this.memoryManager.free(matrixPtr);

    // 获取失败时返回单位矩阵（无变换）
    return { a: 1, b: 0, c: 0, d: 1, e: 0, f: 0 };
  }

  /**
   * 读取图章注解的内容（子对象列表）
   *
   * 图章注解的视觉内容由其内部的子对象（路径、图像、表单 XObject）组成。
   * 本方法遍历图章注解的所有子对象，逐个读取其内容。
   *
   * 注意：自定义图章（Custom Stamp）通常包含路径和图像对象的组合，
   * 标准图章（如 Approved、Draft 等）则由预定义的外观流渲染。
   *
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @returns 图章注解的子对象列表
   *
   * @private
   */
  private readStampAnnotationContents(annotationPtr: number): PdfStampAnnoObjectContents {
    const contents: PdfStampAnnoObjectContents = [];

    // 获取注解中包含的页面对象数量
    const objectCount = this.pdfiumModule.FPDFAnnot_GetObjectCount(annotationPtr);
    for (let i = 0; i < objectCount; i++) {
      const annotationObjectPtr = this.pdfiumModule.FPDFAnnot_GetObject(annotationPtr, i);

      const pageObj = this.readPdfPageObject(annotationObjectPtr);
      if (pageObj) {
        contents.push(pageObj);
      }
    }

    return contents;
  }

  /**
   * 从注解的 /Border 或 /BS（Border Style）字典中读取描边宽度。
   * 当注解未定义边框时，默认返回 1 pt（磅）。。
   *
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @returns 描边宽度（单位：磅），未定义时返回 1
   *
   * @private
   */
  private getStrokeWidth(annotationPtr: number): number {
    // FPDFAnnot_GetBorder(annot, &hRadius, &vRadius, &borderWidth)
    const hPtr = this.memoryManager.malloc(4);
    const vPtr = this.memoryManager.malloc(4);
    const wPtr = this.memoryManager.malloc(4);

    const ok = this.pdfiumModule.FPDFAnnot_GetBorder(annotationPtr, hPtr, vPtr, wPtr);
    const width = ok ? this.pdfiumModule.pdfium.getValue(wPtr, 'float') : 1; // default 1 pt

    this.memoryManager.free(hPtr);
    this.memoryManager.free(vPtr);
    this.memoryManager.free(wPtr);

    return width;
  }

  /**
   * 获取注解的标志位（/F 字段）
   *
   * 注解标志位控制注解的显示和行为特性，常见的标志包括：
   *   - Invisible：不可见
   *   - Hidden：隐藏（取决于查看器设置）
   *   - Print：打印时显示
   *   - NoZoom：不随缩放而改变大小
   *   - NoRotate：不随页面旋转而旋转
   *   - NoView：不在屏幕上显示（但可打印）
   *   - ReadOnly：只读（不可交互）
   *   - Locked：锁定（不可删除/修改）
   *   - ToggleNoView：鼠标悬停时切换可见性
   *
   * 本方法读取 32 位标志位整数值，然后转换为标志名称数组。
   *
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @returns 标志名称数组
   */
  private getAnnotationFlags(annotationPtr: number): PdfAnnotationFlagName[] {
    // 读取注解的 32 位标志位整数
    const rawFlags = this.pdfiumModule.FPDFAnnot_GetFlags(annotationPtr); // number

    // 将位标志转换为可读的标志名称数组
    return flagsToNames(rawFlags);
  }

  /**
   * 设置注解的标志位（/F 字段）
   *
   * 将标志名称数组转换回位标志整数，然后写入注解。
   *
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param flags - 要设置的标志名称数组
   * @returns 是否设置成功
   */
  private setAnnotationFlags(annotationPtr: number, flags: PdfAnnotationFlagName[]): boolean {
    const rawFlags = namesToFlags(flags);
    return this.pdfiumModule.FPDFAnnot_SetFlags(annotationPtr, rawFlags);
  }

  /**
   * 读取 PDF 圆形注解（Circle Annotation）
   *
   * 圆形注解用于在 PDF 页面上绘制椭圆或圆形标记。
   * 本方法提取的信息包括：
   *   - 内部填充颜色（InteriorColor）：圆形内部区域的填充色
   *   - 描边颜色（默认红色 #FF0000）
   *   - 不透明度、边框样式、边框宽度
   *   - 虚线模式（仅当边框样式为 DASHED 时）
   *   - 矩形差异（RD）：实际绘制区域相对于注解矩形的偏移
   *   - 云状边框效果（BE）：边框的云状/模糊效果强度
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 圆形注解对象
   *
   * @private
   */
  private readPdfCircleAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfCircleAnnoObject {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 内部填充颜色（/IC 数组）
    const interiorColor = this.getAnnotationColor(
      annotationPtr,
      PdfAnnotationColorType.InteriorColor,
    );
    const strokeColor = this.getAnnotationColor(annotationPtr);
    const opacity = this.getAnnotationOpacity(annotationPtr);
    const { style: strokeStyle, width: strokeWidth } = this.getBorderStyle(annotationPtr);

    // 仅当边框样式为虚线时读取虚线模式
    let strokeDashArray: number[] | undefined;
    if (strokeStyle === PdfAnnotationBorderStyle.DASHED) {
      const { ok, pattern } = this.getBorderDashPattern(annotationPtr);
      if (ok) {
        strokeDashArray = pattern;
      }
    }

    // 读取矩形差异和边框效果
    const rd = this.getRectangleDifferences(annotationPtr);
    const be = this.getBorderEffect(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.CIRCLE,
      rect,
      color: interiorColor ?? 'transparent',
      opacity,
      strokeWidth,
      strokeColor: strokeColor ?? '#FF0000',
      strokeStyle,
      ...(strokeDashArray !== undefined && { strokeDashArray }),
      ...(be.ok && { cloudyBorderIntensity: be.intensity }),
      ...(rd.ok && {
        rectangleDifferences: {
          left: rd.left,
          top: rd.top,
          right: rd.right,
          bottom: rd.bottom,
        },
      }),
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取 PDF 方形注解（Square Annotation）
   *
   * 方形注解用于在 PDF 页面上绘制矩形标记。
   * 结构与圆形注解类似，但绘制的是矩形形状。
   * 本方法提取的信息与圆形注解相同：
   *   - 内部填充颜色、描边颜色、不透明度
   *   - 边框样式、边框宽度、虚线模式
   *   - 矩形差异（RD）和云状边框效果（BE）
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 方形注解对象
   *
   * @private
   */
  private readPdfSquareAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
    index: string,
  ): PdfSquareAnnoObject {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    // ========== 类型特有属性 ==========
    // 内部填充颜色（/IC 数组）
    const interiorColor = this.getAnnotationColor(
      annotationPtr,
      PdfAnnotationColorType.InteriorColor,
    );
    const strokeColor = this.getAnnotationColor(annotationPtr);
    const opacity = this.getAnnotationOpacity(annotationPtr);
    const { style: strokeStyle, width: strokeWidth } = this.getBorderStyle(annotationPtr);

    // 仅当边框样式为虚线时读取虚线模式
    let strokeDashArray: number[] | undefined;
    if (strokeStyle === PdfAnnotationBorderStyle.DASHED) {
      const { ok, pattern } = this.getBorderDashPattern(annotationPtr);
      if (ok) {
        strokeDashArray = pattern;
      }
    }

    // 读取矩形差异和边框效果
    const rd = this.getRectangleDifferences(annotationPtr);
    const be = this.getBorderEffect(annotationPtr);

    return {
      pageIndex: page.index,
      id: index,
      type: PdfAnnotationSubtype.SQUARE,
      rect,
      color: interiorColor ?? 'transparent',
      opacity,
      strokeColor: strokeColor ?? '#FF0000',
      strokeWidth,
      strokeStyle,
      ...(strokeDashArray !== undefined && { strokeDashArray }),
      ...(be.ok && { cloudyBorderIntensity: be.intensity }),
      ...(rd.ok && {
        rectangleDifferences: {
          left: rd.left,
          top: rd.top,
          right: rd.right,
          bottom: rd.bottom,
        },
      }),
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 读取不支持的 PDF 注解类型的基本信息
   *
   * 当遇到引擎不完全支持的注解类型时（如 Popup、Sound、Movie 等），
   * 本方法仍会读取其基本的矩形区域和基础属性，以便前端至少能显示注解的位置。
   *
   * @param doc - PDF 文档对象
   * @param page - PDF 页面对象
   * @param type - 注解类型枚举值
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @param index - 注解的唯一标识
   * @returns 不支持的注解对象（仅包含基本属性）
   *
   * @private
   */
  private readPdfAnno(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    type: PdfUnsupportedAnnoObject['type'],
    annotationPtr: number,
    index: string,
  ): PdfUnsupportedAnnoObject {
    const pageRect = this.readPageAnnoRect(annotationPtr);
    const rect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    return {
      pageIndex: page.index,
      id: index,
      type,
      rect,
      ...this.readBaseAnnotationProperties(doc, page, annotationPtr),
    };
  }

  /**
   * 获取注解的回复目标 ID（/IRT 字段）
   *
   * PDF 注解通过 /IRT（In Reply To）字段实现回复关系。
   * 本方法解析注解的 /IRT 引用，返回被回复注解的唯一标识（NM 值）。
   *
   * 处理逻辑：
   *   1. 通过 FPDFAnnot_GetLinkedAnnot 获取被引用的父注解指针
   *   2. 读取父注解的 NM（Name）字段作为唯一标识
   *   3. 如果父注解没有 NM，则自动生成一个 UUID 并写入
   *
   * 设计决策：自动补充 NM 是因为 PDF 规范允许注解没有 NM 字段，
   * 但我们的系统依赖 NM 作为注解的唯一标识符。
   *
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION）
   * @returns 父注解的 NM 值，若当前注解不是回复则返回 undefined
   *
   * @private
   */
  private getInReplyToId(annotationPtr: number): string | undefined {
    // 获取 /IRT 引用的父注解指针
    const parentPtr = this.pdfiumModule.FPDFAnnot_GetLinkedAnnot(annotationPtr, 'IRT');
    if (!parentPtr) return;

    // 读取父注解的 NM（唯一名称）字段
    let nm = this.getAnnotString(parentPtr, 'NM');
    if (!nm) {
      // 父注解没有 NM，自动生成一个 UUID 并写入，确保有唯一标识
      nm = uuidV4();
      this.setAnnotString(parentPtr, 'NM', nm);
    }

    // 关闭父注解指针，释放资源
    this.pdfiumModule.FPDFPage_CloseAnnot(parentPtr);
    return nm;
  }

  /**
   * 设置注解的回复目标 ID（/IRT 字段）
   *
   * 将当前注解的 /IRT 字段设置为指向指定的父注解。
   * 如果 id 为 undefined，则清除回复关系。
   *
   * @param pagePtr - PDFium 页面指针（FPDF_PAGE），用于查找父注解
   * @param annotationPtr - PDFium 注解指针（FPDF_ANNOTATION），要设置回复的注解
   * @param id - 父注解的 NM 值，undefined 表示清除回复关系
   * @returns 是否设置成功
   */
  private setInReplyToId(pagePtr: number, annotationPtr: number, id?: string): boolean {
    // 如果未提供 id，清除 /IRT 字段
    if (!id) {
      return this.pdfiumModule.EPDFAnnot_SetLinkedAnnot(annotationPtr, 'IRT', 0);
    }

    // 否则，通过 NM 值在页面中查找父注解并设置链接
    const parentPtr = this.getAnnotationByName(pagePtr, id);
    if (!parentPtr) return false;

    return this.pdfiumModule.EPDFAnnot_SetLinkedAnnot(annotationPtr, 'IRT', parentPtr);
  }

  /**
   * 围绕指定中心点旋转一个坐标点
   *
   * 使用标准的 2D 旋转公式：
   *   x' = cx + (x-cx)*cos(angle) - (y-cy)*sin(angle)
   *   y' = cy + (x-cx)*sin(angle) + (y-cy)*cos(angle)
   *
   * 用于在保存注解时将顶点坐标旋转到 PDF 存储坐标系，
   * 以确保其他 PDF 查看器能正确显示旋转后的注解。
   *
   * @param point - 要旋转的点坐标
   * @param center - 旋转中心点
   * @param angleDegrees - 旋转角度（度数），正值为顺时针
   * @returns 旋转后的新坐标点
   */
  private rotatePointForSave(point: Position, center: Position, angleDegrees: number): Position {
    // 将角度转换为弧度
    const rad = (angleDegrees * Math.PI) / 180;
    const cos = Math.cos(rad);
    const sin = Math.sin(rad);
    // 计算相对于旋转中心的偏移量
    const dx = point.x - center.x;
    const dy = point.y - center.y;
    // 应用旋转矩阵并加上旋转中心偏移
    return {
      x: center.x + dx * cos - dy * sin,
      y: center.y + dx * sin + dy * cos,
    };
  }

  /**
   * 准备注解数据以保存到 PDF
   *
   * 对于包含旋转信息的顶点类注解（Ink、Line、Polygon、Polyline），
   * 在保存前需要物理旋转其顶点坐标，以确保其他 PDF 查看器能正确显示。
   *
   * 工作原理：
   *   - 我们的查看器在加载时会对旋转注解的顶点进行反向旋转来正确显示
   *   - 保存时需要执行正向旋转，将顶点恢复到 PDF 坐标系中的正确位置
   *   - 旋转中心为未旋转矩形的中心点
   *
   * 对于非顶点类型（Square、Circle、FreeText 等），不需要旋转处理，
   * 因为它们的形状由矩形定义，旋转通过 CSS 变换处理。
   *
   * @param annotation - 原始注解对象
   * @returns 处理后的注解对象（可能包含旋转后的顶点）
   *
   * @private
   */
  private prepareAnnotationForSave(annotation: PdfAnnotationObject): PdfAnnotationObject {
    const rotation = annotation.rotation;
    const unrotatedRect = annotation.unrotatedRect;

    // 无旋转或无未旋转矩形信息时，直接返回原始注解
    if (!rotation || rotation === 0 || !unrotatedRect) {
      return annotation;
    }

    // 计算未旋转矩形的中心点作为旋转中心
    const center: Position = {
      x: unrotatedRect.origin.x + unrotatedRect.size.width / 2,
      y: unrotatedRect.origin.y + unrotatedRect.size.height / 2,
    };

    // 根据注解类型分别处理顶点旋转
    switch (annotation.type) {
      case PdfAnnotationSubtype.INK: {
        // 墨迹注解：旋转每条笔画中的每个点
        const ink = annotation as PdfInkAnnoObject;
        const rotatedInkList = ink.inkList.map((stroke) => ({
          points: stroke.points.map((p) => this.rotatePointForSave(p, center, rotation)),
        }));
        return { ...ink, inkList: rotatedInkList };
      }

      case PdfAnnotationSubtype.LINE: {
        // 直线注解：旋转起点和终点
        const line = annotation as PdfLineAnnoObject;
        return {
          ...line,
          linePoints: {
            start: this.rotatePointForSave(line.linePoints.start, center, rotation),
            end: this.rotatePointForSave(line.linePoints.end, center, rotation),
          },
        };
      }

      case PdfAnnotationSubtype.POLYGON: {
        // 多边形注解：旋转所有顶点
        const poly = annotation as PdfPolygonAnnoObject;
        return {
          ...poly,
          vertices: poly.vertices.map((v) => this.rotatePointForSave(v, center, rotation)),
        };
      }

      case PdfAnnotationSubtype.POLYLINE: {
        // 折线注解：旋转所有顶点
        const polyline = annotation as PdfPolylineAnnoObject;
        return {
          ...polyline,
          vertices: polyline.vertices.map((v) => this.rotatePointForSave(v, center, rotation)),
        };
      }

      default:
        // 非顶点类型（Square、Circle、FreeText 等）不需要旋转顶点
        return annotation;
    }
  }

  /**
   * Apply all base annotation properties from PdfAnnotationObjectBase.
   * The setInReplyToId and setReplyType functions handle clearing when undefined.
   *
   * @param pagePtr - pointer to page object
   * @param annotationPtr - pointer to annotation object
   * @param annotation - the annotation object containing properties to apply
   * @returns `true` on success
   */
  private applyBaseAnnotationProperties(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pagePtr: number,
    annotationPtr: number,
    annotation: PdfAnnotationObject,
  ): boolean {
    // ===== 写入作者名称（/T 字段）=====
    if (!this.setAnnotString(annotationPtr, 'T', annotation.author || '')) {
      return false;
    }

    // ===== 写入注解内容（/Contents 字段）=====
    if (!this.setAnnotString(annotationPtr, 'Contents', annotation.contents ?? '')) {
      return false;
    }

    if (annotation.subject && !this.setAnnotString(annotationPtr, 'Subj', annotation.subject)) {
      return false;
    }

    // ===== 写入修改日期（/M 字段，PDF 日期格式）=====
    if (annotation.modified) {
      if (!this.setAnnotationDate(annotationPtr, 'M', annotation.modified)) {
        return false;
      }
    }

    // ===== 写入创建日期（/CreationDate 字段）=====
    if (annotation.created) {
      if (!this.setAnnotationDate(annotationPtr, 'CreationDate', annotation.created)) {
        return false;
      }
    }

    // ===== 写入注解标志位（/F 字段）=====
    if (annotation.flags) {
      if (!this.setAnnotationFlags(annotationPtr, annotation.flags)) {
        return false;
      }
    }

    // ===== 处理自定义数据（EPDFCustom）=====
    // 注意：旋转元数据通过 EPDFRotate / EPDFUnrotatedRect 单独存储，不在 EPDFCustom 中
    const existingCustom = this.getAnnotCustom(annotationPtr) ?? {};
    const customData = {
      ...existingCustom,
      ...(annotation.custom ?? {}),
    };

    // 移除旧版本中可能存在于 customData 中的旋转相关遗留字段
    delete customData.unrotatedRect;
    delete customData.rotation;

    const hasCustomData = Object.keys(customData).length > 0;
    if (hasCustomData) {
      if (!this.setAnnotCustom(annotationPtr, customData)) {
        return false;
      }
    } else if (Object.keys(existingCustom).length > 0) {
      // 原有自定义数据已被清空，需要删除 EPDFCustom 条目
      if (!this.setAnnotCustom(annotationPtr, null)) {
        return false;
      }
    }

    // ===== 写入 EmbedPDF 扩展旋转信息（/EPDFRotate，而非 /Rotate）=====
    // 将 UI 的顺时针角度转换为 PDF 的逆时针角度约定
    if (annotation.rotation !== undefined) {
      const pdfRotation = annotation.rotation ? (360 - annotation.rotation) % 360 : 0;
      this.setAnnotExtendedRotation(annotationPtr, pdfRotation);
    }

    // ===== 写入 EmbedPDF 未旋转矩形（/EPDFUnrotatedRect 数组）=====
    if (annotation.unrotatedRect) {
      this.setAnnotUnrotatedRect(doc, page, annotationPtr, annotation.unrotatedRect);
    } else if (annotation.rotation && annotation.rotation !== 0) {
      // 如果设置了旋转但没有提供未旋转矩形，将当前矩形存储为未旋转矩形
      this.setAnnotUnrotatedRect(doc, page, annotationPtr, annotation.rect);
    }

    // ===== 写入回复关系（/IRT 字段），setter 内部处理 undefined 清除 =====
    if (!this.setInReplyToId(pagePtr, annotationPtr, annotation.inReplyToId)) {
      return false;
    }

    // ===== 写入回复类型（/RT 字段），setter 内部处理 undefined 清除 =====
    if (!this.setReplyType(annotationPtr, annotation.replyType)) {
      return false;
    }

    return true;
  }

  /**
   * Read all base annotation properties from PdfAnnotationObjectBase.
   * Returns an object that can be spread into the annotation return value.
   *
   * @param doc - pdf document object
   * @param page - pdf page object
   * @param annotationPtr - pointer to annotation object
   * @returns object with base annotation properties
   */
  private readBaseAnnotationProperties(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
  ): {
    author: string | undefined;
    contents: string;
    modified: Date | undefined;
    created: Date | undefined;
    flags: PdfAnnotationFlagName[];
    custom: unknown;
    blendMode: PdfBlendMode;
    inReplyToId?: string;
    replyType?: PdfAnnotationReplyType;
  } {
    const author = this.getAnnotString(annotationPtr, 'T');
    const contents = this.getAnnotString(annotationPtr, 'Contents') || '';
    const modified = this.getAnnotationDate(annotationPtr, 'M');
    const created = this.getAnnotationDate(annotationPtr, 'CreationDate');
    const subject = this.getAnnotString(annotationPtr, 'Subj');
    const flags = this.getAnnotationFlags(annotationPtr);
    const custom = this.getAnnotCustom(annotationPtr);
    const inReplyToId = this.getInReplyToId(annotationPtr);
    const replyType = this.getReplyType(annotationPtr);
    const blendMode = this.pdfiumModule.EPDFAnnot_GetBlendMode(annotationPtr);

    // 读取 EmbedPDF 扩展旋转角度，并将 PDF 逆时针约定转换为 UI 顺时针约定
    const pdfRotation = this.getAnnotExtendedRotation(annotationPtr);
    const rotation = pdfRotation !== 0 ? (360 - pdfRotation) % 360 : 0;

    // 读取 EmbedPDF 未旋转矩形（原始页面坐标），并转换为设备坐标
    const rawUnrotatedRect = this.readAnnotUnrotatedRect(annotationPtr);
    const unrotatedRect = rawUnrotatedRect
      ? this.convertPageRectToDeviceRect(doc, page, rawUnrotatedRect)
      : undefined;

    return {
      author,
      contents,
      modified,
      created,
      flags,
      custom,
      blendMode,
      ...(subject && { subject }),
      // 仅当存在 IRT（回复目标）时包含
      ...(inReplyToId && { inReplyToId }),
      // 仅当 RT（回复类型）存在且不是默认值（Reply）时包含
      ...(replyType && replyType !== PdfAnnotationReplyType.Reply && { replyType }),
      ...(rotation !== 0 && { rotation }),
      ...(unrotatedRect !== undefined && { unrotatedRect }),
    };
  }

  /**
   * Fetch a string value (`/T`, `/M`, `/State`, …) from an annotation.
   *
   * @returns decoded UTF-8 string or `undefined` when the key is absent
   *
   * @private
   */
  private getAnnotString(annotationPtr: number, key: string): string | undefined {
    // 第一次调用获取字符串长度（UTF-16 编码单元数）
    const len = this.pdfiumModule.FPDFAnnot_GetStringValue(annotationPtr, key, 0, 0);
    if (len === 0) return;

    // 分配 WASM 内存（UTF-16 编码，每个字符 2 字节 + NUL 终止符）
    const bytes = (len + 1) * 2;
    const ptr = this.memoryManager.malloc(bytes);

    // 第二次调用：将实际字符串内容写入分配的内存
    this.pdfiumModule.FPDFAnnot_GetStringValue(annotationPtr, key, ptr, bytes);
    // 从 WASM 内存中将 UTF-16 字符串解码为 JS 字符串
    const value = this.pdfiumModule.pdfium.UTF16ToString(ptr);
    this.memoryManager.free(ptr);

    return value || undefined;
  }

  private readButtonExportValue(annotationPtr: number): string | undefined {
    const len = this.pdfiumModule.EPDFAnnot_GetButtonExportValue(annotationPtr, 0, 0);
    if (len === 0) return;

    const bytes = (len + 1) * 2;
    const ptr = this.memoryManager.malloc(bytes);

    this.pdfiumModule.EPDFAnnot_GetButtonExportValue(annotationPtr, ptr, bytes);
    const value = this.pdfiumModule.pdfium.UTF16ToString(ptr);
    this.memoryManager.free(ptr);

    return value || undefined;
  }

  /**
   * 获取附件的字符串属性值（如 /T、/M、/State 等）
   *
   * 工作原理与 getAnnotString 相同，但操作对象为附件指针。
   *
   * @returns decoded UTF-8 string or `undefined` when the key is absent
   *
   * @private
   */
  private getAttachmentString(attachmentPtr: number, key: string): string | undefined {
    // 第一次调用获取字符串长度
    const len = this.pdfiumModule.FPDFAttachment_GetStringValue(attachmentPtr, key, 0, 0);
    if (len === 0) return;

    // 分配 WASM 内存并第二次调用获取实际内容
    const bytes = (len + 1) * 2;
    const ptr = this.memoryManager.malloc(bytes);

    this.pdfiumModule.FPDFAttachment_GetStringValue(attachmentPtr, key, ptr, bytes);
    const value = this.pdfiumModule.pdfium.UTF16ToString(ptr);
    this.memoryManager.free(ptr);

    return value || undefined;
  }

  /**
   * 获取附件的数值属性（如 /Size 字段表示文件大小）
   *
   * @returns number or `null` when the key is absent
   *
   * @private
   */
  private getAttachmentNumber(attachmentPtr: number, key: string): number | undefined {
    const outPtr = this.memoryManager.malloc(4); // int32 输出缓冲区
    try {
      const ok = this.pdfiumModule.EPDFAttachment_GetIntegerValue(
        attachmentPtr,
        key,
        outPtr,
      );
      if (!ok) return undefined;
      // 作为无符号整数处理，避免大于 2GB 时出现负值（在 wasm 上罕见但无害）
      return this.pdfiumModule.pdfium.getValue(outPtr, 'i32') >>> 0;
    } finally {
      this.memoryManager.free(outPtr);
    }
  }

  /**
   * 获取注解的自定义数据（/EPDFCustom 字段）
   *
   * 自定义数据以 JSON 字符串形式存储在 PDF 注解字典中，
   * 本方法读取后解析为 JavaScript 对象。
   * @param annotationPtr - pointer to pdf annotation
   * @returns custom data of the annotation
   *
   * @private
   */
  private getAnnotCustom(annotationPtr: number): any {
    const custom = this.getAnnotString(annotationPtr, 'EPDFCustom');
    if (!custom) return;

    try {
      return JSON.parse(custom);
    } catch (error) {
      console.warn('Failed to parse annotation custom data as JSON:', error);
      console.warn('Invalid JSON string:', custom);
      return undefined;
    }
  }

  /**
   * 设置注解的自定义数据（/EPDFCustom 字段）
   *
   * 将 JavaScript 对象安全序列化为 JSON 字符串后写入 PDF 注解字典。
   * 传入 null 或 undefined 时清除自定义数据。
   * @private
   */
  private setAnnotCustom(annotationPtr: number, data: any): boolean {
    if (data === undefined || data === null) {
      // 通过设置空字符串来清除自定义数据
      return this.setAnnotString(annotationPtr, 'EPDFCustom', '');
    }

    try {
      const jsonString = JSON.stringify(data);
      return this.setAnnotString(annotationPtr, 'EPDFCustom', jsonString);
    } catch (error) {
      console.warn('Failed to stringify annotation custom data as JSON:', error);
      console.warn('Invalid data object:', data);
      return false;
    }
  }

  /**
   * 获取注解的意图名称（/IT 字段）
   *
   * 意图名称描述注解的预期用途，例如：
   *   - "FreeTextTypewriter"：自由文本打字机模式
   *   - "FreeTextCallout"：自由文本标注线模式
   *   - "PolygonCloud"：多边形云状标记
   *   - "InkList"：墨迹手写列表
   *
   * 采用与 getAnnotString 相同的两次调用模式：先探测长度，再复制内容。
   */
  private getAnnotIntent(annotationPtr: number): string | undefined {
    const len = this.pdfiumModule.EPDFAnnot_GetIntent(annotationPtr, 0, 0);
    if (len === 0) return;

    const codeUnits = len + 1;
    const bytes = codeUnits * 2;
    const ptr = this.memoryManager.malloc(bytes);

    this.pdfiumModule.EPDFAnnot_GetIntent(annotationPtr, ptr, bytes);
    const value = this.pdfiumModule.pdfium.UTF16ToString(ptr);

    this.memoryManager.free(ptr);

    return value && value !== 'undefined' ? value : undefined;
  }

  /**
   * Write the `/IT` (Intent) name into an annotation dictionary.
   *
   * Mirrors EPDFAnnot_SetIntent in PDFium (expects a UTF‑8 FPDF_BYTESTRING).
   *
   * @param annotationPtr Pointer returned by FPDFPage_GetAnnot
   * @param intent        Name without leading slash, e.g. `"PolygonCloud"`
   *                      A leading “/” will be stripped for convenience.
   * @returns             true on success, false otherwise
   */
  private setAnnotIntent(annotationPtr: number, intent: string): boolean {
    return this.pdfiumModule.EPDFAnnot_SetIntent(annotationPtr, intent);
  }

  /**
   * Returns the rich‑content string stored in the annotation’s `/RC` entry.
   *
   * Works like `getAnnotIntent()`: first probe for length, then copy.
   * `undefined` when the annotation has no rich content.
   */
  private getAnnotRichContent(annotationPtr: number): string | undefined {
    // 第一次调用：获取 UTF-16 编码单元数（不含 NUL 终止符）
    const len = this.pdfiumModule.EPDFAnnot_GetRichContent(annotationPtr, 0, 0);
    if (len === 0) return;

    // +1 为 PDFium 隐式添加的 NUL 终止符，bytes = 2 * code_units
    const codeUnits = len + 1;
    const bytes = codeUnits * 2;
    const ptr = this.memoryManager.malloc(bytes);

    this.pdfiumModule.EPDFAnnot_GetRichContent(annotationPtr, ptr, bytes);
    const value = this.pdfiumModule.pdfium.UTF16ToString(ptr);

    this.memoryManager.free(ptr);

    return value || undefined;
  }

  /**
   * 通过名称（NM 字段值）在页面中查找注解
   * @param pagePtr - pointer to pdf page object
   * @param name - name of annotation
   * @returns pointer to pdf annotation
   *
   * @private
   */
  private getAnnotationByName(pagePtr: number, name: string): number | undefined {
    return this.withWString(name, (wNamePtr) => {
      return this.pdfiumModule.EPDFPage_GetAnnotByName(pagePtr, wNamePtr);
    });
  }

  /**
   * 通过名称（NM 字段值）从页面中删除注解
   * @param pagePtr - pointer to pdf page object
   * @param name - name of annotation
   * @returns true on success
   *
   * @private
   */
  private removeAnnotationByName(pagePtr: number, name: string): boolean {
    return this.withWString(name, (wNamePtr) => {
      return this.pdfiumModule.EPDFPage_RemoveAnnotByName(pagePtr, wNamePtr);
    });
  }

  /**
   * 设置注解的字符串属性值（如 /T、/M、/State 等）
   *
   * @returns `true` if the operation was successful
   *
   * @private
   */
  private setAnnotString(annotationPtr: number, key: string, value: string): boolean {
    return this.withWString(value, (wValPtr) => {
      return this.pdfiumModule.FPDFAnnot_SetStringValue(annotationPtr, key, wValPtr);
    });
  }

  /**
   * 设置附件的字符串属性值
   *
   * @returns `true` if the operation was successful
   *
   * @private
   */
  private setAttachmentString(attachmentPtr: number, key: string, value: string): boolean {
    return this.withWString(value, (wValPtr) => {
      // FPDFAttachment_SetStringValue 写入到附件的 /Params 字典中
      return this.pdfiumModule.FPDFAttachment_SetStringValue(attachmentPtr, key, wValPtr);
    });
  }

  /**
   * 读取注解的顶点坐标列表
   *
   * 用于 Polygon 和 Polyline 注解，从 PDFium 中读取 /Vertices 数组，
   * 并从页面坐标转换为设备坐标。连续重复的点会被自动去重。
   * @param doc - pdf document object
   * @param page  - pdf page infor
   * @param annotationPtr - pointer to pdf annotation
   * @returns vertices of pdf annotation
   *
   * @private
   */
  private readPdfAnnoVertices(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
  ): Position[] {
    const vertices: Position[] = [];
    const count = this.pdfiumModule.FPDFAnnot_GetVertices(annotationPtr, 0, 0);
    const pointMemorySize = 8;
    const pointsPtr = this.memoryManager.malloc(count * pointMemorySize);
    this.pdfiumModule.FPDFAnnot_GetVertices(annotationPtr, pointsPtr, count);
    for (let i = 0; i < count; i++) {
      const pointX = this.pdfiumModule.pdfium.getValue(pointsPtr + i * pointMemorySize, 'float');
      const pointY = this.pdfiumModule.pdfium.getValue(
        pointsPtr + i * pointMemorySize + 4,
        'float',
      );

      const { x, y } = this.convertPagePointToDevicePoint(doc, page, {
        x: pointX,
        y: pointY,
      });
      const last = vertices[vertices.length - 1];
      if (!last || last.x !== x || last.y !== y) {
        vertices.push({ x, y });
      }
    }
    this.memoryManager.free(pointsPtr);

    return vertices;
  }

  /**
   * 设置多边形或折线注解的顶点坐标列表
   *
   * 将设备坐标转换为页面坐标后写入 PDFium。
   * 使用 FS_POINTF 结构体数组（每点 8 字节：float x + float y）。
   *
   * @param doc - pdf document object
   * @param page  - pdf page infor
   * @param annotPtr - pointer to pdf annotation
   * @param vertices - the vertices to be set
   * @returns true on success
   *
   * @private
   */
  private setPdfAnnoVertices(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotPtr: number,
    vertices: Position[],
  ): boolean {
    const pdf = this.pdfiumModule.pdfium;
    const FS_POINTF_SIZE = 8;

    const buf = this.memoryManager.malloc(FS_POINTF_SIZE * vertices.length);
    vertices.forEach((v, i) => {
      const pagePt = this.convertDevicePointToPagePoint(doc, page, v);
      pdf.setValue(buf + i * FS_POINTF_SIZE + 0, pagePt.x, 'float');
      pdf.setValue(buf + i * FS_POINTF_SIZE + 4, pagePt.y, 'float');
    });

    const ok = this.pdfiumModule.EPDFAnnot_SetVertices(annotPtr, buf, vertices.length);
    this.memoryManager.free(buf);
    return ok;
  }

  /**
   * 读取自由文本注解的标注线端点（/CL 数组）
   *
   * 标注线是从自由文本框延伸到页面某处的指示线，
   * 由 2 个点（起点+终点）或 3 个点（起点+折点+终点）组成。
   * 返回设备坐标系中的点数组，无标注线时返回 undefined。
   *
   * @private
   */
  private getCalloutLine(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotationPtr: number,
  ): Position[] | undefined {
    const count = this.pdfiumModule.EPDFAnnot_GetCalloutLineCount(annotationPtr);
    if (count === 0) {
      return undefined;
    }

    const FS_POINTF_SIZE = 8;
    const pointsPtr = this.memoryManager.malloc(count * FS_POINTF_SIZE);
    const result = this.pdfiumModule.EPDFAnnot_GetCalloutLine(annotationPtr, pointsPtr, count);
    if (result === 0) {
      this.memoryManager.free(pointsPtr);
      return undefined;
    }

    const points: Position[] = [];
    for (let i = 0; i < count; i++) {
      const px = this.pdfiumModule.pdfium.getValue(pointsPtr + i * FS_POINTF_SIZE, 'float');
      const py = this.pdfiumModule.pdfium.getValue(pointsPtr + i * FS_POINTF_SIZE + 4, 'float');
      points.push(this.convertPagePointToDevicePoint(doc, page, { x: px, y: py }));
    }
    this.memoryManager.free(pointsPtr);
    return points;
  }

  /**
   * 设置自由文本注解的标注线端点（/CL 数组）
   *
   * 将设备坐标转换为页面坐标后写入 PDF 注解字典。
   *
   * @private
   */
  private setCalloutLine(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotPtr: number,
    points: Position[],
  ): boolean {
    const pdf = this.pdfiumModule.pdfium;
    const FS_POINTF_SIZE = 8;

    const buf = this.memoryManager.malloc(FS_POINTF_SIZE * points.length);
    points.forEach((v, i) => {
      const pagePt = this.convertDevicePointToPagePoint(doc, page, v);
      pdf.setValue(buf + i * FS_POINTF_SIZE + 0, pagePt.x, 'float');
      pdf.setValue(buf + i * FS_POINTF_SIZE + 4, pagePt.y, 'float');
    });

    const ok = this.pdfiumModule.EPDFAnnot_SetCalloutLine(annotPtr, buf, points.length);
    this.memoryManager.free(buf);
    return ok;
  }

  /**
   * 读取 PDF 书签的跳转目标
   *
   * 书签目标可以是两种类型之一：
   *   - Action（/A 字段）：执行某种动作（跳转、打开 URI、启动应用等）
   *   - Destination（/Dest 字段）：跳转到文档内的特定页面位置
   * @param docPtr - pointer to pdf document object
   * @param getActionPtr - callback function to retrive the pointer of action
   * @param getDestinationPtr - callback function to retrive the pointer of destination
   * @returns target of pdf bookmark
   *
   * @private
   */
  private readPdfBookmarkTarget(
    docPtr: number,
    getActionPtr: () => number,
    getDestinationPtr: () => number,
  ): PdfLinkTarget | undefined {
    const actionPtr = getActionPtr();
    if (actionPtr) {
      const action = this.readPdfAction(docPtr, actionPtr);

      return {
        type: 'action',
        action,
      };
    } else {
      const destinationPtr = getDestinationPtr();
      if (destinationPtr) {
        const destination = this.readPdfDestination(docPtr, destinationPtr);

        return {
          type: 'destination',
          destination,
        };
      }
    }
  }

  /**
   * 读取 PDF 表单控件的字段信息
   *
   * 字段信息是表单系统的核心数据，包括：
   *   - 字段标志位（flag）：ReadOnly、Required、NoExport 等
   *   - 字段类型（type）：文本框、复选框、单选按钮、下拉框、列表框、按钮、签名域
   *   - 字段名称（name）：表单提交时使用的字段标识
   *   - 备用名称（alternateName）：用于显示的可读名称
   *   - 字段值（value）：当前选中/输入的值
   *   - 字段对象号（fieldObjectId）：PDF 对象编号，用于跨控件字段关联
   *
   * 根据字段类型还可能包含额外信息：
   *   - TextField：最大长度（MaxLen）
   *   - RadioButton/ComboBox/ListBox：选项列表（options）
   * @param formHandle - form handle
   * @param annotationPtr - pointer to pdf annotation
   * @returns field of pdf widget annotation
   *
   * @private
   */
  private readPdfWidgetAnnoField(formHandle: number, annotationPtr: number): PdfWidgetAnnoField {
    const flag = this.pdfiumModule.FPDFAnnot_GetFormFieldFlags(
      formHandle,
      annotationPtr,
    ) as PDF_FORM_FIELD_FLAG;

    const type = this.pdfiumModule.FPDFAnnot_GetFormFieldType(
      formHandle,
      annotationPtr,
    ) as PDF_FORM_FIELD_TYPE;

    const name = readString(
      this.pdfiumModule.pdfium,
      (buffer: number, bufferLength) => {
        return this.pdfiumModule.FPDFAnnot_GetFormFieldName(
          formHandle,
          annotationPtr,
          buffer,
          bufferLength,
        );
      },
      this.pdfiumModule.pdfium.UTF16ToString,
    );

    const alternateName = readString(
      this.pdfiumModule.pdfium,
      (buffer: number, bufferLength) => {
        return this.pdfiumModule.FPDFAnnot_GetFormFieldAlternateName(
          formHandle,
          annotationPtr,
          buffer,
          bufferLength,
        );
      },
      this.pdfiumModule.pdfium.UTF16ToString,
    );

    const value = readString(
      this.pdfiumModule.pdfium,
      (buffer: number, bufferLength) => {
        return this.pdfiumModule.EPDFAnnot_GetFormFieldRawValue(
          formHandle,
          annotationPtr,
          buffer,
          bufferLength,
        );
      },
      this.pdfiumModule.pdfium.UTF16ToString,
    );

    const fieldObjectId = this.pdfiumModule.EPDFAnnot_GetFormFieldObjectNumber(
      formHandle,
      annotationPtr,
    );

    const base = {
      flag,
      name,
      alternateName,
      value,
      fieldObjectId: fieldObjectId > 0 ? fieldObjectId : undefined,
    };

    switch (type) {
      case PDF_FORM_FIELD_TYPE.TEXTFIELD: {
        let maxLen: number | undefined;
        const floatPtr = this.memoryManager.malloc(4);
        const ok = this.pdfiumModule.FPDFAnnot_GetNumberValue(annotationPtr, 'MaxLen', floatPtr);
        if (ok) {
          maxLen = this.pdfiumModule.pdfium.getValue(floatPtr, 'float');
        }
        this.memoryManager.free(floatPtr);
        return { ...base, type, maxLen };
      }

      case PDF_FORM_FIELD_TYPE.CHECKBOX:
        return { ...base, type };

      case PDF_FORM_FIELD_TYPE.RADIOBUTTON:
        return {
          ...base,
          type,
          options: this.readWidgetOptions(formHandle, annotationPtr),
        };

      case PDF_FORM_FIELD_TYPE.COMBOBOX:
        return { ...base, type, options: this.readWidgetOptions(formHandle, annotationPtr) };

      case PDF_FORM_FIELD_TYPE.LISTBOX:
        return { ...base, type, options: this.readWidgetOptions(formHandle, annotationPtr) };

      case PDF_FORM_FIELD_TYPE.PUSHBUTTON:
        return { ...base, type };

      case PDF_FORM_FIELD_TYPE.SIGNATURE:
        return { ...base, type };

      default:
        return { ...base, type: type as PdfUnknownWidgetAnnoField['type'] };
    }
  }

  /**
   * 读取表单控件（下拉框/列表框/单选按钮）的选项列表
   *
   * 每个选项包含：
   *   - label：选项的显示文本
   *   - isSelected：该选项是否处于选中状态
   *
   * @param formHandle - 表单环境句柄
   * @param annotationPtr - PDFium 注解指针
   * @returns 选项数组
   *
   * @private
   */
  private readWidgetOptions(formHandle: number, annotationPtr: number): PdfWidgetAnnoOption[] {
    const options: PdfWidgetAnnoOption[] = [];
    const count = this.pdfiumModule.FPDFAnnot_GetOptionCount(formHandle, annotationPtr);
    for (let i = 0; i < count; i++) {
      const label = readString(
        this.pdfiumModule.pdfium,
        (buffer: number, bufferLength) => {
          return this.pdfiumModule.FPDFAnnot_GetOptionLabel(
            formHandle,
            annotationPtr,
            i,
            buffer,
            bufferLength,
          );
        },
        this.pdfiumModule.pdfium.UTF16ToString,
      );
      const isSelected = this.pdfiumModule.FPDFAnnot_IsOptionSelected(formHandle, annotationPtr, i);
      options.push({ label, isSelected });
    }
    return options;
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.renderAnnotation}
   *
   * @public
   */
  renderPageAnnotationRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotation: PdfAnnotationObject,
    options?: PdfRenderPageAnnotationOptions,
  ): PdfTask<ImageDataLike> {
    const {
      scaleFactor = 1,
      rotation = Rotation.Degree0,
      dpr = 1,
      mode = AppearanceMode.Normal,
    } = options ?? {};

    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      'renderPageAnnotation',
      doc,
      page,
      annotation,
      options,
    );
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RenderPageAnnotation`,
      'Begin',
      `${doc.id}-${page.index}-${annotation.id}`,
    );

    const task = new Task<ImageDataLike, PdfErrorReason>();
    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `RenderPageAnnotation`,
        'End',
        `${doc.id}-${page.index}-${annotation.id}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // 1) 获取原生句柄
    const pageCtx = ctx.acquirePage(page.index);
    const annotPtr = this.getAnnotationByName(pageCtx.pagePtr, annotation.id);
    if (!annotPtr) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `RenderPageAnnotation`,
        'End',
        `${doc.id}-${page.index}-${annotation.id}`,
      );
      pageCtx.release();
      return PdfTaskHelper.reject({ code: PdfErrorCode.NotFound, message: 'annotation not found' });
    }

    // 1b) 预检查：该注解是否有可渲染的外观流？
    let hasAP = !!this.pdfiumModule.EPDFAnnot_HasAppearanceStream(annotPtr, mode);
    if (!hasAP && annotation.type === PdfAnnotationSubtype.WIDGET) {
      if (!this.pdfiumModule.FPDFAnnot_HasKey(annotPtr, 'AP')) {
        this.pdfiumModule.EPDFAnnot_GenerateFormFieldAP(annotPtr);
        hasAP = !!this.pdfiumModule.EPDFAnnot_HasAppearanceStream(annotPtr, mode);
      }
    }
    if (!hasAP) {
      this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);
      pageCtx.release();
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `RenderPageAnnotation`,
        'End',
        `${doc.id}-${page.index}-${annotation.id}`,
      );
      task.resolve({ data: new Uint8ClampedArray(4), width: 1, height: 1 });
      return task;
    }

    // 2) 计算设备尺寸（考虑旋转），取整像素
    const finalScale = Math.max(0.01, scaleFactor * dpr);

    // 当调用者启用 unrotated 选项且存在未旋转矩形时，使用未旋转矩形
    // 作为位图尺寸和未旋转 WASM 渲染路径（CSS 负责旋转显示）。
    const unrotated = !!options?.unrotated && !!annotation.unrotatedRect;
    const renderRect = unrotated ? annotation.unrotatedRect! : annotation.rect;

    const devRect = toIntRect(transformRect(page.size, renderRect, rotation, finalScale));

    const wDev = Math.max(1, devRect.size.width);
    const hDev = Math.max(1, devRect.size.height);
    const stride = wDev * 4;
    const bytes = stride * hDev;

    // 3) 在 WASM 内存中分配位图缓冲区
    const heapPtr = this.memoryManager.malloc(bytes);
    if (heapPtr === 0) {
      this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);
      pageCtx.release();
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: 'Failed to allocate WASM memory for annotation bitmap',
      });
    }
    const bitmapPtr = this.pdfiumModule.FPDFBitmap_CreateEx(
      wDev,
      hDev,
      BitmapFormat.Bitmap_BGRA,
      heapPtr,
      stride,
    );
    if (!bitmapPtr) {
      this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);
      pageCtx.release();
      this.memoryManager.free(heapPtr);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: 'Failed to create PDFium bitmap for annotation',
      });
    }
    this.pdfiumModule.FPDFBitmap_FillRect(bitmapPtr, 0, 0, wDev, hDev, 0x00000000);

    // 4) 用户矩阵（无 Y 翻转；包含 -origin 平移）
    const M = buildUserToDeviceMatrix(renderRect, rotation, wDev, hDev);
    const mPtr = this.memoryManager.malloc(6 * 4);
    const mView = new Float32Array(this.pdfiumModule.pdfium.HEAPF32.buffer, mPtr, 6);
    mView.set([M.a, M.b, M.c, M.d, M.e, M.f]);

    // 5) 执行渲染
    // 渲染标志：反转字节顺序（BGRA -> RGBA）
    const FLAGS = RenderFlag.REVERSE_BYTE_ORDER;
    let ok = false;
    try {
      if (unrotated) {
        // 使用未旋转渲染路径：忽略 AP 矩阵，使用
        // 使用 EPDFUnrotatedRect 进行 MatchRect 匹配——不修改注解状态
        ok = !!this.pdfiumModule.EPDF_RenderAnnotBitmapUnrotated(
          bitmapPtr,
          pageCtx.pagePtr,
          annotPtr,
          mode,
          mPtr,
          FLAGS,
        );
      } else {
        ok = !!this.pdfiumModule.EPDF_RenderAnnotBitmap(
          bitmapPtr,
          pageCtx.pagePtr,
          annotPtr,
          mode,
          mPtr,
          FLAGS,
        );
      }
    } finally {
      this.memoryManager.free(mPtr);
      this.pdfiumModule.FPDFBitmap_Destroy(bitmapPtr);
      this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);
      pageCtx.release();
      this.memoryManager.free(heapPtr);
    }

    if (!ok) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        `RenderPageAnnotation`,
        'End',
        `${doc.id}-${page.index}-${annotation.id}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: 'EPDF_RenderAnnotBitmap failed',
      });
    }

    const data = this.pdfiumModule.pdfium.HEAPU8.subarray(heapPtr, heapPtr + bytes);
    // Return plain object (ImageDataLike) instead of browser-specific ImageData
    const imageDataLike: ImageDataLike = {
      data: new Uint8ClampedArray(data),
      width: wDev,
      height: hDev,
    };
    task.resolve(imageDataLike);
    return task;
  }

  /**
   * 批量渲染页面中所有注解的外观流（Appearance Stream）
   *
   * 在一次调用中渲染页面中所有注解的外观，返回注解 ID -> 渲染结果的映射。
   * 支持 Normal、Rollover、Down 三种外观模式的渲染。
   *
   * 跳过条件：
   *   - 包含旋转 + 未旋转矩形的 EmbedPDF 旋转注解
   *   - 没有任何外观流的注解
   * Returns a map of annotation ID -> rendered appearances (Normal/Rollover/Down).
   * Skips annotations that have rotation + unrotatedRect (EmbedPDF-rotated)
   * and annotations without any appearance stream.
   *
   * @public
   */
  renderPageAnnotationsRaw(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    options?: PdfRenderPageAnnotationOptions,
  ): PdfTask<AnnotationAppearanceMap> {
    const { scaleFactor = 1, rotation = Rotation.Degree0, dpr = 1 } = options ?? {};

    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'renderPageAnnotationsRaw', doc, page, options);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      'RenderPageAnnotationsRaw',
      'Begin',
      `${doc.id}-${page.index}`,
    );

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(
        LOG_SOURCE,
        LOG_CATEGORY,
        'RenderPageAnnotationsRaw',
        'End',
        `${doc.id}-${page.index}`,
      );
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    const pageCtx = ctx.acquirePage(page.index);
    const result: AnnotationAppearanceMap = {};
    const finalScale = Math.max(0.01, scaleFactor * dpr);
    const annotCount = this.pdfiumModule.FPDFPage_GetAnnotCount(pageCtx.pagePtr);

    for (let i = 0; i < annotCount; i++) {
      const annotPtr = this.pdfiumModule.FPDFPage_GetAnnot(pageCtx.pagePtr, i);
      if (!annotPtr) continue;

      try {
        // 读取注解的 NM（唯一标识）
        const nm = this.getAnnotString(annotPtr, 'NM');
        if (!nm) continue;

        // 跳过 EmbedPDF 旋转注解（包含旋转 + 未旋转矩形信息的）
        const extRotation = this.getAnnotExtendedRotation(annotPtr);
        if (extRotation !== 0) {
          const unrotatedRaw = this.readAnnotUnrotatedRect(annotPtr);
          if (unrotatedRaw) continue;
        }

        // 检测可用的外观模式
        const apModes = this.pdfiumModule.EPDFAnnot_GetAvailableAppearanceModes(annotPtr);
        if (!apModes) continue;

        const appearances: AnnotationAppearances = {};

        // 渲染每种可用的外观模式
        const modesToRender: Array<{
          bit: number;
          mode: AppearanceMode;
          key: keyof AnnotationAppearances;
        }> = [
          { bit: AP_MODE_NORMAL, mode: AppearanceMode.Normal, key: 'normal' },
          { bit: AP_MODE_ROLLOVER, mode: AppearanceMode.Rollover, key: 'rollover' },
          { bit: AP_MODE_DOWN, mode: AppearanceMode.Down, key: 'down' },
        ];

        for (const { bit, mode, key } of modesToRender) {
          if (!(apModes & bit)) continue;

          const rendered = this.renderSingleAnnotAppearance(
            doc,
            page,
            pageCtx,
            annotPtr,
            mode,
            rotation,
            finalScale,
          );
          if (rendered) {
            appearances[key] = rendered;
          }
        }

        if (appearances.normal || appearances.rollover || appearances.down) {
          result[nm] = appearances;
        }
      } finally {
        this.pdfiumModule.FPDFPage_CloseAnnot(annotPtr);
      }
    }

    pageCtx.release();
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      'RenderPageAnnotationsRaw',
      'End',
      `${doc.id}-${page.index}`,
    );

    const task = new Task<AnnotationAppearanceMap, PdfErrorReason>();
    task.resolve(result);
    return task;
  }

  /**
   * 渲染单个注解的指定外观模式
   *
   * 对于 Widget 类型注解，如果没有 AP（外观流）字典，会尝试自动生成。
   * 渲染完成后返回图像数据和矩形区域，失败时返回 null。
   * @private
   */
  private renderSingleAnnotAppearance(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pageCtx: PageContext,
    annotPtr: number,
    mode: AppearanceMode,
    rotation: Rotation,
    finalScale: number,
  ): AnnotationAppearanceImage | null {
    if (!this.pdfiumModule.EPDFAnnot_HasAppearanceStream(annotPtr, mode)) {
      const subtype = this.pdfiumModule.FPDFAnnot_GetSubtype(annotPtr);
      if (
        subtype === PdfAnnotationSubtype.WIDGET &&
        !this.pdfiumModule.FPDFAnnot_HasKey(annotPtr, 'AP')
      ) {
        this.pdfiumModule.EPDFAnnot_GenerateFormFieldAP(annotPtr);
        if (!this.pdfiumModule.EPDFAnnot_HasAppearanceStream(annotPtr, mode)) {
          return null;
        }
      } else {
        return null;
      }
    }

    // 使用 EPDFAnnot_GetRect 读取规范化矩形，然后转换为设备坐标
    const pageRect = this.readPageAnnoRect(annotPtr);
    const annotRect = this.convertPageRectToDeviceRect(doc, page, pageRect);

    const devRect = toIntRect(transformRect(page.size, annotRect, rotation, finalScale));
    const wDev = Math.max(1, devRect.size.width);
    const hDev = Math.max(1, devRect.size.height);
    const stride = wDev * 4;
    const bytes = stride * hDev;

    const heapPtr = this.memoryManager.malloc(bytes);
    const bitmapPtr = this.pdfiumModule.FPDFBitmap_CreateEx(
      wDev,
      hDev,
      BitmapFormat.Bitmap_BGRA,
      heapPtr,
      stride,
    );
    this.pdfiumModule.FPDFBitmap_FillRect(bitmapPtr, 0, 0, wDev, hDev, 0x00000000);

    const M = buildUserToDeviceMatrix(annotRect, rotation, wDev, hDev);
    const mPtr = this.memoryManager.malloc(6 * 4);
    const mView = new Float32Array(this.pdfiumModule.pdfium.HEAPF32.buffer, mPtr, 6);
    mView.set([M.a, M.b, M.c, M.d, M.e, M.f]);

    const FLAGS = RenderFlag.REVERSE_BYTE_ORDER;
    let ok = false;
    try {
      ok = !!this.pdfiumModule.EPDF_RenderAnnotBitmap(
        bitmapPtr,
        pageCtx.pagePtr,
        annotPtr,
        mode,
        mPtr,
        FLAGS,
      );
    } finally {
      this.memoryManager.free(mPtr);
      this.pdfiumModule.FPDFBitmap_Destroy(bitmapPtr);
    }

    if (!ok) {
      this.memoryManager.free(heapPtr);
      return null;
    }

    const data = this.pdfiumModule.pdfium.HEAPU8.subarray(heapPtr, heapPtr + bytes);
    const imageData: ImageDataLike = {
      data: new Uint8ClampedArray(data),
      width: wDev,
      height: hDev,
    };
    this.memoryManager.free(heapPtr);

    return { data: imageData, rect: annotRect };
  }

  private renderRectEncoded(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    rect: Rect,
    options?: PdfRenderPageOptions,
  ): PdfTask<ImageDataLike> {
    const task = new Task<ImageDataLike, PdfErrorReason>();
    const rotation: Rotation = options?.rotation ?? Rotation.Degree0;

    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'document does not open',
      });
    }

    // ---- 1) 计算设备尺寸（缩放因子 x DPR，90/270 度时交换宽高）
    const scale = Math.max(0.01, options?.scaleFactor ?? 1);
    const dpr = Math.max(1, options?.dpr ?? 1);
    const finalScale = scale * dpr;

    const baseW = rect.size.width;
    const baseH = rect.size.height;
    const swap = (rotation & 1) === 1; // 90 or 270

    const wDev = Math.max(1, Math.round((swap ? baseH : baseW) * finalScale));
    const hDev = Math.max(1, Math.round((swap ? baseW : baseH) * finalScale));
    const stride = wDev * 4;
    const bytes = stride * hDev;

    const pageCtx = ctx.acquirePage(page.index);
    const shouldRenderForms = options?.withForms ?? false;

    // ---- 2) 在 WASM 内存中分配 BGRA 位图
    const heapPtr = this.memoryManager.malloc(bytes);
    if (heapPtr === 0) {
      pageCtx.release();
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: 'Failed to allocate WASM memory for bitmap',
      });
    }
    const bitmapPtr = this.pdfiumModule.FPDFBitmap_CreateEx(
      wDev,
      hDev,
      BitmapFormat.Bitmap_BGRA,
      heapPtr,
      stride,
    );
    if (!bitmapPtr) {
      pageCtx.release();
      this.memoryManager.free(heapPtr);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: 'Failed to create PDFium bitmap',
      });
    }
    const bgColor = options?.transparentBackground ? 0x00000000 : 0xffffffff;
    this.pdfiumModule.FPDFBitmap_FillRect(bitmapPtr, 0, 0, wDev, hDev, bgColor);

    const M = buildUserToDeviceMatrix(rect, rotation, wDev, hDev);

    const mPtr = this.memoryManager.malloc(6 * 4); // FS_MATRIX
    const mView = new Float32Array(this.pdfiumModule.pdfium.HEAPF32.buffer, mPtr, 6);
    mView.set([M.a, M.b, M.c, M.d, M.e, M.f]);

    // 裁剪区域设为整个位图（设备空间）
    const clipPtr = this.memoryManager.malloc(4 * 4); // FS_RECTF {left,bottom,right,top}
    const clipView = new Float32Array(this.pdfiumModule.pdfium.HEAPF32.buffer, clipPtr, 4);
    clipView.set([0, 0, wDev, hDev]);

    // 渲染标志：交换字节顺序以向 JS 提供 RGBA；根据选项包含注解渲染
    let flags = RenderFlag.REVERSE_BYTE_ORDER;
    if (options?.withAnnotations ?? false) flags |= RenderFlag.ANNOT;

    try {
      this.pdfiumModule.FPDF_RenderPageBitmapWithMatrix(
        bitmapPtr,
        pageCtx.pagePtr,
        mPtr,
        clipPtr,
        flags,
      );

      if (shouldRenderForms) {
        pageCtx.withFormHandle((formHandle) => {
          const formParams = computeFormDrawParams(M, rect, page.size, rotation);
          const { startX, startY, formsWidth, formsHeight } = formParams;

          this.pdfiumModule.FPDF_FFLDraw(
            formHandle,
            bitmapPtr,
            pageCtx.pagePtr,
            startX,
            startY,
            formsWidth,
            formsHeight,
            rotation,
            flags,
          );
        });
      }
    } finally {
      pageCtx.release();
      this.memoryManager.free(mPtr);
      this.memoryManager.free(clipPtr);
      this.pdfiumModule.FPDFBitmap_Destroy(bitmapPtr);
      this.memoryManager.free(heapPtr);
    }

    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RenderRectEncodedData`,
      'Begin',
      `${doc.id}-${page.index}`,
    );
    const data = this.pdfiumModule.pdfium.HEAPU8.subarray(heapPtr, heapPtr + bytes);
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RenderRectEncodedData`,
      'End',
      `${doc.id}-${page.index}`,
    );

    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RenderRectEncodedImageData`,
      'Begin',
      `${doc.id}-${page.index}`,
    );
    // Return plain object (ImageDataLike) instead of browser-specific ImageData
    // 这确保了在 Node.js 和其他非浏览器环境中的兼容性
    const imageDataLike: ImageDataLike = {
      data: new Uint8ClampedArray(data),
      width: wDev,
      height: hDev,
    };
    this.logger.perf(
      LOG_SOURCE,
      LOG_CATEGORY,
      `RenderRectEncodedImageData`,
      'End',
      `${doc.id}-${page.index}`,
    );
    task.resolve(imageDataLike);

    return task;
  }

  /**
   * 读取 PDF 链接注解的跳转目标
   *
   * 与 readPdfBookmarkTarget 的区别在于优先级：
   *   - 链接注解：优先检查 Destination（/Dest），其次检查 Action（/A）
   *   - 书签：优先检查 Action（/A），其次检查 Destination（/Dest）
   * @param docPtr - pointer to pdf document object
   * @param getActionPtr - callback function to retrive the pointer of action
   * @param getDestinationPtr - callback function to retrive the pointer of destination
   * @returns target of link
   *
   * @private
   */
  private readPdfLinkAnnoTarget(
    docPtr: number,
    getActionPtr: () => number,
    getDestinationPtr: () => number,
  ): PdfLinkTarget | undefined {
    const destinationPtr = getDestinationPtr();
    if (destinationPtr) {
      const destination = this.readPdfDestination(docPtr, destinationPtr);

      return {
        type: 'destination',
        destination,
      };
    } else {
      const actionPtr = getActionPtr();
      if (actionPtr) {
        const action = this.readPdfAction(docPtr, actionPtr);

        return {
          type: 'action',
          action,
        };
      }
    }
  }

  /**
   * 创建本地目标位置指针（FPDF_DEST）
   *
   * 根据目标对象的信息创建 PDFium 的目标指针，用于设置书签和链接的跳转目标。
   * 支持 XYZ（精确定位）、FitPage、FitHorizontal、FitVertical、FitRectangle 等缩放模式。
   *
   * 注意：本方法会临时加载目标页面以创建目标指针，使用完毕后会关闭。
   *
   * @param docPtr - PDFium 文档指针
   * @param dest - 目标位置对象
   * @returns 目标指针，失败时返回 0
   *
   * @private
   */
  private createLocalDestPtr(docPtr: number, dest: PdfDestinationObject): number {
    // 加载目标页面，用于创建本地目标
    const pagePtr = this.pdfiumModule.FPDF_LoadPage(docPtr, dest.pageIndex);
    if (!pagePtr) return 0;

    try {
      if (dest.zoom.mode === PdfZoomMode.XYZ) {
        const { x, y, zoom } = dest.zoom.params;
        // 将传入的 x/y/zoom 视为"已指定"的参数。
        return this.pdfiumModule.EPDFDest_CreateXYZ(
          pagePtr,
          /*has_left*/ true,
          x,
          /*has_top*/ true,
          y,
          /*has_zoom*/ true,
          zoom,
        );
      }

      // 将非 XYZ 模式的"视图模式"映射到 PDFDEST_VIEW_* 枚举和参数。
      let viewEnum: PdfZoomMode;
      let params: number[] = [];

      switch (dest.zoom.mode) {
        case PdfZoomMode.FitPage:
          viewEnum = PdfZoomMode.FitPage; // no params
          break;
        case PdfZoomMode.FitHorizontal:
          // FitH 需要 top 参数；使用 view[0]，未提供则为 0
          viewEnum = PdfZoomMode.FitHorizontal;
          params = [dest.view?.[0] ?? 0];
          break;
        case PdfZoomMode.FitVertical:
          // FitV 需要 left 参数；使用 view[0]，未提供则为 0
          viewEnum = PdfZoomMode.FitVertical;
          params = [dest.view?.[0] ?? 0];
          break;
        case PdfZoomMode.FitRectangle:
          // FitR 需要 left, bottom, right, top 四个参数（不足补零）。
          {
            const v = dest.view ?? [];
            params = [v[0] ?? 0, v[1] ?? 0, v[2] ?? 0, v[3] ?? 0];
            viewEnum = PdfZoomMode.FitRectangle;
          }
          break;
        case PdfZoomMode.Unknown:
        default:
          // Unknown 类型无法编码为有效的显式目标。
          return 0;
      }

      return this.withFloatArray(params, (ptr, count) =>
        this.pdfiumModule.EPDFDest_CreateView(pagePtr, viewEnum, ptr, count),
      );
    } finally {
      this.pdfiumModule.FPDF_ClosePage(pagePtr);
    }
  }

  /**
   * 将跳转目标应用到书签
   *
   * 支持 Destination 和 Action 两种类型的目标：
   *   - Destination：创建本地目标并设置到书签
   *   - Action：根据动作类型创建 GoTo/URI/Launch 动作并设置
   *
   * 注意：RemoteGoto（跳转到其他文档）暂不支持。
   *
   * @param docPtr - PDFium 文档指针
   * @param bmPtr - PDFium 书签指针
   * @param target - 跳转目标
   * @returns 是否设置成功
   *
   * @private
   */
  private applyBookmarkTarget(docPtr: number, bmPtr: number, target: PdfLinkTarget): boolean {
    if (target.type === 'destination') {
      const destPtr = this.createLocalDestPtr(docPtr, target.destination);
      if (!destPtr) return false;
      const ok = this.pdfiumModule.EPDFBookmark_SetDest(docPtr, bmPtr, destPtr);
      return !!ok;
    }

    // Action 类型目标
    const action = target.action;
    switch (action.type) {
      case PdfActionType.Goto: {
        // 内部跳转动作：创建 GoTo 动作并设置到书签
        const destPtr = this.createLocalDestPtr(docPtr, action.destination);
        if (!destPtr) return false;
        const actPtr = this.pdfiumModule.EPDFAction_CreateGoTo(docPtr, destPtr);
        if (!actPtr) return false;
        return !!this.pdfiumModule.EPDFBookmark_SetAction(docPtr, bmPtr, actPtr);
      }

      case PdfActionType.URI: { // URI 动作：打开一个 URL 链接
        const actPtr = this.pdfiumModule.EPDFAction_CreateURI(docPtr, action.uri);
        if (!actPtr) return false;
        return !!this.pdfiumModule.EPDFBookmark_SetAction(docPtr, bmPtr, actPtr);
      }

      case PdfActionType.LaunchAppOrOpenFile: { // 启动应用或打开文件
        const actPtr = this.withWString(action.path, (wptr) =>
          this.pdfiumModule.EPDFAction_CreateLaunch(docPtr, wptr),
        );
        if (!actPtr) return false;
        return !!this.pdfiumModule.EPDFBookmark_SetAction(docPtr, bmPtr, actPtr);
      }

      case PdfActionType.RemoteGoto:
        // 远程跳转需要文件路径来构建 GoToR 动作，当前 Action 结构不携带路径，暂不支持
        return false;

      case PdfActionType.Unsupported:
      default:
        return false;
    }
  }

  /**
   * 将跳转目标应用到链接注解
   *
   * 与 applyBookmarkTarget 类似，但操作对象为链接注解而非书签。
   * 支持 Destination（内部跳转）和 Action（GoTo/URI/Launch）。
   *
   * @param docPtr - PDFium 文档指针
   * @param annotationPtr - PDFium 链接注解指针
   * @param target - 跳转目标
   * @returns 是否设置成功
   *
   * @private
   */
  private applyLinkTarget(docPtr: number, annotationPtr: number, target: PdfLinkTarget): boolean {
    if (target.type === 'destination') {
      const destPtr = this.createLocalDestPtr(docPtr, target.destination);
      if (!destPtr) return false;
      const actPtr = this.pdfiumModule.EPDFAction_CreateGoTo(docPtr, destPtr);
      if (!actPtr) return false;
      return !!this.pdfiumModule.EPDFAnnot_SetAction(annotationPtr, actPtr);
    }

    // Action 类型目标
    const action = target.action;
    switch (action.type) {
      case PdfActionType.Goto: {
        // 内部跳转动作：创建 GoTo 动作并设置到注解
        const destPtr = this.createLocalDestPtr(docPtr, action.destination);
        if (!destPtr) return false;
        const actPtr = this.pdfiumModule.EPDFAction_CreateGoTo(docPtr, destPtr);
        if (!actPtr) return false;
        return !!this.pdfiumModule.EPDFAnnot_SetAction(annotationPtr, actPtr);
      }

      case PdfActionType.URI: {
        // URI 动作
        const actPtr = this.pdfiumModule.EPDFAction_CreateURI(docPtr, action.uri);
        if (!actPtr) return false;
        return !!this.pdfiumModule.EPDFAnnot_SetAction(annotationPtr, actPtr);
      }

      case PdfActionType.LaunchAppOrOpenFile: {
        // 启动应用或打开文件
        const actPtr = this.withWString(action.path, (wptr) =>
          this.pdfiumModule.EPDFAction_CreateLaunch(docPtr, wptr),
        );
        if (!actPtr) return false;
        return !!this.pdfiumModule.EPDFAnnot_SetAction(annotationPtr, actPtr);
      }

      case PdfActionType.RemoteGoto:
      case PdfActionType.Unsupported:
      default:
        return false;
    }
  }

  /**
   * 读取 PDF 动作对象
   *
   * PDF 动作定义了用户交互时的行为，常见类型包括：
   *   - Goto：跳转到文档内指定页面
   *   - RemoteGoto：跳转到其他文档的指定页面（当前转为 Unsupported）
   *   - URI：打开一个 URL 链接
   *   - LaunchAppOrOpenFile：启动应用程序或打开文件
   *   - Unsupported：不支持的动作类型
   * @param docPtr - pointer to pdf document object
   * @param actionPtr - pointer to pdf action object
   * @returns pdf action object
   *
   * @private
   */
  private readPdfAction(docPtr: number, actionPtr: number): PdfActionObject {
    // 获取动作类型枚举值
    const actionType = this.pdfiumModule.FPDFAction_GetType(actionPtr) as PdfActionType;
    let action: PdfActionObject;
    switch (actionType) {
      case PdfActionType.Unsupported:
        action = {
          type: PdfActionType.Unsupported,
        };
        break;
      case PdfActionType.Goto:
        {
          // 内部跳转动作：获取目标位置并读取目标信息
          const destinationPtr = this.pdfiumModule.FPDFAction_GetDest(docPtr, actionPtr);
          if (destinationPtr) {
            const destination = this.readPdfDestination(docPtr, destinationPtr);

            action = {
              type: PdfActionType.Goto,
              destination,
            };
          } else {
            action = {
              type: PdfActionType.Unsupported,
            };
          }
        }
        break;
      case PdfActionType.RemoteGoto:
        {
          // 远程跳转动作需要先获取目标文件路径，
          // 然后加载该文档，再使用新文档句柄调用。
          // 当前实现将其转为 Unsupported 类型
          action = {
            type: PdfActionType.Unsupported,
          };
        }
        break;
      case PdfActionType.URI:
        {
          const uri = readString(
            this.pdfiumModule.pdfium,
            (buffer, bufferLength) => {
              return this.pdfiumModule.FPDFAction_GetURIPath(
                docPtr,
                actionPtr,
                buffer,
                bufferLength,
              );
            },
            this.pdfiumModule.pdfium.UTF8ToString,
          );

          action = {
            type: PdfActionType.URI,
            uri,
          };
        }
        break;
      case PdfActionType.LaunchAppOrOpenFile:
        {
          const path = readString(
            this.pdfiumModule.pdfium,
            (buffer, bufferLength) => {
              return this.pdfiumModule.FPDFAction_GetFilePath(actionPtr, buffer, bufferLength);
            },
            this.pdfiumModule.pdfium.UTF8ToString,
          );
          action = {
            type: PdfActionType.LaunchAppOrOpenFile,
            path,
          };
        }
        break;
    }

    return action;
  }

  /**
   * 读取 PDF 目标位置对象
   *
   * 目标位置定义了跳转目标的具体显示参数，包括：
   *   - 目标页面索引
   *   - 缩放模式：XYZ（精确坐标+缩放）、FitPage（适应页面）、
   *     FitHorizontal（水平适应）、FitVertical（垂直适应）等
   *   - 视图参数数组（view）：缩放模式相关的参数
   * @param docPtr - pointer to pdf document object
   * @param destinationPtr - pointer to pdf destination
   * @returns pdf destination object
   *
   * @private
   */
  private readPdfDestination(docPtr: number, destinationPtr: number): PdfDestinationObject {
    // 获取目标页面索引
    const pageIndex = this.pdfiumModule.FPDFDest_GetDestPageIndex(docPtr, destinationPtr);
    // 视图参数最多 4 个，每个为 float 类型
    const maxParmamsCount = 4;
    const paramsCountPtr = this.memoryManager.malloc(maxParmamsCount);
    const paramsPtr = this.memoryManager.malloc(maxParmamsCount * 4);
    const zoomMode = this.pdfiumModule.FPDFDest_GetView(
      destinationPtr,
      paramsCountPtr,
      paramsPtr,
    ) as PdfZoomMode;
    const paramsCount = this.pdfiumModule.pdfium.getValue(paramsCountPtr, 'i32');
    const view: number[] = [];
    for (let i = 0; i < paramsCount; i++) {
      const paramPtr = paramsPtr + i * 4;
      view.push(this.pdfiumModule.pdfium.getValue(paramPtr, 'float'));
    }
    this.memoryManager.free(paramsCountPtr);
    this.memoryManager.free(paramsPtr);

    // XYZ 模式需要特殊处理：使用 FPDFDest_GetLocationInPage 获取精确的 x/y/zoom 值
    if (zoomMode === PdfZoomMode.XYZ) {
      const hasXPtr = this.memoryManager.malloc(1);
      const hasYPtr = this.memoryManager.malloc(1);
      const hasZPtr = this.memoryManager.malloc(1);
      const xPtr = this.memoryManager.malloc(4);
      const yPtr = this.memoryManager.malloc(4);
      const zPtr = this.memoryManager.malloc(4);

      const isSucceed = this.pdfiumModule.FPDFDest_GetLocationInPage(
        destinationPtr,
        hasXPtr,
        hasYPtr,
        hasZPtr,
        xPtr,
        yPtr,
        zPtr,
      );
      if (isSucceed) {
        const hasX = this.pdfiumModule.pdfium.getValue(hasXPtr, 'i8');
        const hasY = this.pdfiumModule.pdfium.getValue(hasYPtr, 'i8');
        const hasZ = this.pdfiumModule.pdfium.getValue(hasZPtr, 'i8');

        const x = hasX ? this.pdfiumModule.pdfium.getValue(xPtr, 'float') : 0;
        const y = hasY ? this.pdfiumModule.pdfium.getValue(yPtr, 'float') : 0;
        const zoom = hasZ ? this.pdfiumModule.pdfium.getValue(zPtr, 'float') : 0;

        this.memoryManager.free(hasXPtr);
        this.memoryManager.free(hasYPtr);
        this.memoryManager.free(hasZPtr);
        this.memoryManager.free(xPtr);
        this.memoryManager.free(yPtr);
        this.memoryManager.free(zPtr);

        return {
          pageIndex,
          zoom: {
            mode: zoomMode,
            params: {
              x,
              y,
              zoom,
            },
          },
          view,
        };
      }

      this.memoryManager.free(hasXPtr);
      this.memoryManager.free(hasYPtr);
      this.memoryManager.free(hasZPtr);
      this.memoryManager.free(xPtr);
      this.memoryManager.free(yPtr);
      this.memoryManager.free(zPtr);

      return {
        pageIndex,
        zoom: {
          mode: zoomMode,
          params: {
            x: 0,
            y: 0,
            zoom: 0,
          },
        },
        view,
      };
    }

    return {
      pageIndex,
      zoom: {
        mode: zoomMode,
      },
      view,
    };
  }

  /**
   * 读取 PDF 文档级附件
   *
   * 从 PDF 文档的嵌入式文件列表中读取指定索引的附件信息，包括：
   *   - 文件名、描述、MIME 类型
   *   - 文件大小、创建日期、校验和
   * @param docPtr - pointer to pdf document object
   * @param index - index of attachment
   * @returns attachment content
   *
   * @private
   */
  private readPdfAttachment(docPtr: number, index: number): PdfAttachmentObject {
    // 获取附件指针
    const attachmentPtr = this.pdfiumModule.FPDFDoc_GetAttachment(docPtr, index);
    const name = readString(
      this.pdfiumModule.pdfium,
      (buffer, bufferLength) => {
        return this.pdfiumModule.FPDFAttachment_GetName(attachmentPtr, buffer, bufferLength);
      },
      this.pdfiumModule.pdfium.UTF16ToString,
    );
    const description = readString(
      this.pdfiumModule.pdfium,
      (buffer, bufferLength) => {
        return this.pdfiumModule.EPDFAttachment_GetDescription(attachmentPtr, buffer, bufferLength);
      },
      this.pdfiumModule.pdfium.UTF16ToString,
    );
    const mimeType = readString(
      this.pdfiumModule.pdfium,
      (buffer, bufferLength) => {
        return this.pdfiumModule.FPDFAttachment_GetSubtype(attachmentPtr, buffer, bufferLength);
      },
      this.pdfiumModule.pdfium.UTF16ToString,
    );
    const creationDate = this.getAttachmentDate(attachmentPtr, 'CreationDate');
    const checksum = readString(
      this.pdfiumModule.pdfium,
      (buffer, bufferLength) => {
        return this.pdfiumModule.FPDFAttachment_GetStringValue(
          attachmentPtr,
          'Checksum',
          buffer,
          bufferLength,
        );
      },
      this.pdfiumModule.pdfium.UTF16ToString,
    );
    const size = this.getAttachmentNumber(attachmentPtr, 'Size');

    return {
      index,
      name,
      description,
      mimeType,
      size,
      creationDate,
      checksum,
    };
  }

  /**
   * 将设备坐标转换为页面坐标
   *
   * 设备坐标系：原点在左上角，X 向右，Y 向下
   * 页面坐标系：原点在左下角，X 向右，Y 向上
   *
   * 转换时需要考虑页面的旋转角度：
   *   - 0°：Y 轴翻转
   *   - 90°：X/Y 轴交换
   *   - 180°：X 轴翻转
   *   - 270°：X/Y 轴交换并翻转
   *
   * 当文档启用了 normalizedRotation 时，PDFium 内部将页面视为 0° 旋转，
   * 因此坐标转换也必须使用 0°。
   * @param doc - pdf document object
   * @param page  - pdf page infor
   * @param position - position of point
   * @returns converted position
   *
   * @private
   */
  private convertDevicePointToPagePoint(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    position: Position,
  ): Position {
    const DW = page.size.width;
    const DH = page.size.height;
    // 当启用 normalizedRotation 时，PDFium 内部将页面视为 0° 旋转，
    // 因此坐标转换也必须使用 0°
    const r = doc.normalizedRotation ? 0 : page.rotation & 3;

    if (r === 0) {
      // 0°：仅翻转 Y 轴（设备坐标 Y 向下 -> 页面坐标 Y 向上）
      return { x: position.x, y: DH - position.y };
    }
    if (r === 1) {
      // 90° 顺时针：X/Y 轴交换
      return { x: position.y, y: position.x };
    }
    if (r === 2) {
      // 180°：翻转 X 轴
      return { x: DW - position.x, y: position.y };
    }
    {
      // 270° 顺时针：X/Y 轴交换并翻转
      return { x: DH - position.y, y: DW - position.x };
    }
  }

  /**
   * 将页面坐标转换为设备坐标
   *
   * 这是 convertDevicePointToPagePoint 的逆操作。
   * @param doc - pdf document object
   * @param page  - pdf page infor
   * @param position - position of point
   * @returns converted position
   *
   * @private
   */
  private convertPagePointToDevicePoint(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    position: Position,
  ): Position {
    const DW = page.size.width;
    const DH = page.size.height;
    // 当启用 normalizedRotation 时，PDFium 内部将页面视为 0° 旋转，
    // 因此坐标转换也必须使用 0°
    const r = doc.normalizedRotation ? 0 : page.rotation & 3;

    if (r === 0) {
      // 0°：翻转 Y 轴（页面坐标 Y 向上 -> 设备坐标 Y 向下）
      return { x: position.x, y: DH - position.y };
    }
    if (r === 1) {
      // 90° 顺时针：X/Y 轴交换
      return { x: position.y, y: position.x };
    }
    if (r === 2) {
      // 180°：翻转 X 轴
      return { x: DW - position.x, y: position.y };
    }
    {
      // 270° 顺时针：X/Y 轴交换并翻转
      return { x: DW - position.y, y: DH - position.x };
    }
  }

  /**
   * 将页面坐标矩形转换为设备坐标矩形
   *
   * 将 PDF 页面坐标系中的矩形（left, top, right, bottom）转换为
   * 设备坐标系中的 Rect 对象（origin + size）。
   * @param doc - pdf document object
   * @param page  - pdf page infor
   * @param pagePtr - pointer to pdf page object
   * @param pageRect - rectangle that needs to be converted
   * @returns converted rectangle
   *
   * @private
   */
  private convertPageRectToDeviceRect(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    pageRect: {
      left: number;
      top: number;
      right: number;
      bottom: number;
    },
  ): Rect {
    const { x, y } = this.convertPagePointToDevicePoint(doc, page, {
      x: pageRect.left,
      y: pageRect.top,
    });
    const rect = {
      origin: {
        x,
        y,
      },
      size: {
        width: Math.abs(pageRect.right - pageRect.left),
        height: Math.abs(pageRect.top - pageRect.bottom),
      },
    };

    return rect;
  }

  /**
   * 读取注解的所有外观流
   *
   * 外观流（Appearance Stream）是 PDF 注解的视觉表示，以 PostScript 形式存储。
   * PDF 支持三种外观模式：
   *   - Normal：正常显示状态
   *   - Rollover：鼠标悬停状态
   *   - Down：鼠标按下状态
   * @param annotationPtr - pointer to pdf annotation
   * @param mode - appearance mode
   * @returns appearance stream
   *
   * @private
   */
  private readPageAnnoAppearanceStreams(annotationPtr: number) {
    return {
      normal: this.readPageAnnoAppearanceStream(annotationPtr, AppearanceMode.Normal),
      rollover: this.readPageAnnoAppearanceStream(annotationPtr, AppearanceMode.Rollover),
      down: this.readPageAnnoAppearanceStream(annotationPtr, AppearanceMode.Down),
    };
  }

  /**
   * 读取注解的指定外观流内容
   *
   * @param annotationPtr - PDFium 注解指针
   * @param mode - 外观模式（Normal/Rollover/Down），默认 Normal
   * @returns 外观流的字符串内容
   *
   * @private
   */
  private readPageAnnoAppearanceStream(annotationPtr: number, mode = AppearanceMode.Normal) {
    // 第一次调用获取 UTF-16 字符串长度
    const utf16Length = this.pdfiumModule.FPDFAnnot_GetAP(annotationPtr, mode, 0, 0);
    const bytesCount = (utf16Length + 1) * 2; // 包含 NUL 终止符
    const bufferPtr = this.memoryManager.malloc(bytesCount);
    this.pdfiumModule.FPDFAnnot_GetAP(annotationPtr, mode, bufferPtr, bytesCount);
    const ap = this.pdfiumModule.pdfium.UTF16ToString(bufferPtr);
    this.memoryManager.free(bufferPtr);

    return ap;
  }

  /**
   * 设置注解的外观流内容
   *
   * 将字符串形式的外观流写入注解字典的 /AP 条目。
   * 外观流定义了注解的视觉渲染方式。
   * @param annotationPtr - pointer to pdf annotation
   * @param mode - appearance mode
   * @param apContent - appearance stream content (null to remove)
   * @returns whether the appearance stream was set successfully
   *
   * @private
   */
  private setPageAnnoAppearanceStream(
    annotationPtr: number,
    mode: AppearanceMode = AppearanceMode.Normal,
    apContent: string,
  ): boolean {
    // 分配 UTF-16LE 编码缓冲区（+2 字节用于 NUL 终止符）
    const bytes = 2 * (apContent.length + 1);
    const ptr = this.memoryManager.malloc(bytes);
    try {
      this.pdfiumModule.pdfium.stringToUTF16(apContent, ptr, bytes);
      const ok = this.pdfiumModule.FPDFAnnot_SetAP(annotationPtr, mode, ptr);
      return !!ok;
    } finally {
      this.memoryManager.free(ptr);
    }
  }

  /**
   * Set the rect of specified annotation
   * @param doc - pdf document object
   * @param page - page info that the annotation is belonged to
   * @param annotationPtr - pointer to annotation object
   * @param rect - target rectangle
   * @returns whether the rect is setted
   *
   * @private
   */
  private setPageAnnoRect(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    annotPtr: number,
    rect: Rect,
  ): boolean {
    const x0d = rect.origin.x;
    const y0d = rect.origin.y;
    const x1d = rect.origin.x + rect.size.width;
    const y1d = rect.origin.y + rect.size.height;

    // Map all 4 corners to page space (handles any /Rotate)
    const TL = this.convertDevicePointToPagePoint(doc, page, { x: x0d, y: y0d });
    const TR = this.convertDevicePointToPagePoint(doc, page, { x: x1d, y: y0d });
    const BR = this.convertDevicePointToPagePoint(doc, page, { x: x1d, y: y1d });
    const BL = this.convertDevicePointToPagePoint(doc, page, { x: x0d, y: y1d });

    // Page-space AABB
    let left = Math.min(TL.x, TR.x, BR.x, BL.x);
    let right = Math.max(TL.x, TR.x, BR.x, BL.x);
    let bottom = Math.min(TL.y, TR.y, BR.y, BL.y);
    let top = Math.max(TL.y, TR.y, BR.y, BL.y);
    if (left > right) [left, right] = [right, left];
    if (bottom > top) [bottom, top] = [top, bottom];

    // Write FS_RECTF in memory order: L, T, R, B
    const ptr = this.memoryManager.malloc(16);
    const pdf = this.pdfiumModule.pdfium;
    pdf.setValue(ptr + 0, left, 'float'); // L
    pdf.setValue(ptr + 4, top, 'float'); // T
    pdf.setValue(ptr + 8, right, 'float'); // R
    pdf.setValue(ptr + 12, bottom, 'float'); // B

    const ok = this.pdfiumModule.FPDFAnnot_SetRect(annotPtr, ptr);
    this.memoryManager.free(ptr);
    return !!ok;
  }

  /**
   * 读取注解在页面坐标系中的矩形区域
   *
   * 使用 EPDFAnnot_GetRect 获取规范化后的矩形，避免某些 PDF 中
   * 坐标值不规范（如 left > right）的问题。
   * @param annotationPtr - pointer to pdf annotation
   * @returns rectangle of annotation
   *
   * @private
   */
  private readPageAnnoRect(annotationPtr: number) {
    const pageRectPtr = this.memoryManager.malloc(4 * 4);
    const pageRect = {
      left: 0,
      top: 0,
      right: 0,
      bottom: 0,
    };
    if (this.pdfiumModule.EPDFAnnot_GetRect(annotationPtr, pageRectPtr)) {
      pageRect.left = this.pdfiumModule.pdfium.getValue(pageRectPtr, 'float');
      pageRect.top = this.pdfiumModule.pdfium.getValue(pageRectPtr + 4, 'float');
      pageRect.right = this.pdfiumModule.pdfium.getValue(pageRectPtr + 8, 'float');
      pageRect.bottom = this.pdfiumModule.pdfium.getValue(pageRectPtr + 12, 'float');
    }
    this.memoryManager.free(pageRectPtr);

    return pageRect;
  }

  /**
   * 获取指定字符范围的高亮矩形区域（用于搜索高亮）
   *
   * 通过 FPDFText_CountRects 和 FPDFText_GetRect 获取文本搜索结果的
   * 精确矩形区域，每个矩形覆盖一行中连续匹配的文本。
   * 矩形的宽高使用 Math.ceil 向上取整，确保高亮完全覆盖文本字形。
   * @param doc - pdf document object
   * @param page - pdf page info
   * @param pagePtr - pointer to pdf page
   * @param textPagePtr - pointer to pdf text page
   * @param startIndex - starting character index
   * @param charCount - number of characters in the range
   * @returns array of rectangles for highlighting the specified character range
   *
   * @private
   */
  private getHighlightRects(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    textPagePtr: number,
    startIndex: number,
    charCount: number,
  ): Rect[] {
    const rectsCount = this.pdfiumModule.FPDFText_CountRects(textPagePtr, startIndex, charCount);
    const highlightRects: Rect[] = [];

    // 为页面空间矩形分配临时 double 类型内存（每个 8 字节）
    const l = this.memoryManager.malloc(8);
    const t = this.memoryManager.malloc(8);
    const r = this.memoryManager.malloc(8);
    const b = this.memoryManager.malloc(8);

    for (let i = 0; i < rectsCount; i++) {
      const ok = this.pdfiumModule.FPDFText_GetRect(textPagePtr, i, l, t, r, b);
      if (!ok) continue;

      const left = this.pdfiumModule.pdfium.getValue(l, 'double');
      const top = this.pdfiumModule.pdfium.getValue(t, 'double');
      const right = this.pdfiumModule.pdfium.getValue(r, 'double');
      const bottom = this.pdfiumModule.pdfium.getValue(b, 'double');

      // 将矩形的 4 个角点转换到设备空间
      const p1 = this.convertPagePointToDevicePoint(doc, page, { x: left, y: top });
      const p2 = this.convertPagePointToDevicePoint(doc, page, { x: right, y: top });
      const p3 = this.convertPagePointToDevicePoint(doc, page, { x: right, y: bottom });
      const p4 = this.convertPagePointToDevicePoint(doc, page, { x: left, y: bottom });

      const xs = [p1.x, p2.x, p3.x, p4.x];
      const ys = [p1.y, p2.y, p3.y, p4.y];

      const x = Math.min(...xs);
      const y = Math.min(...ys);
      const width = Math.max(...xs) - x;
      const height = Math.max(...ys) - y;

      // 使用 ceil 向上取整，确保高亮完全覆盖字形像素
      highlightRects.push({
        origin: { x, y },
        size: { width: Math.ceil(width), height: Math.ceil(height) },
      });
    }

    this.memoryManager.free(l);
    this.memoryManager.free(t);
    this.memoryManager.free(r);
    this.memoryManager.free(b);

    return highlightRects;
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.searchAllPages}
   *
   * Runs inside the worker.
   * Emits per-page progress: { page, results }
   *
   * @public
   */
  searchInPage(
    doc: PdfDocumentObject,
    page: PdfPageObject,
    keyword: string,
    flags: number,
  ): PdfTask<SearchResult[]> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'searchInPage', doc, page, keyword, flags);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `SearchInPage`, 'Begin', `${doc.id}-${page.index}`);
    // 将搜索关键字分配到 WASM 内存中
    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `PreparePrintDocument`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'Document is not open',
      });
    }
    // 分配 UTF-16 编码的关键字缓冲区（每个字符 2 字节 + NUL 终止符）
    const length = 2 * (keyword.length + 1);
    const keywordPtr = this.memoryManager.malloc(length);
    this.pdfiumModule.pdfium.stringToUTF16(keyword, keywordPtr, length);

    try {
      const results = this.searchAllInPage(doc, ctx, page, keywordPtr, flags);
      return PdfTaskHelper.resolve(results);
    } finally {
      this.memoryManager.free(keywordPtr);
    }
  }

  /**
   * 批量获取多页的注解数据
   *
   * 在一次调用中处理多个页面的注解读取，通过 Task 的 progress 回调
   * 逐页推送进度，支持流式更新界面。
   *
   * @param doc - PDF 文档对象
   * @param pages - 要处理的页面对象数组
   * @returns Task，结果按页面索引键控，带逐页进度回调
   *
   * @public
   */
  getAnnotationsBatch(
    doc: PdfDocumentObject,
    pages: PdfPageObject[],
  ): PdfTask<Record<number, PdfAnnotationObject[]>, BatchProgress<PdfAnnotationObject[]>> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'getAnnotationsBatch', doc.id, pages.length);

    const task = new Task<
      Record<number, PdfAnnotationObject[]>,
      PdfErrorReason,
      BatchProgress<PdfAnnotationObject[]>
    >();

    // 将工作延迟到下一个微任务，以便调用者可以先设置 onProgress 监听器
    queueMicrotask(() => {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'GetAnnotationsBatch', 'Begin', doc.id);

      const ctx = this.cache.getContext(doc.id);
      if (!ctx) {
        task.reject({ code: PdfErrorCode.DocNotOpen, message: 'Document is not open' });
        return;
      }

      const results: Record<number, PdfAnnotationObject[]> = {};
      const total = pages.length;

      const formInfoPtr = this.pdfiumModule.PDFiumExt_OpenFormFillInfo();
      const formHandle = this.pdfiumModule.PDFiumExt_InitFormFillEnvironment(
        ctx.docPtr,
        formInfoPtr,
      );

      try {
        for (let i = 0; i < pages.length; i++) {
          const page = pages[i];
          const annotations = this.readPageAnnotationsRaw(doc, ctx, page, formHandle);
          results[page.index] = annotations;

          // 逐页推送进度更新
          task.progress({
            pageIndex: page.index,
            result: annotations,
            completed: i + 1,
            total,
          });
        }
      } finally {
        this.pdfiumModule.PDFiumExt_ExitFormFillEnvironment(formHandle);
        this.pdfiumModule.PDFiumExt_CloseFormFillInfo(formInfoPtr);
      }

      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'GetAnnotationsBatch', 'End', doc.id);
      task.resolve(results);
    });

    return task;
  }

  /**
   * 批量在多个页面中搜索文本
   *
   * 关键字缓冲区只分配一次，在所有页面间共享，
   * 通过 Task 的 progress 回调逐页推送进度。
   *
   * @param doc - PDF 文档对象
   * @param pages - 要搜索的页面对象数组
   * @param keyword - 搜索关键字
   * @param flags - 搜索标志位
   * @returns Task，结果按页面索引键控，带逐页进度回调
   *
   * @public
   */
  searchBatch(
    doc: PdfDocumentObject,
    pages: PdfPageObject[],
    keyword: string,
    flags: number,
  ): PdfTask<Record<number, SearchResult[]>, BatchProgress<SearchResult[]>> {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'searchBatch', doc.id, pages.length, keyword);

    const task = new Task<
      Record<number, SearchResult[]>,
      PdfErrorReason,
      BatchProgress<SearchResult[]>
    >();

    // 将工作延迟到下一个微任务，以便调用者可以先设置 onProgress 监听器
    queueMicrotask(() => {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'SearchBatch', 'Begin', doc.id);

      const ctx = this.cache.getContext(doc.id);
      if (!ctx) {
        task.reject({ code: PdfErrorCode.DocNotOpen, message: 'Document is not open' });
        return;
      }

      // 关键字指针只分配一次，在所有页面间共享
      // 关键字指针只分配一次，在所有页面间共享
      const length = 2 * (keyword.length + 1);
      const keywordPtr = this.memoryManager.malloc(length);
      this.pdfiumModule.pdfium.stringToUTF16(keyword, keywordPtr, length);

      try {
        const results: Record<number, SearchResult[]> = {};
        const total = pages.length;

        // 在紧凑循环中处理所有页面 - 无队列开销
        for (let i = 0; i < pages.length; i++) {
          const page = pages[i];
          const pageResults = this.searchAllInPage(doc, ctx, page, keywordPtr, flags);
          results[page.index] = pageResults;

          // 逐页推送进度更新
          task.progress({
            pageIndex: page.index,
            result: pageResults,
            completed: i + 1,
            total,
          });
        }

        this.logger.perf(LOG_SOURCE, LOG_CATEGORY, 'SearchBatch', 'End', doc.id);
        task.resolve(results);
      } finally {
        this.memoryManager.free(keywordPtr);
      }
    });

    return task;
  }

  /**
   * 为搜索匹配结果提取词对齐的上下文文本
   *
   * 从完整页面文本中提取匹配位置前后的上下文，确保在词边界处截断，
   * 不会在单词中间截断。返回匹配前文本、匹配文本、匹配后文本。
   *
   * @param fullText - 完整的页面文本（每页只需获取一次）
   * @param start - 匹配的起始字符索引
   * @param count - 匹配的字符数
   * @param windowChars - 左右上下文最少保留的字符数，默认 30
   */
  private buildContext(
    fullText: string,
    start: number,
    count: number,
    windowChars = 30,
  ): TextContext {
    const WORD_BREAK = /[\s\u00A0.,;:!?()\[\]{}<>/\\\-"'`"”\u2013\u2014]/;

    // Find the start of a word moving left
    const findWordStart = (index: number): number => {
      while (index > 0 && !WORD_BREAK.test(fullText[index - 1])) index--;
      return index;
    };

    // Find the end of a word moving right
    const findWordEnd = (index: number): number => {
      while (index < fullText.length && !WORD_BREAK.test(fullText[index])) index++;
      return index;
    };

    // Move left to build context
    let left = start;
    while (left > 0 && WORD_BREAK.test(fullText[left - 1])) left--; // Skip blanks
    let collected = 0;
    while (left > 0 && collected < windowChars) {
      left--;
      if (!WORD_BREAK.test(fullText[left])) collected++;
    }
    left = findWordStart(left);

    // Move right to build context
    let right = start + count;
    while (right < fullText.length && WORD_BREAK.test(fullText[right])) right++; // Skip blanks
    collected = 0;
    while (right < fullText.length && collected < windowChars) {
      if (!WORD_BREAK.test(fullText[right])) collected++;
      right++;
    }
    right = findWordEnd(right);

    // Compose the context
    const before = fullText.slice(left, start).replace(/\s+/g, ' ').trimStart();
    const match = fullText.slice(start, start + count);
    const after = fullText
      .slice(start + count, right)
      .replace(/\s+/g, ' ')
      .trimEnd();

    return {
      before: this.tidy(before),
      match: this.tidy(match),
      after: this.tidy(after),
      truncatedLeft: left > 0,
      truncatedRight: right < fullText.length,
    };
  }

  /**
   * 清理文本中的不可打印字符和空白
   *
   * 处理步骤：
   *   1. 合并被软回车（U+FFFE）分割的连字符单词
   *   2. 移除所有 U+FFFE、软连字符（U+00AD）、零宽字符（U+200B 等）
   *   3. 将所有连续空白压缩为单个空格
   * @param s - text to tidy
   * @returns tidied text
   *
   * @private
   */
  private tidy(s: string): string {
    return (
      s
        /* 1️⃣  join words split by hyphen + U+FFFE + whitespace */
        .replace(/-\uFFFE\s*/g, '')

        /* 2️⃣  drop any remaining U+FFFE, soft-hyphen, zero-width chars */
        .replace(/[\uFFFE\u00AD\u200B\u2060\uFEFF]/g, '')

        /* 3️⃣  collapse whitespace so we stay on one line */
        .replace(/\s+/g, ' ')
    );
  }

  /**
   * 在单个页面中搜索所有匹配项
   *
   * 高效地只加载一次页面并查找所有匹配项：
   *   1. 获取文本页面对象并加载完整文本
   *   2. 使用 FPDFText_FindStart 初始化搜索句柄
   *   3. 循环调用 FPDFText_FindNext 获取所有匹配结果
   *   4. 对每个匹配计算高亮矩形和上下文文本
   *
   * @param doc - PDF 文档对象
   * @param ctx - 文档上下文
   * @param page - PDF 页面对象
   * @param keywordPtr - 搜索关键字的 WASM 内存指针
   * @param flag - 搜索标志位
   * @returns 搜索结果数组
   *
   * @private
   */
  private searchAllInPage(
    doc: PdfDocumentObject,
    ctx: DocumentContext,
    page: PdfPageObject,
    keywordPtr: number,
    flag: number,
  ): SearchResult[] {
    return ctx.borrowPage(page.index, (pageCtx) => {
      const textPagePtr = pageCtx.getTextPage();

      // Load the full text of the page once
      const total = this.pdfiumModule.FPDFText_CountChars(textPagePtr);
      const bufPtr = this.memoryManager.malloc(2 * (total + 1));
      this.pdfiumModule.FPDFText_GetText(textPagePtr, 0, total, bufPtr);
      const fullText = this.pdfiumModule.pdfium.UTF16ToString(bufPtr);
      this.memoryManager.free(bufPtr);

      const pageResults: SearchResult[] = [];

      // Initialize search handle once for the page
      const searchHandle = this.pdfiumModule.FPDFText_FindStart(
        textPagePtr,
        keywordPtr,
        flag,
        0, // Start from the beginning of the page
      );

      // Call FindNext repeatedly to get all matches on the page
      while (this.pdfiumModule.FPDFText_FindNext(searchHandle)) {
        const charIndex = this.pdfiumModule.FPDFText_GetSchResultIndex(searchHandle);
        const charCount = this.pdfiumModule.FPDFText_GetSchCount(searchHandle);

        const rects = this.getHighlightRects(doc, page, textPagePtr, charIndex, charCount);

        const context = this.buildContext(fullText, charIndex, charCount);

        pageResults.push({
          pageIndex: page.index,
          charIndex,
          charCount,
          rects,
          context,
        });
      }

      // Close the search handle only once after finding all results
      this.pdfiumModule.FPDFText_FindClose(searchHandle);
      return pageResults;
    });
  }

  /**
   * {@inheritDoc @embedpdf/models!PdfEngine.preparePrintDocument}
   *
   * 准备打印用的 PDF 文档。
   *
   * 工作流程：
   *   1. 创建新的空白 PDF 文档
   *   2. 根据页面范围从源文档导入指定页面
   *   3. 如果不需要注解，则从打印文档中移除所有注解
   *   4. 将准备好的文档保存为 ArrayBuffer 返回
   *
   * @public
   */
  preparePrintDocument(doc: PdfDocumentObject, options?: PdfPrintOptions): PdfTask<ArrayBuffer> {
    const { includeAnnotations = true, pageRange = null } = options ?? {};

    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'preparePrintDocument', doc, options);
    this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `PreparePrintDocument`, 'Begin', doc.id);

    // 验证文档是否已打开
    const ctx = this.cache.getContext(doc.id);
    if (!ctx) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `PreparePrintDocument`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.DocNotOpen,
        message: 'Document is not open',
      });
    }

    // 创建新的空白文档用于打印
    const printDocPtr = this.pdfiumModule.FPDF_CreateNewDocument();
    if (!printDocPtr) {
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `PreparePrintDocument`, 'End', doc.id);
      return PdfTaskHelper.reject({
        code: PdfErrorCode.CantCreateNewDoc,
        message: 'Cannot create print document',
      });
    }

    try {
      // 验证并规范化页码范围
      const sanitizedPageRange = this.sanitizePageRange(pageRange, doc.pageCount);

      // 从源文档导入页面
      // pageRange 为 null 表示导入所有页面
      if (
        !this.pdfiumModule.FPDF_ImportPages(
          printDocPtr,
          ctx.docPtr,
          sanitizedPageRange ?? '',
          0, // Insert at beginning
        )
      ) {
        this.pdfiumModule.FPDF_CloseDocument(printDocPtr);
        this.logger.error(LOG_SOURCE, LOG_CATEGORY, 'Failed to import pages for printing');
        this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `PreparePrintDocument`, 'End', doc.id);

        return PdfTaskHelper.reject({
          code: PdfErrorCode.CantImportPages,
          message: 'Failed to import pages for printing',
        });
      }

      // 如果要求移除注解
      if (!includeAnnotations) {
        const removalResult = this.removeAnnotationsFromPrintDocument(printDocPtr);

        if (!removalResult.success) {
          this.pdfiumModule.FPDF_CloseDocument(printDocPtr);
          this.logger.error(
            LOG_SOURCE,
            LOG_CATEGORY,
            `Failed to remove annotations: ${removalResult.error}`,
          );
          this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `PreparePrintDocument`, 'End', doc.id);

          return PdfTaskHelper.reject({
            code: PdfErrorCode.Unknown,
            message: `Failed to prepare print document: ${removalResult.error}`,
          });
        }

        this.logger.debug(
          LOG_SOURCE,
          LOG_CATEGORY,
          `Removed ${removalResult.annotationsRemoved} annotations from ${removalResult.pagesProcessed} pages`,
        );
      }

      // 将准备好的文档保存到缓冲区
      const buffer = this.saveDocument(printDocPtr);

      // Clean up
      this.pdfiumModule.FPDF_CloseDocument(printDocPtr);

      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `PreparePrintDocument`, 'End', doc.id);
      return PdfTaskHelper.resolve(buffer);
    } catch (error) {
      // 发生错误时确保资源被清理
      if (printDocPtr) {
        this.pdfiumModule.FPDF_CloseDocument(printDocPtr);
      }

      this.logger.error(LOG_SOURCE, LOG_CATEGORY, 'preparePrintDocument failed', error);
      this.logger.perf(LOG_SOURCE, LOG_CATEGORY, `PreparePrintDocument`, 'End', doc.id);

      return PdfTaskHelper.reject({
        code: PdfErrorCode.Unknown,
        message: error instanceof Error ? error.message : 'Failed to prepare print document',
      });
    }
  }

  /**
   * 从打印文档中移除所有注解
   *
   * 使用高性能的原始注解函数（Raw Annotation Functions）操作，
   * 无需完整加载页面即可移除注解，显著提升性能。
   *
   * 关键优化：
   *   - 使用 EPDFPage_GetAnnotCountRaw 直接获取注解数量，无需加载页面
   *   - 使用 EPDFPage_RemoveAnnotRaw 直接移除注解
   *   - 按逆序移除注解，避免索引偏移问题
   *   - 注解移除后重新生成页面内容流
   *
   * @param printDocPtr - 打印文档的 PDFium 指针
   * @returns 结果对象，包含成功状态和统计信息
   *
   * @private
   */
  private removeAnnotationsFromPrintDocument(printDocPtr: number): {
    success: boolean;
    annotationsRemoved: number;
    pagesProcessed: number;
    error?: string;
  } {
    let totalAnnotationsRemoved = 0;
    let pagesProcessed = 0;

    try {
      const pageCount = this.pdfiumModule.FPDF_GetPageCount(printDocPtr);

      // Process each page
      for (let pageIndex = 0; pageIndex < pageCount; pageIndex++) {
        // 使用快速原始函数获取注解数量，无需完整加载页面
        const annotCount = this.pdfiumModule.EPDFPage_GetAnnotCountRaw(printDocPtr, pageIndex);

        if (annotCount <= 0) {
          pagesProcessed++;
          continue;
        }

        // 按逆序移除注解以维护索引正确性
        // 这很重要，因为移除一个注解会使后续注解的索引向前移动
        let annotationsRemovedFromPage = 0;

        for (let annotIndex = annotCount - 1; annotIndex >= 0; annotIndex--) {
          // 使用快速原始移除函数
          const removed = this.pdfiumModule.EPDFPage_RemoveAnnotRaw(
            printDocPtr,
            pageIndex,
            annotIndex,
          );

          if (removed) {
            annotationsRemovedFromPage++;
            totalAnnotationsRemoved++;
          } else {
            this.logger.warn(
              LOG_SOURCE,
              LOG_CATEGORY,
              `Failed to remove annotation ${annotIndex} from page ${pageIndex}`,
            );
          }
        }

        // 如果有注解被移除，需要重新生成页面内容流
        if (annotationsRemovedFromPage > 0) {
          // 移除注解后需要重新生成页面内容，确保渲染正确
          const pagePtr = this.pdfiumModule.FPDF_LoadPage(printDocPtr, pageIndex);
          if (pagePtr) {
            this.pdfiumModule.FPDFPage_GenerateContent(pagePtr);
            this.pdfiumModule.FPDF_ClosePage(pagePtr);
          }
        }

        pagesProcessed++;
      }

      return {
        success: true,
        annotationsRemoved: totalAnnotationsRemoved,
        pagesProcessed: pagesProcessed,
      };
    } catch (error) {
      return {
        success: false,
        annotationsRemoved: totalAnnotationsRemoved,
        pagesProcessed: pagesProcessed,
        error: error instanceof Error ? error.message : 'Unknown error during annotation removal',
      };
    }
  }

  /**
   * 验证并规范化页码范围字符串
   *
   * 支持的格式：
   *   - 单页："3"
   *   - 范围："1-5"
   *   - 组合："1,3,5-7"
   *   - null/空字符串：表示所有页面
   *
   * @param pageRange - 页码范围字符串（如 "1,3,5-7"），null 表示所有页面
   * @param totalPages - 文档总页数
   * @returns 规范化后的页码范围字符串，null 表示所有页面
   *
   * @private
   */
  private sanitizePageRange(
    pageRange: string | null | undefined,
    totalPages: number,
  ): string | null {
    // null 或空字符串表示所有页面
    if (!pageRange || pageRange.trim() === '') {
      return null;
    }

    try {
      const sanitized: number[] = [];
      const parts = pageRange.split(',');

      for (const part of parts) {
        const trimmed = part.trim();

        if (trimmed.includes('-')) {
          // 处理范围格式（如 "5-7"）
          const [startStr, endStr] = trimmed.split('-').map((s) => s.trim());
          const start = parseInt(startStr, 10);
          const end = parseInt(endStr, 10);

          if (isNaN(start) || isNaN(end)) {
            this.logger.warn(LOG_SOURCE, LOG_CATEGORY, `Invalid range: ${trimmed}`);
            continue;
          }

          // Clamp to valid bounds (1-based page numbers)
          const validStart = Math.max(1, Math.min(start, totalPages));
          const validEnd = Math.max(1, Math.min(end, totalPages));

          // Add all pages in range
          for (let i = validStart; i <= validEnd; i++) {
            if (!sanitized.includes(i)) {
              sanitized.push(i);
            }
          }
        } else {
          // 处理单页码格式
          const pageNum = parseInt(trimmed, 10);

          if (isNaN(pageNum)) {
            this.logger.warn(LOG_SOURCE, LOG_CATEGORY, `Invalid page number: ${trimmed}`);
            continue;
          }

          // Clamp to valid bounds
          const validPageNum = Math.max(1, Math.min(pageNum, totalPages));

          if (!sanitized.includes(validPageNum)) {
            sanitized.push(validPageNum);
          }
        }
      }

      // 如果没有有效页码，返回 null（表示所有页面）
      if (sanitized.length === 0) {
        this.logger.warn(LOG_SOURCE, LOG_CATEGORY, 'No valid pages in range, using all pages');
        return null;
      }

      // Sort and convert back to range string
      sanitized.sort((a, b) => a - b);

      // 将连续的页码优化为范围格式
      const optimized: string[] = [];
      let rangeStart = sanitized[0];
      let rangeEnd = sanitized[0];

      for (let i = 1; i < sanitized.length; i++) {
        if (sanitized[i] === rangeEnd + 1) {
          rangeEnd = sanitized[i];
        } else {
          // 结束当前范围
          if (rangeStart === rangeEnd) {
            optimized.push(rangeStart.toString());
          } else if (rangeEnd - rangeStart === 1) {
            optimized.push(rangeStart.toString());
            optimized.push(rangeEnd.toString());
          } else {
            optimized.push(`${rangeStart}-${rangeEnd}`);
          }

          // 开始新范围
          rangeStart = sanitized[i];
          rangeEnd = sanitized[i];
        }
      }

      // 添加最后一个范围
      if (rangeStart === rangeEnd) {
        optimized.push(rangeStart.toString());
      } else if (rangeEnd - rangeStart === 1) {
        optimized.push(rangeStart.toString());
        optimized.push(rangeEnd.toString());
      } else {
        optimized.push(`${rangeStart}-${rangeEnd}`);
      }

      const result = optimized.join(',');

      this.logger.debug(
        LOG_SOURCE,
        LOG_CATEGORY,
        `Sanitized page range: "${pageRange}" -> "${result}"`,
      );

      return result;
    } catch (error) {
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, `Error sanitizing page range: ${error}`);
      return null; // Fallback to all pages
    }
  }
}
