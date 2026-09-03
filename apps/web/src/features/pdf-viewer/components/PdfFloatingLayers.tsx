/**
 * PDF 浮动图层组件
 *
 * 从 PdfViewer 中提取以改进代码组织。
 *
 * 职责：
 * - 图片预览弹窗
 * - 悬停翻译提示（段落级翻译）
 * - 术语提示（双击术语翻译）
 * - 快速翻译提示（选中文本翻译）
 *
 * 所有浮动在 PDF 内容之上的覆盖层 UI。
 */

import React from 'react';
import { Modal, App } from 'antd';
import HoverTranslationTooltip from './HoverTranslationTooltip';
import TermTooltip from './TermTooltip';
import QuickTranslateTooltip from './QuickTranslateTooltip';
import HighlightPopover from './HighlightPopover';
import { TemplateFloatingMenu } from './TemplateFloatingMenu';
import styles from '../PdfViewer.module.css';

import type { SentencePair } from '../types';

/** 翻译提示框状态 */
export interface TranslationTooltipState {
  visible: boolean;
  text: string;
  sentences?: SentencePair[];
  hoveredSentenceIndex?: number;
  x: number;
  y: number;
  /** 段落顶边（视口坐标）；浮层据此判断上下翻转放置 */
  paraTop?: number;
}

/** 术语提示框状态 */
export interface TermTooltipState {
  /** 是否可见 */
  visible: boolean;
  /** 术语原文 */
  term: string;
  /** 术语翻译 */
  translation: string;
  /** X 坐标 */
  x: number;
  /** Y 坐标 */
  y: number;
  /** 是否正在加载 */
  loading?: boolean;
}

/** 快速翻译提示框状态 */
export interface QuickTranslateTooltipState {
  /** 是否可见 */
  visible: boolean;
  /** 原文文本 */
  text: string;
  /** 翻译结果 */
  translation: string;
  /** X 坐标 */
  x: number;
  /** Y 坐标 */
  y: number;
  /** 是否正在加载 */
  loading?: boolean;
}

export interface PdfFloatingLayersProps {
  // ===== 图片预览 =====
  /** 预览图片的 URL */
  imagePreviewUrl: string | null;
  /** 关闭图片预览回调 */
  onCloseImagePreview: () => void;

  // ===== 悬停翻译 =====
  /** 悬停翻译提示框状态 */
  translationTooltip: TranslationTooltipState;
  /** 提示框鼠标进入回调（防止意外消失） */
  onTooltipMouseEnter: () => void;
  /** 提示框鼠标离开回调（允许隐藏） */
  onTooltipMouseLeave: () => void;

  // ===== 术语提示 =====
  /** 术语提示框状态 */
  termTooltip: TermTooltipState;
  /** 隐藏术语提示框回调 */
  onHideTermTooltip: () => void;

  // ===== 快速翻译提示 =====
  /** 快速翻译提示框状态 */
  quickTranslateTooltip: QuickTranslateTooltipState;

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
  translationTooltip,
  onTooltipMouseEnter,
  onTooltipMouseLeave,
  termTooltip,
  onHideTermTooltip,
  quickTranslateTooltip,
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

      {/* 悬停翻译提示框 */}
      <HoverTranslationTooltip
        visible={translationTooltip.visible}
        text={translationTooltip.text}
        sentences={translationTooltip.sentences || []}
        hoveredSentenceIndex={translationTooltip.hoveredSentenceIndex ?? -1}
        x={translationTooltip.x}
        y={translationTooltip.y}
        paraTop={translationTooltip.paraTop}
        maxWidth={500}
        onMouseEnter={onTooltipMouseEnter}
        onMouseLeave={onTooltipMouseLeave}
      />

      {/* 术语提示框 */}
      <TermTooltip
        visible={termTooltip.visible}
        term={termTooltip.term}
        translation={termTooltip.translation}
        x={termTooltip.x}
        y={termTooltip.y}
        loading={termTooltip.loading ?? false}
      />

      {/* 快速翻译提示框 */}
      <QuickTranslateTooltip
        visible={quickTranslateTooltip.visible}
        text={quickTranslateTooltip.text}
        translation={quickTranslateTooltip.translation}
        x={quickTranslateTooltip.x}
        y={quickTranslateTooltip.y}
        loading={quickTranslateTooltip.loading ?? false}
      />

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
