/**
 * @file BMP 编码器 — 将原始 RGBA 像素数据转换为无压缩 BMP Blob
 *
 * 这是最快的图像编码路径：无需 Canvas、无需压缩、无需 Worker 通信，
 * 只需在内存中拼接 66 字节的 BMP 文件头 + 原始 RGBA 像素数据。
 *
 * 技术细节：
 *   - 使用 BI_BITFIELDS 压缩模式 + RGBA 通道掩码，避免逐像素字节交换
 *   - 使用负高度值表示自顶向下的行序（top-down），避免行翻转
 *   - 生成的 BMP 文件可被所有现代浏览器原生解码显示
 *
 * 性能对比：
 *   PNG 编码（Canvas.toBlob）需要数十毫秒，
 *   BMP 编码（本函数）仅需微秒级——快 100 倍以上。
 */

/**
 * 将 RGBA 像素数据转换为无压缩 BMP Blob
 *
 * @param rgba   - RGBA 像素数据（每像素 4 字节，从上到下、从左到右排列）
 * @param width  - 图像宽度（像素）
 * @param height - 图像高度（像素）
 * @returns 包含 BMP 文件数据的 Blob（MIME 类型为 image/bmp）
 */
export function rgbaToBmpBlob(rgba: Uint8ClampedArray, width: number, height: number): Blob {
  const pixels = width * height * 4; // 像素数据总字节数
  const headerLength = 66; // BMP 文件头 + DIB 头 + 通道掩码 = 66 字节

  // 小端序 32 位整数编码辅助函数
  const le32 = (v: number) => [v & 0xff, (v >>> 8) & 0xff, (v >>> 16) & 0xff, (v >>> 24) & 0xff];

  // BMP 文件头（14 字节）+ DIB 头（40 字节）+ 通道掩码（12 字节）= 66 字节
  // prettier-ignore
  const header = new Uint8Array([
    0x42, 0x4D,                     // 文件签名 'BM'
    ...le32(headerLength + pixels), // 文件总大小（头 + 像素数据）
    0, 0, 0, 0,                     // 保留字段
    headerLength, 0, 0, 0,          // 像素数据偏移量（从头开始算）
    40, 0, 0, 0,                    // DIB 头大小（BITMAPINFOHEADER = 40）
    ...le32(width),                 // 图像宽度
    ...le32(-height),               // 图像高度（负值 = 自顶向下行序，避免行翻转）
    1, 0,                           // 颜色平面数 = 1
    32, 0,                          // 每像素位数 = 32（RGBA 各 8 位）
    3, 0, 0, 0,                     // 压缩方式 = BI_BITFIELDS（3 = 使用通道掩码）
    ...le32(pixels),                // 像素数据大小
    0, 0, 0, 0,                     // 水平分辨率（像素/米，0 = 不指定）
    0, 0, 0, 0,                     // 垂直分辨率
    0, 0, 0, 0,                     // 调色板颜色数
    0, 0, 0, 0,                     // 重要颜色数
    0xFF, 0, 0, 0,                  // 红色通道掩码（第 0 字节）
    0, 0xFF, 0, 0,                  // 绿色通道掩码（第 1 字节）
    0, 0, 0xFF, 0,                  // 蓝色通道掩码（第 2 字节）
  ]);

  // 拼接头 + 像素数据，生成 Blob
  return new Blob(
    [header, new Uint8Array(rgba.buffer as ArrayBuffer, rgba.byteOffset, rgba.byteLength)],
    { type: 'image/bmp' },
  );
}
