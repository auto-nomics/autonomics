/**
 * @file 瓦片渲染插件 — TilingLayer 组件
 *
 * TilingLayer 是瓦片图层容器组件，负责：
 * 1. 订阅瓦片渲染事件，获取指定页面的瓦片列表
 * 2. 计算当前实际使用的缩放比例（支持外部覆盖）
 * 3. 渲染所有瓦片的 TileImg 子组件
 *
 * 渲染逻辑：
 * - 组件挂载时订阅 tiling 插件的事件，每次瓦片列表更新时重新渲染
 * - 组件卸载时自动取消订阅（通过 useEffect 的清理函数）
 * - 每个 TileImg 接收瓦片信息和缩放比例，独立管理自己的渲染生命周期
 *
 * 状态管理：
 * - tiles: 本地 state，存储当前页面的瓦片列表，由事件回调更新
 * - actualScale: 通过 useMemo 计算，优先使用外部传入的 scale，否则使用文档的 scale
 */

import { Tile } from '../..';
import { useEffect, useState, HTMLAttributes, CSSProperties, useMemo } from '../../../framework';

import { TileImg } from './tile-img';
import { useTilingCapability } from '../hooks/use-tiling';
import { useDocumentState } from '../../../core/shared';

/**
 * TilingLayer 组件的属性接口。
 *
 * 继承标准 div 属性（排除 style 以自定义类型），额外包含：
 * - documentId: 关联的文档 ID
 * - pageIndex: 要渲染的页面索引（0-based）
 * - scale: 可选的外部缩放比例覆盖
 * - style: 可选的 CSS 样式
 */
type TilingLayoutProps = Omit<HTMLAttributes<HTMLDivElement>, 'style'> & {
  /** 文档唯一标识 */
  documentId: string;
  /** 页码索引（0-based） */
  pageIndex: number;
  /** 可选的外部缩放比例覆盖，不传则使用文档当前的 scale */
  scale?: number;
  /** 可选的 CSS 样式，应用到容器 div 上 */
  style?: CSSProperties;
};

/**
 * 瓦片图层容器组件。
 *
 * 为单个 PDF 页面渲染所有可见的瓦片图片。每个瓦片通过 TileImg 组件独立渲染。
 *
 * @param props - 组件属性
 * @param props.documentId - 文档 ID
 * @param props.pageIndex - 页码索引
 * @param props.scale - 可选的缩放比例覆盖
 * @param props.style - 可选的 CSS 样式
 */
export function TilingLayer({
  documentId,
  pageIndex,
  scale: scaleOverride,
  style,
  ...props
}: TilingLayoutProps) {
  // 获取瓦片插件的能力接口
  const { provides: tilingProvides } = useTilingCapability();
  // 获取文档状态（用于读取当前缩放比例）
  const documentState = useDocumentState(documentId);
  // 本地存储当前页面的瓦片列表
  const [tiles, setTiles] = useState<Tile[]>([]);

  // 订阅瓦片渲染事件：当瓦片列表更新时，更新本地 state
  useEffect(() => {
    if (tilingProvides) {
      return tilingProvides.onTileRendering((event) => {
        // 仅处理属于当前文档且包含当前页面的更新
        if (event.documentId === documentId) {
          const newTiles = event.tiles[pageIndex] ?? [];
          // 按 tile.id diff，避免全量 setState 导致不必要的重渲染
          setTiles((prevTiles) => {
            // 创建现有瓦片的 ID 映射，快速查找
            const prevMap = new Map(prevTiles.map(t => [t.id, t]));
            // 检查是否有任何变化（新增、删除、修改）
            const hasChanges = newTiles.some(newTile => {
              const prevTile = prevMap.get(newTile.id);
              return !prevTile ||
                prevTile.status !== newTile.status ||
                prevTile.isFallback !== newTile.isFallback ||
                prevTile.srcScale !== newTile.srcScale;
            }) || prevTiles.some(prevTile =>
              !newTiles.find(updatedTile => updatedTile.id === prevTile.id)
            );
            // 只有真正变化时才更新 state
            return hasChanges ? newTiles : prevTiles;
          });
        }
      });
    }
  }, [tilingProvides, documentId, pageIndex]);

  // 计算实际使用的缩放比例
  // 优先级：外部传入的 scaleOverride > 文档当前的 scale > 默认值 1
  const actualScale = useMemo(() => {
    if (scaleOverride !== undefined) return scaleOverride;
    return documentState?.scale ?? 1;
  }, [scaleOverride, documentState?.scale]);

  return (
    <div
      style={{
        ...style,
      }}
      {...props}
    >
      {tiles?.map((tile) => (
        <TileImg
          key={tile.id}
          documentId={documentId}
          pageIndex={pageIndex}
          tile={tile}
          dpr={window.devicePixelRatio}
          scale={actualScale}
        />
      ))}
    </div>
  );
}
