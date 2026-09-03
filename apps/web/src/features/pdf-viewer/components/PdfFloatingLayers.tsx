/**
 * PDF 浮动图层组件
 *
 * 从 PdfViewer 中提取以改进代码组织。
 *
 * 职责：
 * - 图片预览弹窗
 * - 高亮删除 Popover
 *
 * 所有浮动在 PDF 内容之上的覆盖层 UI。
 *
 * 注：悬停翻译 / 双击术语 / 快速翻译三类浮层已随 translationApi 一并移除
 * （autonomics 后端无翻译管线，见 plan Phase 5 清扫）。
 */

import React from 'react';
import { Modal, App } from 'antd';
import HighlightPopover from './HighlightPopover';
import { TemplateFloatingMenu } from './TemplateFloatingMenu';
import styles from '../PdfViewer.module.css';

export interface PdfFloatingLayersProps {
  // ===== 图片预览 =====
  /** 预览图片的 URL */
  imagePreviewUrl: string | null;
  /** 关闭图片预览回调 */
  onCloseImagePreview: () => void;

  // ===== 模板浮动菜单 =====
  /** 模板菜单状态 */
  templateMenu: { visible: boolean; x: number; y: number; context: any };
  /** 关闭模板菜单回调 */
  onCloseTemplateMenu: () => void;
  /** 模板选择回调 */
  onTemplateSelect: (item: any) => void;

  // ===== PDFium 高亮 Popover =====
  /** 当前激活的高亮（PDFium 模式） */
  pdfiumActiveHighlight?: any;
  /** Popover 位置 */
  pdfiumPopoverPosition?: { x: number; y: number };
  /** 删除高亮回调 */
  onDeleteHighlight?: (highlight: any) => void;
  /** 关闭 Popover 回调 */
  onCloseHighlightPopover?: () => void;
}

/**
 * PDF 浮动图层组件
 *
 * 渲染所有浮动在 PDF 内容之上的覆盖层 UI 元素。
 * 这些元素与主 PDF 渲染分离以提高代码组织性。
 */
export const PdfFloatingLayers: React.FC<PdfFloatingLayersProps> = React.memo(({
  imagePreviewUrl,
  onCloseImagePreview,
  templateMenu,
  onCloseTemplateMenu,
  onTemplateSelect,
  pdfiumActiveHighlight,
  pdfiumPopoverPosition,
  onDeleteHighlight,
  onCloseHighlightPopover,
}) => {
  const { message } = App.useApp();

  return (
    <>
      {/* 右键浮动模板选择菜单 */}
      <TemplateFloatingMenu
        visible={templateMenu.visible}
        x={templateMenu.x}
        y={templateMenu.y}
        context={templateMenu.context}
        onClose={onCloseTemplateMenu}
        onTemplateSelect={onTemplateSelect}
      />

      {/* 图片预览弹窗 */}
      <Modal
        open={!!imagePreviewUrl}
        footer={null}
        onCancel={onCloseImagePreview}
        width="80%"
        centered
      >
        {imagePreviewUrl && (
          <img
            src={imagePreviewUrl}
            alt="图片预览"
            className={styles.imagePreview}
            onError={(e) => {
              e.currentTarget.style.display = 'none';
              message.error('图片加载失败');
            }}
          />
        )}
      </Modal>

      {/* PDFium 高亮点击 Popover */}
      {pdfiumActiveHighlight && (
        <HighlightPopover
          visible={true}
          x={pdfiumPopoverPosition?.x ?? 0}
          y={pdfiumPopoverPosition?.y ?? 0}
          highlight={pdfiumActiveHighlight}
          onDelete={onDeleteHighlight ?? (() => {})}
          onClose={onCloseHighlightPopover ?? (() => {})}
        />
      )}
    </>
  );
});
