/**
 * @file RenderLayer 渲染层组件
 *
 * 本文件实现了 RenderLayer React 组件，负责将 PDF 页面渲染为图像并展示在 UI 中。
 * 它是渲染插件在 React 层的核心 UI 组件。
 *
 * 主要功能：
 * 1. 通过渲染插件的 capability 接口发起页面渲染请求
 * 2. 将渲染结果（Blob）转换为 Object URL 并绑定到 <img> 元素
 * 3. 自动响应文档状态变化（缩放、旋转）和页面刷新事件，触发重新渲染
 * 4. 智能管理 Object URL 的生命周期，防止内存泄漏
 *
 * 渲染触发时机（通过 useEffect 依赖数组控制）：
 * - documentId 或 pageIndex 变更 → 切换目标页面
 * - actualScale 或 actualDpr 变更 → 缩放/清晰度变化
 * - renderProvides 变更 → 插件能力就绪
 * - refreshVersion 变更 → 页面内容被刷新（如注解更新）
 *
 * 内存管理策略：
 * - 渲染完成后立即将 Blob 转为 Object URL 并释放 Blob 引用
 * - 组件卸载或重新渲染前，通过 cleanup 函数撤销未完成的渲染任务或释放旧 URL
 * - <img> 加载完成后通过 onLoad 回调释放 Object URL
 */

import { Fragment, useEffect, useRef, useState, useMemo } from '../../../framework';
import type { CSSProperties, HTMLAttributes } from '../../../framework';

import { ignore, PdfErrorCode, Rotation } from '../../../models';

import { useRenderCapability } from '../hooks/use-render';
import { useDocumentState } from '../../../core/shared';

/**
 * RenderLayer 组件的 Props 类型定义。
 *
 * 继承自 HTMLImageElement 的标准属性（排除了 style 以便自定义处理），
 * 并添加了渲染控制相关的专属属性。
 */
type RenderLayerProps = Omit<HTMLAttributes<HTMLImageElement>, 'style'> & {
  /**
   * 要渲染的文档 ID。
   * 必须与已加载到文档系统中的文档 ID 一致。
   */
  documentId: string;
  /**
   * 要渲染的页面索引（从 0 开始）。
   * 例如首页为 0，第二页为 1。
   */
  pageIndex: number;
  /**
   * 可选的缩放比例覆盖值。
   * 若未提供，则使用文档当前的全局缩放比例（documentState.scale）。
   * 适用于需要独立控制某页缩放的场景（如缩略图固定缩放）。
   */
  scale?: number;
  /**
   * 可选的设备像素比（DPR）覆盖值。
   * 若未提供，则使用 window.devicePixelRatio。
   * 在高 DPI 屏幕上设置较高 DPR 可获得更清晰的渲染效果。
   */
  dpr?: number;
  /**
   * 传递给底层 <img> 元素的额外样式。
   * 组件内部默认设置 width: 100% 和 height: 100%，
   * 此处的样式将覆盖或扩展默认样式。
   */
  style?: CSSProperties;
};

/**
 * RenderLayer 组件 —— PDF 页面渲染层
 *
 * 负责将指定的 PDF 页面渲染为图像并显示为 <img> 元素。
 *
 * 智能属性处理逻辑：
 * - 若 scale/dpr 等 props 被显式传入，则使用传入值（覆盖模式）
 * - 若未传入，则自动从文档状态中读取当前值（跟随模式）
 *
 * 自动重新渲染触发条件：
 *   1. 文档状态变化（缩放比例 scale、旋转角度 rotation）
 *   2. 页面刷新（通过核心模块的 REFRESH_PAGES action 触发）
 *   3. 组件 props 变化（documentId、pageIndex、scale、dpr）
 *
 * 状态管理：
 * - imageUrl: 当前渲染结果对应的 Object URL，为 null 时不渲染 <img>
 * - urlRef:   与 imageUrl 同步的 ref，用于在 cleanup 函数中安全释放 URL
 *
 * @param props 组件属性，包含 documentId、pageIndex 及可选的 scale、dpr、style 等
 */
export function RenderLayer({
  documentId,
  pageIndex,
  scale: scaleOverride,
  dpr: dprOverride,
  style,
  ...props
}: RenderLayerProps) {
  // 获取渲染插件的能力对象（包含 renderPage、forDocument 等方法）
  const { provides: renderProvides } = useRenderCapability();
  // 订阅指定文档的状态（缩放比例、旋转角度、页面刷新版本等）
  const documentState = useDocumentState(documentId);

  // 当前渲染结果对应的 Object URL，null 表示尚无渲染结果
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  // 用于在 useEffect cleanup 中访问最新的 URL 引用，避免闭包陷阱
  const urlRef = useRef<string | null>(null);

  /**
   * 计算当前页面的刷新版本号。
   * refreshVersion 由核心模块在页面内容变更时递增（如注解添加/删除），
   * 作为 useEffect 的依赖项，确保页面内容变化时触发重新渲染。
   */
  const refreshVersion = useMemo(() => {
    if (!documentState) return 0;
    return documentState.pageRefreshVersions[pageIndex] || 0;
  }, [documentState, pageIndex]);

  /**
   * 计算实际使用的缩放比例。
   * 优先级：scaleOverride（props 传入） > documentState.scale（文档全局缩放） > 1（默认值）
   */
  const actualScale = useMemo(() => {
    if (scaleOverride !== undefined) return scaleOverride;
    return documentState?.scale ?? 1;
  }, [scaleOverride, documentState?.scale]);

  /**
   * 计算实际使用的设备像素比。
   * 优先级：dprOverride（props 传入） > window.devicePixelRatio（系统当前值）
   */
  const actualDpr = useMemo(() => {
    if (dprOverride !== undefined) return dprOverride;
    return window.devicePixelRatio;
  }, [dprOverride]);

  /**
   * 核心渲染副作用：
   * 当依赖项（documentId、pageIndex、actualScale、actualDpr、refreshVersion）变化时，
   * 发起新的渲染请求并管理 Object URL 的生命周期。
   */
  useEffect(() => {
    // 如果渲染能力尚未就绪，跳过本次渲染
    if (!renderProvides) return;

    // 通过 forDocument 作用域发起整页渲染请求
    // 渲染分辨率下限 1.0：fit-width 等场景下 scale 可能 < 1，此时按原 scale 渲染物理像素不足，
    // 文字看起来发虚。强制至少以原生密度渲染，显示时由浏览器下采样到 CSS 尺寸（与 TilingLayer 行为一致）。
    const task = renderProvides.forDocument(documentId).renderPage({
      pageIndex,
      options: {
        scaleFactor: Math.max(1, actualScale),
        dpr: actualDpr,
      },
    });

    // 等待渲染完成后，将 Blob 转为 Object URL 并更新状态
    task.wait((blob) => {
      const url = URL.createObjectURL(blob);
      setImageUrl(url);
      urlRef.current = url;
    }, ignore);

    // 清理函数：组件卸载或依赖项变化导致重新渲染前执行
    return () => {
      if (urlRef.current) {
        // 如果 URL 已创建（渲染已完成），立即释放 Object URL 防止内存泄漏
        URL.revokeObjectURL(urlRef.current);
        urlRef.current = null;
      } else {
        // 如果 URL 尚未创建（渲染仍在进行中），取消渲染任务避免无效回调
        task.abort({
          code: PdfErrorCode.Cancelled,
          message: 'canceled render task',
        });
      }
    };
  }, [documentId, pageIndex, actualScale, actualDpr, renderProvides, refreshVersion]);

  /**
   * 图像加载完成回调。
   * 在 <img> 元素成功加载 Object URL 后，释放该 URL 以回收内存。
   * 注意：URL 释放后浏览器仍保留已解码的图像数据，<img> 仍可正常显示。
   */
  const handleImageLoad = () => {
    if (urlRef.current) {
      URL.revokeObjectURL(urlRef.current);
      urlRef.current = null;
    }
  };

  /**
   * 渲染输出：
   * 仅在 imageUrl 非空（即渲染完成）时才挂载 <img> 元素。
   * - className="pdfium-page-image" 便于外部样式覆盖
   * - 默认 width/height 为 100%，通过 style prop 可覆盖
   * - 通过 ...props 展开传递额外的 HTML 属性（如 className、onClick 等）
   */
  return (
    <Fragment>
      {imageUrl && (
        <img
          src={imageUrl}
          className="pdfium-page-image"
          onLoad={handleImageLoad}
          {...props}
          style={{
            width: '100%',
            height: '100%',
            ...(style || {}),
          }}
        />
      )}
    </Fragment>
  );
}
