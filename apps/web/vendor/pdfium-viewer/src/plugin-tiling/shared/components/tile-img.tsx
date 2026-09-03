/**
 * @file 瓦片渲染插件 — TileImg 组件
 *
 * TileImg 是单个瓦片的图片渲染组件，负责：
 * 1. 发起瓦片渲染请求（通过 tiling 插件的 renderTile 能力）
 * 2. 将渲染结果（Blob）转换为 Object URL 并显示为 <img>
 * 3. 管理瓦片的 CSS 定位和缩放（支持缩放比例与渲染时不同的场景）
 * 4. 在组件卸载时清理 Object URL 或取消正在进行的渲染任务
 *
 * 渲染策略：
 * - 每个瓦片只触发一次渲染请求（通过 tile.id 作为 useEffect 依赖）
 * - 渲染完成后将 Blob 转为 Object URL 赋给 <img> 的 src
 * - 图片加载完成后立即释放 Object URL（避免内存泄漏）
 * - 组件卸载时：如果图片已生成则释放 URL，如果渲染仍在进行则取消任务
 *
 * 缩放适配：
 * - relativeScale = 当前 scale / tile.srcScale
 * - 当用户在渲染完成后缩放时，CSS 尺寸按 relativeScale 缩放，
 *   直到新缩放级别的瓦片渲染完成后替换
 */

import { ignore, PdfErrorCode } from '../../../models';
import { Tile } from '../..';
import { useEffect, useMemo, useRef, useState } from '../../../framework';

import { useTilingCapability } from '../hooks/use-tiling';

/**
 * TileImg 组件的属性接口。
 */
interface TileImgProps {
  /** 文档唯一标识 */
  documentId: string;
  /** 页码索引（0-based） */
  pageIndex: number;
  /** 瓦片描述对象，包含位置、尺寸、渲染状态等信息 */
  tile: Tile;
  /** 设备像素比，用于高分辨率渲染 */
  dpr: number;
  /** 当前文档的缩放比例，用于计算 CSS 定位 */
  scale: number;
}

/**
 * 单个瓦片图片组件。
 *
 * 管理瓦片从渲染请求到图片显示的完整生命周期。
 * 组件通过 useEffect 触发渲染，渲染完成后通过 Object URL 显示图片。
 *
 * @param props - 组件属性
 * @returns 图片元素，渲染未完成时返回 null
 */
export function TileImg({ documentId, pageIndex, tile, dpr, scale }: TileImgProps) {
  // 获取瓦片插件能力接口
  const { provides: tilingCapability } = useTilingCapability();
  // 创建文档级别的操作 scope（绑定 documentId）
  const scope = useMemo(
    () => tilingCapability?.forDocument(documentId),
    [tilingCapability, documentId],
  );

  // 图片的 Object URL 状态，渲染完成后设置
  const [url, setUrl] = useState<string>();
  // 使用 ref 跟踪 URL，确保在 useEffect 清理函数中可以正确访问
  const urlRef = useRef<string | null>(null);

  // 计算相对缩放比例：当前显示 scale 与瓦片渲染时 scale 的比值
  // 用于在缩放变化时临时调整瓦片的 CSS 尺寸
  const relativeScale = scale / tile.srcScale;

  /**
   * 核心渲染副作用：发起瓦片渲染请求。
   *
   * - 仅在 tile.id 变化时触发（id 包含 scale、位置等信息，变化意味着需要重新渲染）
   * - 如果瓦片已经渲染完成（ready）且 URL 已存在，则跳过
   * - 渲染完成后将 Blob 转为 Object URL 并更新状态
   * - 清理函数负责释放 URL 或取消渲染任务
   */
  useEffect(() => {
    // 如果已经渲染完成且 URL 已存在，跳过重复渲染
    if (tile.status === 'ready' && urlRef.current) return;
    if (!scope) return;

    // 发起渲染请求
    const task = scope.renderTile({ pageIndex, tile, dpr });

    // 渲染成功回调：创建 Object URL 并更新状态
    task.wait((blob) => {
      const objectUrl = URL.createObjectURL(blob);
      urlRef.current = objectUrl;
      setUrl(objectUrl);
    }, ignore); // 忽略渲染失败

    // 清理函数：组件卸载或 tile.id 变化时执行
    return () => {
      if (urlRef.current) {
        // URL 已存在，释放 Object URL 避免内存泄漏
        URL.revokeObjectURL(urlRef.current);
        urlRef.current = null;
      } else {
        // URL 尚未生成（渲染仍在进行），取消渲染任务
        task.abort({
          code: PdfErrorCode.Cancelled,
          message: 'canceled render task',
        });
      }
    };
  }, [scope, pageIndex, tile.id]); // tile.id 包含 scale 信息，缩放变化时 id 也变化

  /**
   * 图片加载完成回调。
   * 在浏览器完成图片解码后释放 Object URL（图片数据已在内存中）。
   */
  const handleImageLoad = () => {
    if (urlRef.current) {
      URL.revokeObjectURL(urlRef.current);
      urlRef.current = null;
    }
  };

  // 渲染未完成时不显示任何内容（可替换为占位符）
  if (!url) return null;

  return (
    <img
      src={url}
      className="pdfium-page-image"
      onLoad={handleImageLoad}
      style={{
        position: 'absolute',
        // 通过 relativeScale 调整 CSS 定位，支持缩放比例变化的临时适配
        left: tile.screenRect.origin.x * relativeScale,
        top: tile.screenRect.origin.y * relativeScale,
        width: tile.screenRect.size.width * relativeScale,
        height: tile.screenRect.size.height * relativeScale,
        display: 'block',
      }}
    />
  );
}
