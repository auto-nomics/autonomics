/**
 * @file 渲染插件类型定义
 *
 * 本文件集中定义了 Render Plugin 所需的全部 TypeScript 类型，包括：
 * - 插件配置接口（RenderPluginConfig）
 * - 渲染操作的请求参数接口（RenderPageOptions / RenderPageRectOptions）
 * - 文档级渲染作用域接口（RenderScope）
 * - 插件对外暴露的能力接口（RenderCapability）
 *
 * 设计决策：
 * - 将 "整页渲染" 与 "矩形区域渲染" 拆分为独立接口，便于调用方按需选择。
 * - RenderScope 通过 forDocument(documentId) 模式实现文档级作用域绑定，
 *   避免在每个方法调用中重复传递 documentId。
 * - 渲染结果分为 Blob（编码后的图像）和 ImageDataLike（原始像素数据）两种，
 *   分别满足 UI 展示和像素级处理的不同需求。
 */

import { BasePluginConfig } from '../core';
import {
  ImageConversionTypes,
  ImageDataLike,
  PdfErrorReason,
  PdfRenderPageOptions,
  Rect,
  Task,
} from '../models';

/**
 * 渲染插件的可选配置项。
 * 所有字段均有默认值，注册插件时可不传。
 */
export interface RenderPluginConfig extends BasePluginConfig {
  /**
   * 渲染时是否初始化并绘制表单控件（如文本框、复选框等）。
   * 默认为 `false`，即不渲染表单。
   */
  withForms?: boolean;
  /**
   * 渲染时是否包含 PDF 注解（如高亮、批注等）。
   * 默认为 `false`，即不渲染注解。
   */
  withAnnotations?: boolean;
  /**
   * 渲染输出的默认图像格式。
   * 可选值由 ImageConversionTypes 定义（如 'image/png'、'image/jpeg'）。
   * 默认为 `'image/png'`。
   */
  defaultImageType?: ImageConversionTypes;
  /**
   * 渲染输出的默认图像质量系数（仅对有损压缩格式如 JPEG 生效）。
   * 取值范围 0～1，默认为 `0.92`。
   */
  defaultImageQuality?: number;
}

/**
 * 矩形区域渲染的请求参数。
 * 用于仅渲染页面中某个矩形子区域，常用于缩略图裁剪或局部放大预览。
 */
export interface RenderPageRectOptions {
  /** 要渲染的页面索引（从 0 开始） */
  pageIndex: number;
  /** 要渲染的矩形区域，坐标相对于页面左上角 */
  rect: Rect;
  /** 渲染选项（缩放比例、设备像素比等） */
  options: PdfRenderPageOptions;
}

/**
 * 整页渲染的请求参数。
 * 用于渲染完整的一页 PDF 内容。
 */
export interface RenderPageOptions {
  /** 要渲染的页面索引（从 0 开始） */
  pageIndex: number;
  /** 渲染选项（缩放比例、设备像素比等） */
  options: PdfRenderPageOptions;
}

/**
 * 文档级渲染作用域接口。
 *
 * 通过 RenderCapability.forDocument(documentId) 获取后，
 * 所有方法调用自动绑定到指定文档，无需再次传递 documentId。
 * 这是一种"预绑定"模式，简化了多文档场景下的重复调用。
 */
export interface RenderScope {
  /**
   * 渲染指定页面的完整内容，返回编码后的图像 Blob。
   * @param options 页面索引及渲染选项
   * @returns 异步任务，成功时解析为 Blob，失败时携带 PdfErrorReason
   */
  renderPage(options: RenderPageOptions): Task<Blob, PdfErrorReason>;
  /**
   * 渲染指定页面的矩形区域，返回编码后的图像 Blob。
   * @param options 页面索引、矩形区域及渲染选项
   * @returns 异步任务，成功时解析为 Blob，失败时携带 PdfErrorReason
   */
  renderPageRect(options: RenderPageRectOptions): Task<Blob, PdfErrorReason>;
  /**
   * 渲染指定页面的完整内容，返回原始像素数据（未经编码）。
   * 适用于需要直接操作像素的场景，如文本选择高亮叠加。
   * @param options 页面索引及渲染选项
   * @returns 异步任务，成功时解析为 ImageDataLike，失败时携带 PdfErrorReason
   */
  renderPageRaw(options: RenderPageOptions): Task<ImageDataLike, PdfErrorReason>;
  /**
   * 渲染指定页面的矩形区域，返回原始像素数据（未经编码）。
   * @param options 页面索引、矩形区域及渲染选项
   * @returns 异步任务，成功时解析为 ImageDataLike，失败时携带 PdfErrorReason
   */
  renderPageRectRaw(options: RenderPageRectOptions): Task<ImageDataLike, PdfErrorReason>;
}

/**
 * 渲染插件对外暴露的能力接口。
 *
 * 该接口通过 PluginSystem 的 capability 机制提供给其他插件或 UI 层使用。
 * 包含两大类操作：
 *   1. 直接操作 – 基于当前活动文档执行渲染（renderPage / renderPageRect 等）
 *   2. 作用域操作 – 通过 forDocument() 绑定到指定文档后执行渲染
 */
export interface RenderCapability {
  // ── 活动文档操作 ──

  /**
   * 渲染当前活动文档中指定页面的完整内容，返回编码后的图像 Blob。
   */
  renderPage(options: RenderPageOptions): Task<Blob, PdfErrorReason>;
  /**
   * 渲染当前活动文档中指定页面的矩形区域，返回编码后的图像 Blob。
   */
  renderPageRect(options: RenderPageRectOptions): Task<Blob, PdfErrorReason>;
  /**
   * 渲染当前活动文档中指定页面的完整内容，返回原始像素数据。
   */
  renderPageRaw(options: RenderPageOptions): Task<ImageDataLike, PdfErrorReason>;
  /**
   * 渲染当前活动文档中指定页面的矩形区域，返回原始像素数据。
   */
  renderPageRectRaw(options: RenderPageRectOptions): Task<ImageDataLike, PdfErrorReason>;

  // ── 文档作用域操作 ──

  /**
   * 创建一个绑定到指定文档的渲染作用域。
   * 返回的 RenderScope 中所有方法自动使用该 documentId，
   * 无需每次调用时重复传递。
   * @param documentId 目标文档的唯一标识
   * @returns 绑定到指定文档的渲染作用域对象
   */
  forDocument(documentId: string): RenderScope;
}
