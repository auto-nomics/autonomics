/**
 * @file helper.ts
 * @description PDFium WASM 辅助工具函数
 *
 * 本文件提供了一组与 PDFium WASM 堆内存交互的工具函数，
 * 以及 PDF 文档相关的计算辅助函数。主要包括：
 *
 * 1. WASM 堆内存读取：
 *    - readString：从 WASM 堆中读取字符串（支持自动扩展缓冲区）
 *    - readArrayBuffer：从 WASM 堆中读取二进制数据
 *
 * 2. PDF 元数据验证：
 *    - isValidCustomKey：验证自定义 PDF Info 字典键名是否合法
 *
 * 3. 表单渲染计算：
 *    - computeFormDrawParams：计算表单控件在页面上的绘制参数
 *      （考虑了页面旋转、缩放矩阵等变换）
 */

import { Matrix, Rotation, Rect, Size } from '../../models';
import { PdfiumRuntimeMethods, PdfiumModule } from '../../pdfium';

/**
 * 从 WASM 堆内存中读取字符串
 *
 * 采用"两遍读取"策略处理长度未知的字符串：
 *   1. 先分配默认大小的缓冲区，调用 readChars 获取实际长度
 *   2. 如果实际长度超过默认缓冲区，则重新分配足够大的缓冲区再次读取
 *   3. 读取完成后释放堆内存
 *
 * 设计原因：PDFium 的许多 API（如 FPDF_GetMetaText）要求调用者提供缓冲区，
 * 并返回实际需要的字节数。如果缓冲区不够大，需要重新分配。这种模式在 C 语言
 * 的 PDFium API 中非常常见。
 *
 * @param wasmModule - PDFium WASM 模块实例，提供堆内存访问和 malloc/free
 * @param readChars - 读取字符的函数，签名 (buffer, bufferLength) => 实际长度
 *                    第一次调用时将实际长度写入返回值；
 *                    当 bufferLength >= 实际长度时，将字符数据写入 buffer
 * @param parseChars - 解析字符的函数，从堆地址读取并转换为 JS 字符串
 * @param defaultLength - 初始缓冲区大小（字节数），默认 100
 *                        对于已知长度较短的字符串可以设置更小的值以节省内存
 * @returns 从 WASM 堆读取并解析出的字符串
 *
 * @public
 */
export function readString(
  wasmModule: PdfiumRuntimeMethods & PdfiumModule,
  readChars: (buffer: number, bufferLength: number) => number,
  parseChars: (buffer: number) => string,
  defaultLength: number = 100,
): string {
  // 第一遍：分配默认大小的缓冲区并清零
  let buffer = wasmModule.wasmExports.malloc(defaultLength);
  for (let i = 0; i < defaultLength; i++) {
    wasmModule.HEAP8[buffer + i] = 0;
  }

  // 调用读取函数，获取实际需要的字节数
  const actualLength = readChars(buffer, defaultLength);
  let str: string;

  if (actualLength > defaultLength) {
    // 实际长度超过默认缓冲区 → 需要重新分配
    wasmModule.wasmExports.free(buffer);
    buffer = wasmModule.wasmExports.malloc(actualLength);
    // 清零新缓冲区（防止残留数据干扰字符串解析）
    for (let i = 0; i < actualLength; i++) {
      wasmModule.HEAP8[buffer + i] = 0;
    }
    // 用足够大的缓冲区再次读取
    readChars(buffer, actualLength);
    str = parseChars(buffer);
  } else {
    // 默认缓冲区足够大，直接解析
    str = parseChars(buffer);
  }

  // 释放堆内存，防止内存泄漏
  wasmModule.wasmExports.free(buffer);

  return str;
}

/**
 * 从 WASM 堆内存中读取二进制数据（ArrayBuffer）
 *
 * 与 readString 类似的模式，但返回的是原始二进制数据而非字符串。
 * 适用于读取 PDF 中的嵌入文件、图片等二进制内容。
 *
 * 读取步骤：
 *   1. 先传入空缓冲区（buffer=0, bufferLength=0）获取实际需要的字节数
 *   2. 分配对应大小的 WASM 堆内存
 *   3. 调用读取函数将数据写入堆内存
 *   4. 逐字节复制到 JS ArrayBuffer
 *   5. 释放 WASM 堆内存
 *
 * @param wasmModule - PDFium WASM 模块实例
 * @param readChars - 读取函数，签名 (buffer, bufferLength) => 实际字节数
 *                    当 buffer=0 时返回所需大小；否则将数据写入 buffer
 * @returns 包含从 WASM 堆读取的二进制数据的 ArrayBuffer
 *
 * @public
 */
export function readArrayBuffer(
  wasmModule: PdfiumRuntimeMethods & PdfiumModule,
  readChars: (buffer: number, bufferLength: number) => number,
): ArrayBuffer {
  // 先查询实际需要多少字节
  const bufferSize = readChars(0, 0);

  // 在 WASM 堆中分配对应大小的内存
  const bufferPtr = wasmModule.wasmExports.malloc(bufferSize);

  // 将数据读取到 WASM 堆中
  readChars(bufferPtr, bufferSize);

  // 从 WASM 堆逐字节复制到 JS ArrayBuffer
  const arrayBuffer = new ArrayBuffer(bufferSize);
  const view = new DataView(arrayBuffer);

  for (let i = 0; i < bufferSize; i++) {
    view.setInt8(i, wasmModule.getValue(bufferPtr + i, 'i8'));
  }

  // 释放 WASM 堆内存
  wasmModule.wasmExports.free(bufferPtr);

  return arrayBuffer;
}

/**
 * PDF Info 字典中的保留键名集合
 *
 * 这些键名由 PDF 规范定义，不能作为自定义元数据键使用：
 *   - Title：文档标题
 *   - Author：文档作者
 *   - Subject：文档主题
 *   - Keywords：文档关键词
 *   - Producer：生成该 PDF 的应用程序
 *   - Creator：创建原始文档的应用程序
 *   - CreationDate：创建日期
 *   - ModDate：修改日期
 *   - Trapped：是否已添加陷印（trapping）信息
 */
const RESERVED_INFO_KEYS = new Set([
  'Title',
  'Author',
  'Subject',
  'Keywords',
  'Producer',
  'Creator',
  'CreationDate',
  'ModDate',
  'Trapped',
]);

/**
 * 验证自定义 PDF Info 字典键名是否合法
 *
 * 根据 PDF 规范中的 Name 对象规则进行验证，但做了适当的简化：
 *   - 键名不能为空
 *   - 键名长度不超过 127 个字符
 *   - 不能与 PDF 规范保留的 Info 字段重名
 *   - 不能以斜杠 '/' 开头（PDF Name 对象的命名约定）
 *   - 只允许 ASCII 可打印字符（0x20-0x7e），避免编码问题
 *
 * @param key - 待验证的自定义键名
 * @returns true 表示键名合法，false 表示不合法
 */
export function isValidCustomKey(key: string): boolean {
  // 空键名或超长键名（>127字符）无效
  if (!key || key.length > 127) return false;
  // 不能与 PDF 规范保留字段重名
  if (RESERVED_INFO_KEYS.has(key)) return false;
  // 不能以斜杠开头（PDF Name 对象语法）
  if (key[0] === '/') return false;
  // 只允许 ASCII 可打印字符（0x20 空格 到 0x7e 波浪号）
  // 如需支持 Unicode 键名，可以放宽此限制
  for (let i = 0; i < key.length; i++) {
    const c = key.charCodeAt(i);
    if (c < 0x20 || c > 0x7e) return false;
  }
  return true;
}

/**
 * 表单绘制参数
 *
 * 描述了 PDF 表单控件在渲染时的位置和缩放信息。
 * 这些参数需要考虑页面的旋转角度和渲染矩阵的缩放变换。
 */
interface FormDrawParams {
  /** 表单位图在目标坐标系中的起始 X 偏移（像素） */
  startX: number;
  /** 表单位图在目标坐标系中的起始 Y 偏移（像素） */
  startY: number;
  /** 表单位图的宽度（像素），经过缩放变换后的值 */
  formsWidth: number;
  /** 表单位图的高度（像素），经过缩放变换后的值 */
  formsHeight: number;
  /** 渲染矩阵的 X 轴缩放因子（由矩阵的 a,b 分量计算） */
  scaleX: number;
  /** 渲染矩阵的 Y 轴缩放因子（由矩阵的 c,d 分量计算） */
  scaleY: number;
}

/**
 * 计算表单控件在渲染时的绘制参数
 *
 * 此函数根据页面渲染矩阵、裁剪区域、页面尺寸和旋转角度，
 * 计算出表单控件的精确绘制位置和尺寸。
 *
 * 核心算法：
 *   1. 缩放因子提取：使用 Math.hypot 从仿射变换矩阵中提取各轴的缩放比例。
 *      这是因为渲染矩阵可能包含旋转和缩放的组合，Math.hypot(a, b) 能
 *      准确计算出 X 轴方向的缩放幅度，不受旋转分量的影响。
 *
 *   2. 宽高交换处理：当旋转角度为奇数（90° 或 270°）时，页面的宽高会互换。
 *      例如一个 A4 纵向页面旋转 90° 后变成横向，此时表单位图的尺寸计算
 *      需要用 pageHeight 乘以 scaleX 得到宽度。
 *
 *   3. 起始位置计算：根据旋转角度的不同，裁剪区域（rect）到表单位图的
 *      偏移计算方式不同。每次旋转都会改变坐标系的原点和方向：
 *        - 0°：正常方向，原点在左下
 *        - 90°：顺时针旋转，原点移到左上
 *        - 180°：完全倒置，原点在右上
 *        - 270°：逆时针旋转，原点在右下
 *
 * @param matrix - 页面渲染矩阵（仿射变换），包含 a,b,c,d,e,f 六个分量
 *                 其中 (a,b) 控制 X 轴变换，(c,d) 控制 Y 轴变换
 * @param rect - 要绘制的裁剪区域（PDF 坐标系，原点在左下角）
 * @param pageSize - 页面的原始尺寸（未经旋转）
 * @param rotation - 页面旋转角度（0°/90°/180°/270°）
 * @returns 表单绘制所需的全部参数
 */
export function computeFormDrawParams(
  matrix: Matrix,
  rect: Rect,
  pageSize: Size,
  rotation: Rotation,
): FormDrawParams {
  // 提取裁剪区域的边界坐标（PDF 坐标系：原点在左下角）
  const rectLeft = rect.origin.x;
  const rectBottom = rect.origin.y;
  const rectRight = rectLeft + rect.size.width;
  const rectTop = rectBottom + rect.size.height;
  const pageWidth = pageSize.width;
  const pageHeight = pageSize.height;

  // 从仿射变换矩阵中提取缩放因子
  // 矩阵形式：[a b] => X轴缩放 = hypot(a, b)，即向量的模
  //          [c d] => Y轴缩放 = hypot(c, d)
  // 这种方法能正确处理同时包含旋转和缩放的矩阵
  const scaleX = Math.hypot(matrix.a, matrix.b);
  const scaleY = Math.hypot(matrix.c, matrix.d);

  // 当旋转角度为奇数（90° 或 270°）时，页面宽高互换
  const swap = (rotation & 1) === 1;

  // 根据是否交换来计算表单位图的实际像素尺寸
  // Math.max(1, ...) 确保最小尺寸为 1 像素，避免零尺寸导致的问题
  const formsWidth = swap
    ? Math.max(1, Math.round(pageHeight * scaleX))
    : Math.max(1, Math.round(pageWidth * scaleX));
  const formsHeight = swap
    ? Math.max(1, Math.round(pageWidth * scaleY))
    : Math.max(1, Math.round(pageHeight * scaleY));

  // 根据旋转角度计算表单位图在目标坐标系中的起始位置
  // 不同旋转角度下，坐标系的原点和方向不同，因此偏移计算也不同
  let startX: number;
  let startY: number;
  switch (rotation) {
    case Rotation.Degree0:
      // 无旋转：直接取负偏移（将裁剪区域左下角对齐到原点）
      startX = -Math.round(rectLeft * scaleX);
      startY = -Math.round(rectBottom * scaleY);
      break;
    case Rotation.Degree90:
      // 顺时针旋转 90°：原点移到左上角，需要从顶部（rectTop）计算偏移
      startX = Math.round((rectTop - pageHeight) * scaleX);
      startY = -Math.round(rectLeft * scaleY);
      break;
    case Rotation.Degree180:
      // 旋转 180°：原点移到右上角，需要从右边和顶部计算偏移
      startX = Math.round((rectRight - pageWidth) * scaleX);
      startY = Math.round((rectTop - pageHeight) * scaleY);
      break;
    case Rotation.Degree270:
      // 逆时针旋转 270°（即顺时针 90°）：原点移到右下角
      startX = -Math.round(rectBottom * scaleX);
      startY = Math.round((rectRight - pageWidth) * scaleY);
      break;
    default:
      // 未知旋转角度，回退到 0° 的计算方式
      startX = -Math.round(rectLeft * scaleX);
      startY = -Math.round(rectBottom * scaleY);
      break;
  }

  return { startX, startY, formsWidth, formsHeight, scaleX, scaleY };
}
