/**
 * @file 瓦片渲染插件 — 工具函数
 *
 * 本文件提供瓦片网格计算的核心算法 `calculateTilesForPage`。
 *
 * 核心算法思路：
 * 1. 在屏幕像素空间中，根据 tileSize 和 overlapPx 构建等间距的网格
 * 2. 通过旋转反变换确定当前可见区域在未旋转坐标系中的位置
 * 3. 仅生成覆盖可见区域（含 extraRings 扩展）的瓦片，跳过不可见瓦片
 * 4. 边缘瓦片裁剪到页面边界，避免越界
 * 5. 每个瓦片同时记录屏幕坐标（用于 CSS 定位）和 PDF 坐标（用于渲染裁剪）
 */

import { Rect, restoreRect, transformSize } from '../models';
import { CalculateTilesForPageOptions, Tile } from './types';

/**
 * 为单个 PDF 页面计算瓦片网格。
 *
 * 构建一个相邻瓦片之间重叠 `overlapPx`（屏幕像素）的网格。
 * 内部瓦片保持完整的 `tileSize` 大小，边缘瓦片裁剪到页面边界。
 * 所有屏幕空间的数值取整为整数，避免子像素缝隙。
 *
 * @param options - 计算参数（瓦片大小、重叠、扩展圈数、缩放、旋转、页面信息、可见区域）
 * @returns 该页面在可见区域内的所有瓦片描述数组
 */
export function calculateTilesForPage({
  tileSize = 768,
  overlapPx = 2.5,
  extraRings = 0,
  scale,
  rotation,
  page,
  metric,
}: CalculateTilesForPageOptions): Tile[] {
  /* ---- 第一阶段：在屏幕像素空间中计算页面和网格参数 ---- */

  const pageW = page.size.width * scale; // 页面在屏幕上的宽度（像素）
  const pageH = page.size.height * scale; // 页面在屏幕上的高度（像素）

  // 瓦片之间的步进距离：瓦片大小减去重叠区域
  // 例如 tileSize=768, overlapPx=2.5 → step=765.5，每个瓦片向前移动 765.5px
  const step = tileSize - overlapPx;

  /* ---- 第二阶段：确定可见区域在未旋转坐标系中的位置 ---- */

  // 计算旋转后的容器尺寸（用于坐标反变换）
  const containerSize = transformSize(page.size, rotation, scale);

  // 滚动插件提供的可见矩形（已经过旋转变换后的屏幕坐标）
  const rotatedVisRect: Rect = {
    origin: { x: metric.scaled.pageX, y: metric.scaled.pageY },
    size: { width: metric.scaled.visibleWidth, height: metric.scaled.visibleHeight },
  };

  // 将旋转后的可见矩形反变换回未旋转的页面坐标系
  // 这样瓦片计算可以始终在未旋转空间中进行，渲染时再应用旋转
  const unrotatedVisRect = restoreRect(containerSize, rotatedVisRect, rotation, 1);

  // 提取可见区域的四条边界
  const visLeft = unrotatedVisRect.origin.x;
  const visTop = unrotatedVisRect.origin.y;
  const visRight = visLeft + unrotatedVisRect.size.width;
  const visBottom = visTop + unrotatedVisRect.size.height;

  /* ---- 第三阶段：计算网格的行列范围（仅覆盖可见区域 + extraRings） ---- */

  // 最大列/行索引：确保最右/最下的瓦片不超出页面边界
  const maxCol = Math.floor((pageW - 1) / step);
  const maxRow = Math.floor((pageH - 1) / step);

  // 可见区域对应的起止列/行，向外扩展 extraRings 圈用于预渲染
  const startCol = Math.max(0, Math.floor(visLeft / step) - extraRings);
  const endCol = Math.min(maxCol, Math.floor((visRight - 1) / step) + extraRings);
  const startRow = Math.max(0, Math.floor(visTop / step) - extraRings);
  const endRow = Math.min(maxRow, Math.floor((visBottom - 1) / step) + extraRings);

  /* ---- 第四阶段：构建瓦片对象 ---- */

  const tiles: Tile[] = [];

  for (let col = startCol; col <= endCol; col++) {
    // 当前列在屏幕上的 x 坐标（像素）
    const xScreen = col * step; // 整数像素
    // 当前列的瓦片宽度：取 tileSize 与剩余页面宽度的较小值（边缘裁剪）
    const wScreen = Math.min(tileSize, pageW - xScreen);

    // 转换为 PDF 点坐标：屏幕像素除以缩放比例
    const xPage = xScreen / scale; // 可能是小数
    const wPage = wScreen / scale;

    for (let row = startRow; row <= endRow; row++) {
      // 当前行在屏幕上的 y 坐标（像素）
      const yScreen = row * step;
      // 当前行的瓦片高度：取 tileSize 与剩余页面高度的较小值
      const hScreen = Math.min(tileSize, pageH - yScreen);

      // 转换为 PDF 点坐标
      const yPage = yScreen / scale;
      const hPage = hScreen / scale;

      // 创建瓦片描述对象
      tiles.push({
        // 唯一 ID：包含页码、缩放、位置和尺寸信息，保证同一位置相同参数的瓦片 ID 一致
        id: `p${page.index}-${scale}-x${xScreen}-y${yScreen}-w${wScreen}-h${hScreen}`,
        col,
        row,
        // PDF 页面坐标（点），用于渲染时指定裁剪区域
        pageRect: { origin: { x: xPage, y: yPage }, size: { width: wPage, height: hPage } },
        // 屏幕坐标（像素），用于 CSS 绝对定位
        screenRect: {
          origin: { x: xScreen, y: yScreen },
          size: { width: wScreen, height: hScreen },
        },
        status: 'queued', // 初始状态为排队等待渲染
        srcScale: scale, // 记录渲染时的缩放比例
        isFallback: false, // 新计算的瓦片不是降级瓦片
      });
    }
  }

  return tiles;
}
