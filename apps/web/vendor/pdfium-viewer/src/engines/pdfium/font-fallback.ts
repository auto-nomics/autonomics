/**
 * @file font-fallback.ts
 *
 * PDFium 字体回退管理器 —— 纯 TypeScript 实现
 *
 * 【文件概述】
 * 本文件实现了 PDFium 渲染引擎的字体回退机制。当 PDF 文档中引用了未内嵌的字体时，
 * PDFium 会通过系统字体信息接口（FPDF_SYSFONTINFO）向宿主程序请求字体数据。
 * 本模块通过 Emscripten 的 addFunction API 在 JavaScript 层面完整实现该接口，
 * 使得 PDFium 在 Web Worker 环境中也能按需加载缺失的字体。
 *
 * 【核心机制】
 * 1. 在 WASM 线性内存中构造 FPDF_SYSFONTINFO C 结构体，并将 JavaScript 回调函数
 *    注册为结构体中的函数指针。
 * 2. PDFium 渲染文字时若发现缺失字体，依次调用 MapFont → GetFontData 等回调。
 * 3. 回调函数根据字符集（charset）查找用户配置的字体，通过网络或自定义加载器
 *    获取字体二进制数据，复制到 WASM 内存中供 PDFium 使用。
 *
 * 【字体匹配算法】
 * 采用 CSS font matching 的简化版本：
 * - 优先匹配斜体/正体
 * - 在匹配的候选中选择 weight 距离最近的变体
 * - weight 相同时，请求字重 ≥ 500 时偏好更粗，< 500 时偏好更细
 */

import { FontCharset, Logger, NoopLogger } from '../../models';
import type { WrappedPdfiumModule } from '../../pdfium';

// 重新导出 FontCharset 枚举，方便外部使用
export { FontCharset };

/**
 * 单个字体变体的描述信息
 *
 * 表示一种字重（weight）与斜体（italic）的组合，对应一个字体文件。
 * 例如 "Noto Sans JP Bold Italic" 对应 weight=700, italic=true。
 */
export interface FontVariant {
  /** 字体文件的 URL 或路径 */
  url: string;
  /** 字重值，范围 100-900，默认 400（Regular） */
  weight?: number;
  /** 是否为斜体变体，默认 false */
  italic?: boolean;
}

/**
 * 字体配置条目的联合类型
 *
 * 为了使用便捷，支持三种配置形式：
 * - 简单字符串 URL：等同于 weight=400、非斜体的单个 FontVariant
 * - 单个 FontVariant 对象：指定具体字重和斜体信息
 * - FontVariant 数组：提供同一字体的多种字重/斜体变体，系统会自动选择最匹配的
 */
export type FontEntry = string | FontVariant | FontVariant[];

/**
 * 自定义字体加载器函数类型
 *
 * 当默认的 XMLHttpRequest 加载方式不适用时（例如 Node.js 环境），
 * 可通过此函数提供自定义的字体加载逻辑。
 *
 * @param fontPath - 字体文件路径或 URL
 * @returns 字体的二进制数据（Uint8Array），加载失败时返回 null
 */
export type FontLoader = (fontPath: string) => Uint8Array | null;

/**
 * 字体回退配置接口
 *
 * 将 PDF 字符集编号映射到对应的字体文件，供 FontFallbackManager 使用。
 * 支持按字符集精确配置，也支持提供默认字体作为兜底。
 */
export interface FontFallbackConfig {
  /**
   * 字符集到字体条目的映射表
   *
   * 键为 FontCharset 枚举值（如 SHIFTJIS=128, HANGEUL=129 等），
   * 值可以是简单 URL 字符串、单个 FontVariant 或 FontVariant 数组。
   * 使用 Partial<Record<...>> 表示并非所有字符集都需要配置。
   */
  fonts: Partial<Record<FontCharset, FontEntry>>;

  /**
   * 可选的默认字体条目
   *
   * 当请求的字符集在 fonts 映射中未找到时，使用此字体作为兜底。
   * 如果也未配置 defaultFont，则 PDFium 将无法渲染该字符集的文字。
   */
  defaultFont?: FontEntry;

  /**
   * 相对字体 URL 的基础路径前缀（浏览器环境）
   *
   * 当字体 URL 不是以 http://、https:// 或 / 开头时，
   * 会自动在前面拼接此 baseUrl。
   */
  baseUrl?: string;

  /**
   * 自定义字体加载器函数
   *
   * 提供此函数后，系统将使用它替代默认的 XMLHttpRequest 加载器来获取字体数据。
   * 典型使用场景：
   * - Node.js 环境：使用 fs.readFileSync 从本地文件系统读取
   * - 自定义缓存策略
   * - 从非标准数据源加载
   *
   * @example Node.js 使用示例：
   * ```typescript
   * import { readFileSync } from 'fs';
   * import { join } from 'path';
   *
   * const fontFallback = {
   *   fonts: { [FontCharset.SHIFTJIS]: 'NotoSansJP-Regular.otf' },
   *   fontLoader: (fontPath) => {
   *     try {
   *       return new Uint8Array(readFileSync(join('/fonts', fontPath)));
   *     } catch {
   *       return null;
   *     }
   *   },
   * };
   * ```
   */
  fontLoader?: FontLoader;
}

/**
 * 字体句柄 —— PDFium 与 JS 层之间的字体关联对象
 *
 * 每次 PDFium 请求映射一个字体（MapFont 回调），就会创建一个 FontHandle。
 * 句柄通过自增 ID 标识，PDFium 后续使用该 ID 来获取字体数据或删除字体。
 * data 字段延迟加载：首次 GetFontData 时才真正获取字体二进制数据。
 */
interface FontHandle {
  /** 句柄唯一 ID，由 nextHandleId 自增生成，作为返回给 PDFium 的指针值 */
  id: number;
  /** 字体对应的字符集编号（如 SHIFTJIS=128） */
  charset: number;
  /** 请求时的字重值 */
  weight: number;
  /** 请求时是否为斜体 */
  italic: boolean;
  /** 字体文件的 URL（经过 baseUrl 拼接后的完整路径） */
  url: string;
  /** 字体二进制数据，首次 GetFontData 时延迟加载 */
  data: Uint8Array | null;
}

/**
 * FPDF_SYSFONTINFO 结构体内存布局（32 位 WASM 指针）
 *
 * 这是在 WASM 线性内存中分配的 C 结构体，用于向 PDFium 注册系统字体信息接口。
 * 所有指针在 32 位 WASM 中占 4 字节，int 占 4 字节。
 *
 * struct FPDF_SYSFONTINFO {
 *   int version;                     // 偏移 0，  大小 4 —— 结构体版本号，必须为 1
 *   void (*Release)(...);            // 偏移 4，  大小 4 —— 释放回调（可选）
 *   void (*EnumFonts)(...);          // 偏移 8，  大小 4 —— 枚举字体回调（可选）
 *   void* (*MapFont)(...);           // 偏移 12， 大小 4 —— 按字符集映射字体（核心）
 *   void* (*GetFont)(...);           // 偏移 16， 大小 4 —— 按名称获取字体
 *   unsigned long (*GetFontData)(...);// 偏移 20， 大小 4 —— 获取字体二进制数据（核心）
 *   unsigned long (*GetFaceName)(...);// 偏移 24， 大小 4 —— 获取字体名称（可选）
 *   int (*GetFontCharset)(...);      // 偏移 28， 大小 4 —— 获取字体字符集
 *   void (*DeleteFont)(...);         // 偏移 32， 大小 4 —— 删除字体句柄
 * };
 * 总大小：36 字节
 */
const SYSFONTINFO_SIZE = 36; // 结构体总字节数
const OFFSET_VERSION = 0; // 版本号字段偏移
const OFFSET_RELEASE = 4; // Release 回调偏移
const OFFSET_ENUMFONTS = 8; // EnumFonts 回调偏移
const OFFSET_MAPFONT = 12; // MapFont 回调偏移
const OFFSET_GETFONT = 16; // GetFont 回调偏移
const OFFSET_GETFONTDATA = 20; // GetFontData 回调偏移
const OFFSET_GETFACENAME = 24; // GetFaceName 回调偏移
const OFFSET_GETFONTCHARSET = 28; // GetFontCharset 回调偏移
const OFFSET_DELETEFONT = 32; // DeleteFont 回调偏移

/** 日志来源标识 */
const LOG_SOURCE = 'pdfium';
/** 日志分类标识 */
const LOG_CATEGORY = 'font-fallback';

/**
 * PDFium 字体回退管理器 —— 纯 TypeScript 实现
 *
 * 【职责】
 * 当 PDFium 渲染 PDF 文档时遇到未内嵌的字体，会通过 FPDF_SYSFONTINFO 接口
 * 向宿主程序请求字体数据。本管理器负责：
 * - 在 WASM 内存中构造并注册 FPDF_SYSFONTINFO 结构体
 * - 将 JavaScript 回调函数注册为 PDFium 可调用的函数指针
 * - 根据 PDFium 传入的字符集、字重、斜体等参数，匹配并加载对应的字体文件
 * - 管理字体句柄的生命周期和字体数据的缓存
 *
 * 【实现原理】
 * 使用 Emscripten 的 addFunction API 将 JavaScript 函数注册到 WASM 函数表中，
 * 获取函数指针后填入 FPDF_SYSFONTINFO 结构体，再通过 FPDF_SetSystemFontInfo
 * 将结构体注册到 PDFium 引擎。后续 PDFium 的所有字体请求都会路由到这些回调。
 *
 * 【线程安全】
 * 此类设计为在 Web Worker 中运行，因此 fetchFontSync 使用同步 XMLHttpRequest
 * 不会阻塞主线程。
 */
export class FontFallbackManager {
  /** 字体回退配置，包含字符集到字体文件的映射 */
  private readonly fontConfig: FontFallbackConfig;
  /** 日志记录器 */
  private readonly logger: Logger;
  /** 活跃的字体句柄映射表：handleId → FontHandle */
  private readonly fontHandles: Map<number, FontHandle> = new Map();
  /** 字体数据缓存：URL → Uint8Array，避免重复下载同一字体 */
  private readonly fontCache: Map<string, Uint8Array> = new Map();
  /** 下一个可用的句柄 ID（自增计数器） */
  private nextHandleId = 1;
  /** PDFium WASM 模块引用 */
  private module: WrappedPdfiumModule | null = null;
  /** 字体回退系统是否已启用 */
  private enabled = false;

  // ========== WASM 资源指针（用于清理时释放） ==========

  /** FPDF_SYSFONTINFO 结构体在 WASM 内存中的地址 */
  private structPtr: number = 0;
  /** Release 回调函数指针 */
  private releaseFnPtr: number = 0;
  /** EnumFonts 回调函数指针 */
  private enumFontsFnPtr: number = 0;
  /** MapFont 回调函数指针 */
  private mapFontFnPtr: number = 0;
  /** GetFont 回调函数指针 */
  private getFontFnPtr: number = 0;
  /** GetFontData 回调函数指针 */
  private getFontDataFnPtr: number = 0;
  /** GetFaceName 回调函数指针 */
  private getFaceNameFnPtr: number = 0;
  /** GetFontCharset 回调函数指针 */
  private getFontCharsetFnPtr: number = 0;
  /** DeleteFont 回调函数指针 */
  private deleteFontFnPtr: number = 0;

  /**
   * 构造字体回退管理器
   *
   * @param config - 字体回退配置，包含字符集到字体的映射和可选的加载器
   * @param logger - 日志记录器，默认为无操作记录器（NoopLogger）
   */
  constructor(config: FontFallbackConfig, logger: Logger = new NoopLogger()) {
    this.fontConfig = config;
    this.logger = logger;
  }

  /**
   * 初始化字体回退系统并注册到 PDFium 模块
   *
   * 此方法执行以下步骤：
   * 1. 检查是否已初始化（防重复调用）
   * 2. 检查 addFunction API 可用性（需要 WASM 编译时开启 -sALLOW_TABLE_GROWTH）
   * 3. 在 WASM 内存中分配 FPDF_SYSFONTINFO 结构体（36 字节）
   * 4. 使用 addFunction 将 JS 回调注册为 8 个 WASM 函数指针
   * 5. 将函数指针填入结构体对应偏移位置
   * 6. 通过 FPDF_SetSystemFontInfo 将结构体注册到 PDFium
   *
   * addFunction 签名格式说明：
   * - 'v' = void，'i' = int32
   * - 返回值类型 + 参数类型拼接，例如 'vi' 表示 void(int)
   * - 所有 WASM 指针用 'i' 表示（32 位环境）
   *
   * @param module - 已加载的 PDFium WASM 模块包装器
   * @throws 当内存分配失败或函数注册失败时抛出异常
   */
  initialize(module: WrappedPdfiumModule): void {
    // 防止重复初始化
    if (this.enabled) {
      this.logger.warn(LOG_SOURCE, LOG_CATEGORY, 'Font fallback already initialized');
      return;
    }

    this.module = module;
    const pdfium = module.pdfium;

    // 检查 addFunction 是否可用——这是将 JS 函数导出为 WASM 函数指针的关键 API
    if (typeof pdfium.addFunction !== 'function') {
      this.logger.error(
        LOG_SOURCE,
        LOG_CATEGORY,
        'addFunction not available. Make sure WASM is compiled with -sALLOW_TABLE_GROWTH',
      );
      return;
    }

    try {
      // ========== 第一步：分配并清零 FPDF_SYSFONTINFO 结构体 ==========
      this.structPtr = pdfium.wasmExports.malloc(SYSFONTINFO_SIZE);
      if (!this.structPtr) {
        throw new Error('Failed to allocate FPDF_SYSFONTINFO struct');
      }

      // 逐字节清零，确保所有函数指针初始为 NULL
      for (let i = 0; i < SYSFONTINFO_SIZE; i++) {
        pdfium.setValue(this.structPtr + i, 0, 'i8');
      }

      // ========== 第二步：注册各回调函数为 WASM 函数指针 ==========
      // 签名说明：'v' = void, 'i' = int32
      // WASM 中所有指针为 32 位，统一用 'i' 表示

      // Release: void (*)(FPDF_SYSFONTINFO* pThis)
      // PDFium 销毁结构体时调用。此处无需操作，由 FontFallbackManager 自行管理生命周期。
      this.releaseFnPtr = pdfium.addFunction((_pThis: number) => {
        // 无需操作 —— 我们在 disable()/cleanup() 中自行释放资源
      }, 'vi');

      // EnumFonts: void (*)(FPDF_SYSFONTINFO* pThis, void* pMapper)
      // PDFium 枚举可用字体时调用。我们不预先枚举，PDFium 会在需要时调用 MapFont。
      this.enumFontsFnPtr = pdfium.addFunction((_pThis: number, _pMapper: number) => {
        // 不枚举字体 —— PDFium 会在需要时通过 MapFont 按需请求
      }, 'vii');

      // MapFont: void* (*)(FPDF_SYSFONTINFO* pThis, int weight, FPDF_BOOL bItalic,
      //                    int charset, int pitch_family, const char* face, FPDF_BOOL* bExact)
      // 核心回调：PDFium 请求映射字体时调用。
      // 返回值为字体句柄 ID（作为 void* 传回 PDFium）。
      // 签名：返回值(i) + 7 个参数(iiiiiii) = 'iiiiiiii'（8 个字符）
      this.mapFontFnPtr = pdfium.addFunction(
        (
          _pThis: number,
          weight: number,
          bItalic: number,
          charset: number,
          pitchFamily: number,
          facePtr: number,
          bExactPtr: number,
        ) => {
          // 将 UTF-8 C 字符串转为 JS 字符串
          const face = facePtr ? pdfium.UTF8ToString(facePtr) : '';
          const handle = this.mapFont(weight, bItalic, charset, pitchFamily, face);

          // 将 bExact 设为 0（false）—— 我们提供的是回退字体，不是精确匹配
          if (bExactPtr) {
            pdfium.setValue(bExactPtr, 0, 'i32');
          }

          return handle;
        },
        'iiiiiiii',
      );

      // GetFont: void* (*)(FPDF_SYSFONTINFO* pThis, const char* face)
      // 按字体名称获取字体。委托给 mapFont，使用默认参数（weight=400, 非斜体）。
      this.getFontFnPtr = pdfium.addFunction((_pThis: number, facePtr: number) => {
        const face = facePtr ? pdfium.UTF8ToString(facePtr) : '';
        // 委托给 mapFont，使用默认字重 400、非斜体、默认字符集
        return this.mapFont(400, 0, 0, 0, face);
      }, 'iii');

      // GetFontData: unsigned long (*)(FPDF_SYSFONTINFO* pThis, void* hFont,
      //                                unsigned int table, unsigned char* buffer, unsigned long buf_size)
      // 核心回调：PDFium 请求字体二进制数据时调用。
      // table 参数指定 TrueType 表名（0 表示请求整个字体文件）。
      this.getFontDataFnPtr = pdfium.addFunction(
        (_pThis: number, hFont: number, table: number, buffer: number, bufSize: number) => {
          return this.getFontData(hFont, table, buffer, bufSize);
        },
        'iiiiii',
      );

      // GetFaceName: unsigned long (*)(FPDF_SYSFONTINFO* pThis, void* hFont, char* buffer, unsigned long buf_size)
      // 返回字体名称。我们不跟踪字体名称，返回 0 表示未实现。
      this.getFaceNameFnPtr = pdfium.addFunction(
        (_pThis: number, _hFont: number, _buffer: number, _bufSize: number) => {
          // 不跟踪字体名称，返回 0
          return 0;
        },
        'iiiii',
      );

      // GetFontCharset: int (*)(FPDF_SYSFONTINFO* pThis, void* hFont)
      // 返回字体句柄对应的字符集编号。
      this.getFontCharsetFnPtr = pdfium.addFunction((_pThis: number, hFont: number) => {
        const handle = this.fontHandles.get(hFont);
        return handle?.charset ?? 0; // 找不到句柄时返回 0（ANSI 字符集）
      }, 'iii');

      // DeleteFont: void (*)(FPDF_SYSFONTINFO* pThis, void* hFont)
      // PDFium 使用完字体后调用，释放句柄资源。
      this.deleteFontFnPtr = pdfium.addFunction((_pThis: number, hFont: number) => {
        this.deleteFont(hFont);
      }, 'vii');

      // ========== 第三步：填充结构体字段 ==========
      pdfium.setValue(this.structPtr + OFFSET_VERSION, 1, 'i32'); // 版本号必须为 1
      pdfium.setValue(this.structPtr + OFFSET_RELEASE, this.releaseFnPtr, 'i32');
      pdfium.setValue(this.structPtr + OFFSET_ENUMFONTS, this.enumFontsFnPtr, 'i32');
      pdfium.setValue(this.structPtr + OFFSET_MAPFONT, this.mapFontFnPtr, 'i32');
      pdfium.setValue(this.structPtr + OFFSET_GETFONT, this.getFontFnPtr, 'i32');
      pdfium.setValue(this.structPtr + OFFSET_GETFONTDATA, this.getFontDataFnPtr, 'i32');
      pdfium.setValue(this.structPtr + OFFSET_GETFACENAME, this.getFaceNameFnPtr, 'i32');
      pdfium.setValue(this.structPtr + OFFSET_GETFONTCHARSET, this.getFontCharsetFnPtr, 'i32');
      pdfium.setValue(this.structPtr + OFFSET_DELETEFONT, this.deleteFontFnPtr, 'i32');

      // ========== 第四步：注册到 PDFium ==========
      module.FPDF_SetSystemFontInfo(this.structPtr);
      this.enabled = true;

      this.logger.info(
        LOG_SOURCE,
        LOG_CATEGORY,
        'Font fallback system initialized (pure TypeScript)',
        Object.keys(this.fontConfig.fonts),
      );
    } catch (error) {
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, 'Failed to initialize font fallback', error);
      // 初始化失败时清理已分配的资源，避免内存泄漏
      this.cleanup();
      throw error;
    }
  }

  /**
   * 禁用字体回退系统并释放所有 WASM 资源
   *
   * 调用后会：
   * 1. 通过 FPDF_SetSystemFontInfo(0) 从 PDFium 注销字体信息接口
   * 2. 释放 WASM 内存中的结构体
   * 3. 移除所有已注册的函数指针
   */
  disable(): void {
    if (!this.enabled || !this.module) {
      return;
    }

    // 传入 0（NULL）以注销系统字体信息接口
    this.module.FPDF_SetSystemFontInfo(0);
    this.cleanup();
    this.enabled = false;

    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'Font fallback system disabled');
  }

  /**
   * 释放所有已分配的 WASM 资源
   *
   * 包括：
   * - 释放 FPDF_SYSFONTINFO 结构体占用的 WASM 内存
   * - 从 WASM 函数表中移除所有已注册的回调函数指针
   *
   * 注意：字体数据缓存（fontCache）不会被清除，因为字体数据可跨会话复用。
   */
  private cleanup(): void {
    if (!this.module) return;

    const pdfium = this.module.pdfium;

    // 释放结构体内存
    if (this.structPtr) {
      pdfium.wasmExports.free(this.structPtr);
      this.structPtr = 0;
    }

    // 安全移除函数指针（容错处理：函数可能已被移除）
    const removeIfExists = (ptr: number) => {
      if (ptr && typeof pdfium.removeFunction === 'function') {
        try {
          pdfium.removeFunction(ptr);
        } catch {
          // 忽略错误 —— 函数可能已经被移除
        }
      }
    };

    // 逐一移除 8 个回调函数指针
    removeIfExists(this.releaseFnPtr);
    removeIfExists(this.enumFontsFnPtr);
    removeIfExists(this.mapFontFnPtr);
    removeIfExists(this.getFontFnPtr);
    removeIfExists(this.getFontDataFnPtr);
    removeIfExists(this.getFaceNameFnPtr);
    removeIfExists(this.getFontCharsetFnPtr);
    removeIfExists(this.deleteFontFnPtr);

    // 重置所有指针为 0，防止悬空引用
    this.releaseFnPtr = 0;
    this.enumFontsFnPtr = 0;
    this.mapFontFnPtr = 0;
    this.getFontFnPtr = 0;
    this.getFontDataFnPtr = 0;
    this.getFaceNameFnPtr = 0;
    this.getFontCharsetFnPtr = 0;
    this.deleteFontFnPtr = 0;
  }

  /**
   * 检查字体回退系统是否已启用
   *
   * @returns true 表示已初始化并注册到 PDFium
   */
  isEnabled(): boolean {
    return this.enabled;
  }

  /**
   * 获取字体加载统计信息（用于调试和监控）
   *
   * @returns 包含活跃句柄数、缓存数量和缓存 URL 列表的对象
   */
  getStats(): {
    /** 当前活跃的字体句柄数量 */
    handleCount: number;
    /** 字体数据缓存条目数量 */
    cacheSize: number;
    /** 已缓存字体的 URL 列表 */
    cachedUrls: string[];
  } {
    return {
      handleCount: this.fontHandles.size,
      cacheSize: this.fontCache.size,
      cachedUrls: Array.from(this.fontCache.keys()),
    };
  }

  /**
   * 预加载指定字符集的字体（可选的优化手段）
   *
   * 在实际渲染之前提前下载并缓存字体数据，避免渲染时的同步等待。
   * 适用于在打开 PDF 前已知文档语言的情况。
   *
   * @param charsets - 需要预加载的字符集数组
   */
  async preloadFonts(charsets: FontCharset[]): Promise<void> {
    // 从字符集列表中提取对应的字体 URL，过滤掉未配置的字符集
    const urls = charsets
      .map((charset) => this.getFontUrlForCharset(charset))
      .filter((url): url is string => url !== null);

    // 去重：同一字体文件可能对应多个字符集
    const uniqueUrls = [...new Set(urls)];

    // 并行下载所有字体文件
    await Promise.all(
      uniqueUrls.map(async (url) => {
        // 跳过已缓存的字体
        if (!this.fontCache.has(url)) {
          try {
            const data = await this.fetchFontAsync(url);
            if (data) {
              this.fontCache.set(url, data);
              this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Pre-loaded font: ${url}`);
            }
          } catch (error) {
            this.logger.warn(LOG_SOURCE, LOG_CATEGORY, `Failed to pre-load font: ${url}`, error);
          }
        }
      }),
    );
  }

  // ============================================================================
  // PDFium 回调函数实现
  // 以下方法由 WASM 回调桥接调用，实现 FPDF_SYSFONTINFO 接口的各个方法。
  // ============================================================================

  /**
   * MapFont 回调实现 —— PDFium 请求映射字体时调用
   *
   * 根据 PDFium 提供的字符集、字重、斜体等参数，在配置中查找最匹配的字体，
   * 并创建一个 FontHandle 作为返回值（句柄 ID 传回 PDFium）。
   *
   * 调用流程：PDFium 渲染文字 → 发现缺失字体 → 调用 MapFont → 获取句柄 ID
   *
   * @param weight - 请求的字重（100-900，如 400=Regular, 700=Bold）
   * @param bItalic - 是否需要斜体（0=否, 1=是）
   * @param charset - 字符集编号（如 128=SHIFTJIS, 134=GB2312）
   * @param pitchFamily - 字体间距和族信息（暂未使用）
   * @param face - 请求的字体名称（UTF-8 字符串）
   * @returns 字体句柄 ID，未找到匹配字体时返回 0
   */
  private mapFont(
    weight: number,
    bItalic: number,
    charset: number,
    pitchFamily: number,
    face: string,
  ): number {
    const italic = bItalic !== 0;

    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, 'MapFont called', {
      weight,
      italic,
      charset,
      pitchFamily,
      face,
    });

    // 在配置中查找最匹配的字体变体
    const result = this.findBestFontMatch(charset, weight, italic);
    if (!result) {
      this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `No font configured for charset ${charset}`);
      return 0; // 返回 0 表示无可用字体
    }

    // 创建新的字体句柄
    const handle: FontHandle = {
      id: this.nextHandleId++,
      charset,
      weight,
      italic,
      url: result.url,
      data: null, // 延迟加载：首次 GetFontData 时才真正获取数据
    };

    this.fontHandles.set(handle.id, handle);

    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      `Created font handle ${handle.id} for ${result.url} (requested: weight=${weight}, italic=${italic}, matched: weight=${result.matchedWeight}, italic=${result.matchedItalic})`,
    );

    return handle.id;
  }

  /**
   * GetFontData 回调实现 —— PDFium 请求字体二进制数据时调用
   *
   * 此方法是字体加载的核心环节，负责：
   * 1. 根据句柄 ID 查找对应的 FontHandle
   * 2. 若数据未加载，先查缓存再同步下载
   * 3. 处理 PDFium 的两阶段调用模式：
   *    - 第一次调用：buffer=null，返回所需数据大小
   *    - 第二次调用：buffer 有足够空间，将数据复制到 WASM 内存
   *
   * PDFium 获取字体数据的典型流程：
   * 1. 调用 GetFontData(hFont, 0, null, 0) → 返回字体文件总字节数
   * 2. 分配缓冲区后调用 GetFontData(hFont, 0, buffer, size) → 复制数据
   *
   * @param fontHandle - 字体句柄 ID（由 MapFont 返回）
   * @param table - TrueType 表名（0 表示请求整个字体文件）
   * @param bufferPtr - WASM 内存中的缓冲区地址（0 表示仅查询大小）
   * @param bufSize - 缓冲区大小
   * @returns 复制到缓冲区的字节数，或所需的大小；失败返回 0
   */
  private getFontData(
    fontHandle: number,
    table: number,
    bufferPtr: number,
    bufSize: number,
  ): number {
    const handle = this.fontHandles.get(fontHandle);
    if (!handle) {
      this.logger.warn(LOG_SOURCE, LOG_CATEGORY, `Unknown font handle: ${fontHandle}`);
      return 0;
    }

    // ========== 延迟加载字体数据 ==========
    // 首次请求时才真正获取字体文件内容
    if (!handle.data) {
      // 先检查缓存：同一字体文件可能被多个句柄引用
      if (this.fontCache.has(handle.url)) {
        handle.data = this.fontCache.get(handle.url)!;
      } else {
        // 同步获取字体数据（在 Web Worker 中运行，不会阻塞主线程）
        handle.data = this.fetchFontSync(handle.url);
        if (handle.data) {
          this.fontCache.set(handle.url, handle.data);
        }
      }
    }

    if (!handle.data) {
      this.logger.warn(LOG_SOURCE, LOG_CATEGORY, `Failed to load font: ${handle.url}`);
      return 0;
    }

    const fontData = handle.data;

    // 如果 table != 0，PDFium 请求的是特定的 TrueType 表（如 'cmap', 'glyf' 等）
    // 为简化实现，返回 0 让 PDFium 改为请求整个字体文件
    if (table !== 0) {
      this.logger.debug(
        LOG_SOURCE,
        LOG_CATEGORY,
        `Table ${table} requested - returning 0 to request whole file`,
      );
      return 0;
    }

    // PDFium 的两阶段调用：
    // 第一阶段：buffer 为空或大小不足，仅返回所需字节数
    if (bufferPtr === 0 || bufSize < fontData.length) {
      return fontData.length;
    }

    // 第二阶段：将字体数据复制到 PDFium 提供的 WASM 缓冲区
    if (this.module) {
      const heap = this.module.pdfium.HEAPU8;
      heap.set(fontData, bufferPtr); // 将 Uint8Array 直接写入 WASM 线性内存

      this.logger.debug(
        LOG_SOURCE,
        LOG_CATEGORY,
        `Copied ${fontData.length} bytes to buffer for handle ${fontHandle}`,
      );
    }

    return fontData.length;
  }

  /**
   * DeleteFont 回调实现 —— PDFium 使用完字体后调用
   *
   * 释放字体句柄。注意字体数据（fontCache）不会被删除，
   * 因为同一字体可能被多个文档或页面复用。
   *
   * @param fontHandle - 需要删除的字体句柄 ID
   */
  private deleteFont(fontHandle: number): void {
    const handle = this.fontHandles.get(fontHandle);
    if (handle) {
      this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Deleting font handle ${fontHandle}`);
      // 仅删除句柄映射，保留字体数据缓存以供复用
      this.fontHandles.delete(fontHandle);
    }
  }

  // ============================================================================
  // 辅助方法
  // ============================================================================

  /**
   * 根据字符集、字重和斜体属性查找最匹配的字体变体
   *
   * 查找策略：
   * 1. 优先在 fonts 映射中查找对应字符集的配置
   * 2. 若未找到，尝试使用 defaultFont 兜底
   * 3. 将配置标准化为 FontVariant 数组
   * 4. 使用 CSS 字体匹配算法选择最佳变体
   * 5. 对相对 URL 拼接 baseUrl 前缀
   *
   * @param charset - 字符集编号
   * @param requestedWeight - 请求的字重
   * @param requestedItalic - 请求是否为斜体
   * @returns 匹配结果（URL、匹配的字重和斜体属性），无匹配时返回 null
   */
  private findBestFontMatch(
    charset: number,
    requestedWeight: number,
    requestedItalic: boolean,
  ): { url: string; matchedWeight: number; matchedItalic: boolean } | null {
    const { fonts, defaultFont, baseUrl } = this.fontConfig;

    // 先查找精确字符集配置，再尝试默认字体
    const entry = fonts[charset as FontCharset] ?? defaultFont;
    if (!entry) {
      return null;
    }

    // 将 FontEntry 统一标准化为 FontVariant 数组
    const variants = this.normalizeToVariants(entry);
    if (variants.length === 0) {
      return null;
    }

    // 使用字体匹配算法选择最佳变体
    const best = this.selectBestVariant(variants, requestedWeight, requestedItalic);

    // 处理 URL：相对路径拼接 baseUrl
    let url = best.url;
    if (
      baseUrl &&
      !url.startsWith('http://') &&
      !url.startsWith('https://') &&
      !url.startsWith('/')
    ) {
      url = `${baseUrl}/${url}`;
    }

    return {
      url,
      matchedWeight: best.weight ?? 400,
      matchedItalic: best.italic ?? false,
    };
  }

  /**
   * 将 FontEntry 统一标准化为 FontVariant 数组
   *
   * 由于 FontEntry 是联合类型（string | FontVariant | FontVariant[]），
   * 此方法将其统一转换为 FontVariant[]，方便后续处理。
   * 字符串 URL 默认 weight=400、italic=false。
   *
   * @param entry - 字体配置条目（任意形式）
   * @returns 标准化后的 FontVariant 数组
   */
  private normalizeToVariants(entry: FontEntry): FontVariant[] {
    if (typeof entry === 'string') {
      // 简单字符串 → 单个默认变体
      return [{ url: entry, weight: 400, italic: false }];
    }
    if (Array.isArray(entry)) {
      // 数组 → 补全缺失的默认值
      return entry.map((v) => ({
        url: v.url,
        weight: v.weight ?? 400,
        italic: v.italic ?? false,
      }));
    }
    // 单个 FontVariant 对象 → 补全默认值
    return [{ url: entry.url, weight: entry.weight ?? 400, italic: entry.italic ?? false }];
  }

  /**
   * 使用 CSS 字体匹配算法从候选变体中选择最佳匹配
   *
   * 算法步骤（参考 CSS Fonts Module Level 3）：
   * 1. 若只有一个变体，直接返回
   * 2. 优先筛选斜体属性匹配的候选
   * 3. 在候选中寻找字重距离最近的变体
   * 4. 字重相同时的决胜规则：
   *    - 请求字重 ≥ 500 时偏好更粗的字体
   *    - 请求字重 < 500 时偏好更细的字体
   *
   * @param variants - 候选字体变体数组
   * @param requestedWeight - 请求的字重
   * @param requestedItalic - 请求是否为斜体
   * @returns 最佳匹配的字体变体
   */
  private selectBestVariant(
    variants: FontVariant[],
    requestedWeight: number,
    requestedItalic: boolean,
  ): FontVariant {
    // 只有一个候选时直接返回，无需匹配
    if (variants.length === 1) {
      return variants[0];
    }

    // 第一步：筛选斜体属性匹配的候选
    const italicMatches = variants.filter((v) => (v.italic ?? false) === requestedItalic);
    // 若有斜体匹配的候选则使用，否则回退到全部候选
    const candidates = italicMatches.length > 0 ? italicMatches : variants;

    // 第二步：在候选中寻找字重距离最近的变体
    let bestMatch = candidates[0];
    let bestDistance = Math.abs((bestMatch.weight ?? 400) - requestedWeight);

    for (const variant of candidates) {
      const variantWeight = variant.weight ?? 400;
      const distance = Math.abs(variantWeight - requestedWeight);

      if (distance < bestDistance) {
        // 找到更近的字重
        bestMatch = variant;
        bestDistance = distance;
      } else if (distance === bestDistance) {
        // 第三步：字重相同时的决胜规则
        const currentWeight = bestMatch.weight ?? 400;
        if (requestedWeight >= 500) {
          // 请求字重偏粗（≥ 500）时，偏好更粗的变体
          if (variantWeight > currentWeight) {
            bestMatch = variant;
          }
        } else {
          // 请求字重偏细（< 500）时，偏好更细的变体
          if (variantWeight < currentWeight) {
            bestMatch = variant;
          }
        }
      }
    }

    return bestMatch;
  }

  /**
   * 获取指定字符集对应的字体 URL（向后兼容的辅助方法）
   *
   * @param charset - 字符集编号
   * @returns 字体 URL，未配置时返回 null
   */
  private getFontUrlForCharset(charset: number): string | null {
    const result = this.findBestFontMatch(charset, 400, false);
    return result?.url ?? null;
  }

  /**
   * 同步获取字体数据
   *
   * 此方法在 PDFium 的 GetFontData 回调中被调用。由于 PDFium 的回调机制
   * 不支持异步操作，必须使用同步方式获取字体数据。
   *
   * 加载策略（按优先级）：
   * 1. 若配置了自定义 fontLoader（如 Node.js 的 fs.readFileSync），优先使用
   * 2. 否则使用浏览器的同步 XMLHttpRequest（第三个参数 false）
   *
   * 注意：同步 XMLHttpRequest 会阻塞当前线程，但由于此代码在 Web Worker 中运行，
   * 不会影响主线程的响应性。
   *
   * @param pathOrUrl - 字体文件的 URL 或路径
   * @returns 字体二进制数据，加载失败时返回 null
   */
  private fetchFontSync(pathOrUrl: string): Uint8Array | null {
    this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Fetching font synchronously: ${pathOrUrl}`);

    // 优先使用自定义字体加载器（适用于 Node.js 环境或自定义数据源）
    if (this.fontConfig.fontLoader) {
      try {
        const data = this.fontConfig.fontLoader(pathOrUrl);
        if (data) {
          this.logger.info(
            LOG_SOURCE,
            LOG_CATEGORY,
            `Loaded font via custom loader: ${pathOrUrl} (${data.length} bytes)`,
          );
        } else {
          this.logger.warn(
            LOG_SOURCE,
            LOG_CATEGORY,
            `Custom font loader returned null for: ${pathOrUrl}`,
          );
        }
        return data;
      } catch (error) {
        this.logger.error(
          LOG_SOURCE,
          LOG_CATEGORY,
          `Error in custom font loader: ${pathOrUrl}`,
          error,
        );
        return null;
      }
    }

    // 默认方式：浏览器同步 XMLHttpRequest
    try {
      const xhr = new XMLHttpRequest();
      xhr.open('GET', pathOrUrl, false); // false = 同步请求
      xhr.responseType = 'arraybuffer';
      xhr.send();

      if (xhr.status === 200) {
        const data = new Uint8Array(xhr.response as ArrayBuffer);
        this.logger.info(
          LOG_SOURCE,
          LOG_CATEGORY,
          `Loaded font: ${pathOrUrl} (${data.length} bytes)`,
        );
        return data;
      } else {
        this.logger.error(
          LOG_SOURCE,
          LOG_CATEGORY,
          `Failed to load font: ${pathOrUrl} (HTTP ${xhr.status})`,
        );
        return null;
      }
    } catch (error) {
      this.logger.error(LOG_SOURCE, LOG_CATEGORY, `Error fetching font: ${pathOrUrl}`, error);
      return null;
    }
  }

  /**
   * 异步获取字体数据（用于预加载）
   *
   * 此方法仅在 preloadFonts 中使用，不涉及 PDFium 回调。
   * 优先使用自定义 fontLoader，否则使用浏览器的 Fetch API。
   *
   * @param pathOrUrl - 字体文件的 URL 或路径
   * @returns 字体二进制数据，加载失败时返回 null
   */
  private async fetchFontAsync(pathOrUrl: string): Promise<Uint8Array | null> {
    // 优先使用自定义字体加载器（fontLoader 本身是同步的，包装为异步接口）
    if (this.fontConfig.fontLoader) {
      try {
        return this.fontConfig.fontLoader(pathOrUrl);
      } catch {
        return null;
      }
    }

    // 默认方式：浏览器 Fetch API（异步）
    try {
      const response = await fetch(pathOrUrl);
      if (response.ok) {
        const buffer = await response.arrayBuffer();
        return new Uint8Array(buffer);
      }
      return null;
    } catch {
      return null;
    }
  }
}

/**
 * 创建 Node.js 文件系统字体加载器
 *
 * 【设计原因】
 * 此函数将 fs 和 path 模块作为参数传入，而非直接 import，是为了避免
 * 在浏览器打包时将 Node.js 模块引入产物中。这种依赖注入模式使得同一代码
 * 可以在浏览器和 Node.js 环境中安全使用。
 *
 * @param fs - Node.js fs 模块或兼容对象（需提供 readFileSync 方法）
 * @param path - Node.js path 模块或兼容对象（需提供 join 方法）
 * @param basePath - 字体文件所在的基础目录路径
 * @returns 可用于 FontFallbackConfig.fontLoader 的字体加载函数
 *
 * @example 使用示例：
 * ```typescript
 * import { readFileSync } from 'fs';
 * import { join } from 'path';
 * import { createNodeFontLoader, FontCharset } from '.';
 *
 * const fontLoader = createNodeFontLoader(
 *   { readFileSync },
 *   { join },
 *   '/path/to/fonts'
 * );
 *
 * const fontFallback = {
 *   fonts: {
 *     [FontCharset.SHIFTJIS]: 'NotoSansJP-Regular.otf',
 *   },
 *   fontLoader,
 * };
 * ```
 */
export function createNodeFontLoader(
  fs: { readFileSync: (path: string) => Uint8Array | ArrayBufferLike },
  path: { join: (...paths: string[]) => string },
  basePath: string,
): FontLoader {
  return (fontPath: string): Uint8Array | null => {
    try {
      const fullPath = path.join(basePath, fontPath);
      const data = fs.readFileSync(fullPath);
      // Node.js 的 Buffer 是 Uint8Array 的子类，但某些环境下 readFileSync
      // 可能返回 ArrayBuffer，需要统一转换为 Uint8Array
      if (data instanceof Uint8Array) {
        return data;
      }
      return new Uint8Array(data);
    } catch {
      return null;
    }
  };
}
