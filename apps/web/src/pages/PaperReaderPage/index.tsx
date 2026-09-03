/**
 * 论文阅读器页面组件
 *
 * 应用的核心页面，提供三栏布局的论文阅读体验：
 * - 左侧：段落导览侧栏（可折叠），显示论文的章节结构和导航
 * - 中间：PDF 阅读器（PDFium 渲染），支持选中文本注入到 AI 对话；
 *   题录记录（无 PDF）时显示 MetadataView 元数据视图
 * - 右侧：AI Chat 面板，与论文内容进行对话，支持上下文注入
 *
 * 布局设计：
 * - 三栏布局充分利用宽屏空间，中间面板自适应剩余宽度
 * - 左侧面板可折叠（Ctrl+Left），为 PDF 阅读器腾出更多空间
 * - 右侧面板可折叠（Ctrl+Right），专注阅读模式
 * - 面板宽度可拖拽调整，约束在最小/最大值之间
 *
 * 组件通信：
 * - 使用 ref 命令式调用 ChatPanel 的方法（如注入上下文）
 * - 使用 ContextBridge 组件提供跨组件的 AI 提示词模板共享
 * - 使用回调函数（onContextInject）实现 PDF 选中文本到 Chat 的传递
 *
 * 特殊路由参数：
 * - :paperId - 论文 ID（路径参数）
 * - ?page=xxx - 初始页码（查询参数，可选）
 * - ?attachmentId=xxx - 指定附件 ID（查询参数，可选）
 *
 * @module PaperReaderPage
 */
import React, { useState, useEffect, useCallback, useRef, lazy, Suspense } from 'react';
import { useTranslation } from 'react-i18next';
import { useParams, useNavigate, useSearchParams } from 'react-router-dom';
import { Button, Spin, App } from 'antd';
import {
  ArrowLeftOutlined,
  MessageOutlined,
  UploadOutlined,
  CloudDownloadOutlined,
} from '@ant-design/icons';
import { getPaper, getPdfUrl, getMarkdown } from '../../services/papersApi';
import { getAttachmentUrl, getAttachments } from '../../services/attachmentsApi';
import { attachPdf, downloadPdf } from '../../services/bibliographyApi';
import { isTauri, getTauriBaseUrl } from '../../services/client';

const PdfViewer = lazy(() => import('../../features/pdf-viewer/components/WasmPdfViewer'));
import SidebarPanel from '../../components/SidebarPanel';
import ChatPanel from '../../features/ai-chat/components/ChatPanel';
import ContextBridge, { buildContextPrompt } from '../../components/ContextBridge';
import FeatureErrorBoundary from '../../components/FeatureErrorBoundary';
import { getTypeLabel, getTypeColor, getTypeFields, METADATA_FIELD_LABELS } from '../../utils/literatureTypes';

import MetadataView from './components/MetadataView';
import useGuideBlockData from './hooks/useGuideBlockData';
import ReadingContextBar from '../../features/paragraph-guide-v2/ReadingContextBar';
import useScrollContext from '../../features/paragraph-guide-v2/hooks/useScrollContext';
import useAnchors from '../../features/paragraph-guide-v2/hooks/useAnchors';

import { useReaderShortcuts } from './hooks/useReaderShortcuts';
import { usePanelCoordination } from './hooks/usePanelCoordination';
import { DRAG_HANDLE } from './constants';

/**
 * V4 导览阅读体验：ReadingContextBar 包装组件
 *
 * 负责管理锚点点击导航，并渲染 ReadingContextBar。
 *
 * 由于 PdfViewer 使用单页模式（而非滚动模式），这里使用页码变化来
 * 触发当前章节更新，而不是滚动位置。
 */
const ReadingContextBarWithTracking = React.memo(function ReadingContextBarWithTracking({
  guideNodes,
  blocksData,
  currentPage,
  onNavigate,
  currentSection,
  currentGuide,
  anchors,
}: any) {
  // 使用 useAnchors Hook 管理锚点点击导航
  const { handleAnchorClick } = useAnchors({
    blocksData,
    currentSection,
    onNavigate,
  });

  return (
    <ReadingContextBar
      currentSection={currentSection}
      currentGuide={currentGuide}
      anchors={anchors}
      onAnchorClick={handleAnchorClick}
      compact={false}
    />
  );
});

/**
 * 论文阅读器页面主组件
 *
 * 职责：
 * 1. 从 URL 获取论文 ID，加载论文信息
 * 2. 管理三栏布局的折叠状态
 * 3. 协调三个子组件之间的通信（上下文注入）
 * 4. 处理加载和错误状态
 */
export default function PaperReaderPage() { // 导出默认的论文阅读器页面组件
  const { t } = useTranslation('paperReader');
  const { message } = App.useApp();
  // 从 URL 参数中获取论文 ID（如 /paper/123 中的 123）
  const { id } = useParams(); // 从当前 URL 提取动态参数 id
  const paperId = id!; // UUID 字符串，useParams 确保 id 存在
  // 路由导航函数
  const navigate = useNavigate(); // 获取编程式导航函数
  // 从 URL 查询参数中获取附件 ID（如 ?attachmentId=456 中的 456）
  // 统一 PDF 与附件系统后，所有 PDF（包括原始 PDF）都通过 attachmentId 访问
  // 通过 isViewingNonPrimary 状态判断当前是否在查看非原始 PDF 的附件
  const [searchParams] = useSearchParams();
  const attachmentId = searchParams.get('attachmentId'); // 附件 ID（字符串或 null）


  // 当前是否正在查看非原始 PDF 的附件（用于决定是否显示导览侧栏和提示横幅）
  // 初始值为 false，在附件数据加载后更新
  // 当 attachmentId 为 null 时（直接访问 /paper/{id}，不带查询参数），说明没有指定附件，
  // 此时后端的 /api/papers/{id}/pdf 端点会返回原始 PDF，所以不是查看附件
  // 当 attachmentId 不为 null 时，需要检查该附件是否为原始 PDF（is_primary），
  // 如果是原始 PDF 则不显示横幅，如果是其他附件则显示横幅并隐藏导览
  const [isViewingNonPrimary, setIsViewingNonPrimary] = useState(false);

  // 论文基本信息状态
  const [paper, setPaper] = useState<any>(null); // 状态：论文详情对象，初始为 null（未加载）
  // 论文 Markdown 内容状态（用于 ChatPanel 的分层上下文注入）
  const [markdown, setMarkdown] = useState(''); // 状态：论文解析后的 Markdown 内容，初始为空字符串
  // 初始加载状态
  const [loading, setLoading] = useState(true); // 状态：是否正在加载，初始为 true

  // ========== Panel coordination management ==========
  // 使用 usePanelCoordination hook 管理面板折叠状态、宽度和拖拽调整
  const {
    guideCollapsed,
    guideWidth,
    toggleGuide,
    handleGuideResizeStart,
    chatCollapsed,
    chatWidth,
    toggleChat,
    handleChatResizeStart,
    isDragging,
  } = usePanelCoordination();

  // ========== 统一搜索栏相关状态 ==========
  const [chatSearchKeyword, setChatSearchKeyword] = useState('');

  // V2 坐标导航数据：用于传递给 PdfViewer 的 CoordinateHighlightOverlay
  const [coordinateNavigation, setCoordinateNavigation] = useState<any>(null); // 状态：V2 坐标导航数据 {pageIdx, bboxes, animationMs}

  const [currentPage, setCurrentPage] = useState(1); // 状态：当前 PDF 页码（1-based）

  // PDF URL 解析：Tauri 模式下将相对路径转为 sidecar 绝对 URL
  // 解析完成后才渲染 PdfViewer，避免 URL 中间状态触发重复加载
  const rawPdfUrl = attachmentId
    ? getAttachmentUrl(attachmentId)
    : getPdfUrl(paperId);
  const [resolvedPdfUrl, setResolvedPdfUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!rawPdfUrl) return;
    if (!isTauri) { setResolvedPdfUrl(rawPdfUrl); return; }
    let cancelled = false;
    getTauriBaseUrl().then(baseUrl => {
      if (!cancelled) setResolvedPdfUrl(`${baseUrl}${rawPdfUrl}`);
    }).catch(() => {
      if (!cancelled) setResolvedPdfUrl(rawPdfUrl);
    });
    return () => { cancelled = true; };
  }, [rawPdfUrl]);

  // V2 导览块数据管理（加载、处理、计算当前章节/引导/锚点）
  const {
    blocksData, guideNodes, currentSection, currentGuide, currentAnchors,
  } = useGuideBlockData({ paperId, paper, currentPage });

  // ChatPanel 组件的 ref，用于命令式调用其方法
  // 为什么使用 ref 而不是回调？
  // - 需要在父组件中主动调用子组件的方法
  // - ref 不会触发重渲染，性能更好
  const chatPanelRef = useRef<any>(null); // ref：指向 ChatPanel 组件实例

  // PdfViewer 组件的 ref，用于命令式调用其 goToPage 方法
  // 为什么需要这个 ref？
  // - 用户点击段落导览的段落摘要时，需要跳转到 PDF 对应的页面
  // - 通过 ref 可以直接调用 PdfViewer 暴露的 goToPage 方法
  const pdfViewerRef = useRef<any>(null); // ref：指向 PdfViewer 组件实例
  const renderCountRef = useRef(0);
  renderCountRef.current++;
  if (renderCountRef.current < 30 || renderCountRef.current % 50 === 0) {
    console.log(`[PaperReaderPage render #${renderCountRef.current}]`, { paperId, attachmentId, loading, paper: !!paper, currentPage, resolvedPdfUrl });
  }

  // ParagraphGuideV2 组件的 ref，用于命令式调用其 scrollToNode 方法
  // 用于 V2 反向导航（PDF 点击 → 导览滚动到对应节点）
  const paragraphGuideV2Ref = useRef<any>(null); // ref：指向 ParagraphGuideV2 组件实例

  /**
   * 重新加载论文数据
   *
   * 此函数用于在 PDF 上传或下载成功后刷新论文数据，
   * 而不需要使用 window.location.reload() 刷新整个页面。
   *
   * 为什么不用 window.location.reload()？
   * - 页面刷新会丢失所有前端状态（聊天记录、搜索关键词、面板折叠状态等）
   * - 用户体验差，有明显的白屏闪烁
   * - 重新加载数据只需要更新相关状态，不需要完全重载页面
   */
  const reloadPaper = useCallback(async () => {
    try {
      // 并行加载论文信息和 Markdown 内容，提升加载速度
      const [data, mdData] = await Promise.all([
        getPaper(paperId!),    // 获取论文详细信息
        getMarkdown(paperId!).catch(() => ({ markdown: '' })), // 获取 Markdown 内容，失败时返回空字符串
      ]);
      setPaper(data); // 更新论文数据到状态，触发重新渲染
      setMarkdown((mdData as any).markdown || ''); // 更新 Markdown 内容到状态
    } catch (err) {
      message.error(t('reloadFailed', { error: (err as any).message }));
    }
  }, [paperId]); // 依赖 paperId，确保使用正确的论文 ID

  /**
   * 加载论文信息
   *
   * 为什么在 useEffect 中调用而不是直接在函数体中？
   * - React 规则：副作用（如数据请求）必须在 useEffect 中执行
   * - 避免每次渲染都发起请求
   */
  useEffect(() => { // useEffect 副作用钩子
    // 创建 AbortController 用于取消请求
    const controller = new AbortController();

    async function load() { // 定义异步加载函数
      try { // 开始 try-catch
        // 并行加载论文信息、Markdown 内容和附件列表，提升加载速度
        const [data, mdData, attData] = await Promise.all([ // 使用 Promise.all 并行请求
          getPaper(paperId!, controller.signal),    // 获取论文详细信息
          getMarkdown(paperId!, controller.signal).catch(() => ({ markdown: '' })), // 获取 Markdown 内容，失败时返回空字符串（非关键数据）
          getAttachments(paperId!, controller.signal).catch(() => ({ attachments: [] })), // 获取附件列表，失败时返回空数组（非关键数据）
        ]);

        // 检查是否已取消
        if (controller.signal.aborted) return;

        setPaper(data); // 设置论文数据到状态
        setMarkdown((mdData as any).markdown || ''); // 设置 Markdown 内容到状态

        // ========== 判断当前是否在查看非原始 PDF 的附件 ==========
        // 如果 URL 中有 attachmentId，需要检查该附件是否为原始 PDF（is_primary=true）
        // 如果不是原始 PDF，则设置 isViewingNonPrimary 为 true，用于：
        // 1. 隐藏左侧段落导览面板（导览数据只针对原始 PDF）
        // 2. 显示"正在查看附件 PDF"提示横幅
        if (attachmentId) {
          // 从附件列表中查找 URL 指定的附件
          const currentAtt = ((attData as any).attachments || []).find(
            (att: any) => att.id === attachmentId // 直接比较 UUID 字符串
          );
          // 如果找到了附件且不是原始 PDF（is_primary 为 false 或不存在），标记为查看非原始 PDF
          if (currentAtt && !currentAtt.is_primary) {
            setIsViewingNonPrimary(true); // 设置为查看非原始 PDF 的附件
          } else {
            setIsViewingNonPrimary(false); // 是原始 PDF 或未找到附件，不标记
          }
        } else {
          // URL 中没有 attachmentId，通过后端默认 PDF 端点加载原始 PDF
          setIsViewingNonPrimary(false); // 不是查看附件
        }
      } catch (err) { // 捕获加载错误
        // 忽略 AbortError，只记录其他错误
        if ((err as any).name !== 'AbortError') {
          const errMsg = err instanceof Error ? err.message : String(err);
          // sidecar 启动时的连接错误静默处理
          if (!errMsg.includes('error sending request') && !errMsg.includes('Failed to fetch')) {
            message.error(t('loadFailed', { error: errMsg })); // 弹出错误提示
          }
        }
      } finally { // 无论成功失败都执行
        // 无论成功失败都关闭加载状态（仅在未取消时）
        if (!controller.signal.aborted) {
          setLoading(false); // 关闭加载动画
        }
      }
    }
    load(); // 执行加载函数

    // 清理函数：组件卸载或依赖变化时取消进行中的请求
    return () => {
      controller.abort();
    };
  }, [paperId, attachmentId]); // 依赖 paperId 和 attachmentId，切换论文或附件时重新加载

  /**
   * 全局键盘快捷键管理
   *
   * - Ctrl+ArrowLeft: 切换左侧导览面板
   * - Ctrl+ArrowRight: 切换右侧 AI 聊天面板
   * - Ctrl+Alt+A: 返回列表页
   *
   * 已提取到 useReaderShortcuts hook 中
   */
  useReaderShortcuts({
    paperId,
    navigate,
    onToggleGuide: toggleGuide,
    onToggleChat: toggleChat,
  });

  /**
   * 处理上下文注入到 AI 对话（两段式：system + user）
   *
   * 直接通过 ref 调用 ChatPanel 的 sendMessage 方法：
   * - userPrompt 作为用户消息文本送入对话历史
   * - systemPrompt 作为单次 system 段透传到 useChatSender.buildSystemPrompt，覆盖默认 persona
   *
   * 兼容：若上游只传了 prompt（旧调用方），按老行为发送纯文本消息。
   */
  const handleContextInject = useCallback((context: any) => {
    if (!chatPanelRef.current?.sendMessage) return;
    const text = context.userPrompt ?? context.prompt;
    if (!text) return;
    chatPanelRef.current.sendMessage(text, context.systemPrompt ? { systemPrompt: context.systemPrompt } : undefined);
  }, []);

  /**
   * 处理 V2 坐标导航（ParagraphGuideV2 的 onNavigate 回调）
   *
   * 这是 V2 导览系统的坐标导航功能：
   * 用户点击 V2 导览节点时，PdfViewer 根据坐标数据跳转到对应页面并高亮显示。
   *
   * 与 V1 导航的区别：
   * - V1: handleGuideNavigate(page, text) - 基于文本匹配，需要搜索文本内容
   * - V2: handleCoordinateNavigate({pageIdx, bboxes, animationMs}) - 基于精确坐标，直接定位
   *
   * @param {object} navData - 导航数据对象
   * @param {number} navData.pageIdx - 目标页码索引（0-based）
   * @param {Array<Array<number>>} navData.bboxes - 边界框数组，每个 bbox 为 [x0, y0, x1, y1]
   * @param {number} [navData.animationMs] - 高亮动画持续时间（毫秒），默认 4000ms
   */
  const handleCoordinateNavigate = useCallback((navData: any) => { // useCallback 缓存 V2 坐标导航处理函数
    if (!navData) return; // 无导航数据时直接返回
    setCoordinateNavigation(navData); // 设置坐标导航数据，PdfViewer 会监听此状态并跳转+高亮
  }, []); // 无外部依赖，setCoordinateNavigation 是稳定引用

  /**
   * 处理 V2 反向导航（PDF 坐标点击 → 导览节点滚动）
   *
   * 这是 V2 导览系统的反向导航功能（改进版：使用精确 PDF 块坐标）：
   * 用户点击 PDF 文本时，根据点击坐标反查对应的导览节点并滚动到该位置。
   *
   * 实现步骤：
   * 1. 将屏幕像素坐标转换为 MinerU 归一化坐标（0-1000）
   * 2. 在精确 PDF 块数据中查找包含点击位置的块
   * 3. 通过块→节点的映射关系，找到关联的导览节点
   * 4. 调用 ParagraphGuideV2 的 scrollToNode 方法滚动到该节点
   *
   * 相比旧版的改进：
   * - 旧版：使用 guide_nodes 的 primary_bbox（聚合坐标）做命中检测
   *   问题：primary_bbox 覆盖面积大，可能包含空白区域，导致误触发
   * - 新版：使用 pdf_blocks 的精确坐标做命中检测，再映射到 guide_node
   *   优势：只有点击在实际内容区域（文字、标题、表格等）才触发导航
   *
   * 坐标转换公式：normX = (screenX / (pageWidth * scale)) * 1000
   *
   * @param {object} clickData - 点击数据对象
   * @param {number} clickData.pageIdx - 点击所在页码索引（0-based）
   * @param {number} clickData.x - 点击位置的屏幕 X 坐标（相对于 PDF 页面容器）
   * @param {number} clickData.y - 点击位置的屏幕 Y 坐标
   * @param {number} clickData.pageWidth - PDF 页面原始宽度（PDF point）
   * @param {number} clickData.pageHeight - PDF 页面原始高度（PDF point）
   * @param {number} clickData.scale - 当前缩放比例
   */
  const handlePdfCoordinateClick = useCallback((clickData: any) => { // useCallback 缓存 V2 反向导航处理函数
    const { pageIdx, x, y, pageWidth, pageHeight, scale } = clickData; // 解构点击数据

    // 第一步：将屏幕像素坐标转换为 MinerU 归一化坐标（0-1000）
    const COORD_RANGE = 1000; // MinerU 坐标范围
    const normX = (x / (pageWidth * scale)) * COORD_RANGE; // 归一化 X 坐标
    const normY = (y / (pageHeight * scale)) * COORD_RANGE; // 归一化 Y 坐标

    // 第二步：在块数据中查找包含点击位置的精确块
    // blocksData 包含两种数据源：
    // - 精确模式：pdf_blocks 数据，每个块有精确 bbox 和 node_id 映射
    // - 降级模式：guide_nodes 的 primary_bbox，块本身就是导览节点
    const clickedBlock = blocksData.find((block: any) => {
      // 首先检查页码是否一致（不同页的块不可能包含点击位置）
      if (block.page_idx !== pageIdx) return false; // 页码不同，跳过

      // 解构块的边界框坐标：[x0, y0, x1, y1]（0-1000 MinerU 坐标系）
      const [x0, y0, x1, y1] = block.bbox;

      // 检查点击位置是否在块的边界框范围内
      // 精确块的 bbox 更小更精确，只有点击在实际内容区域才会匹配
      return normX >= x0 && normX <= x1 && normY >= y0 && normY <= y1;
    });

    // 没有找到匹配的块，说明点击了空白区域或非内容区域
    if (!clickedBlock) {
      return; // 静默返回，不做任何操作
    }

    // 第三步：查找该块关联的导览节点并滚动定位
    // 精确模式：clickedBlock.node_id 是通过空间匹配关联的导览节点 ID
    // 降级模式：clickedBlock.id 就是导览节点本身的 ID
    // 如果 node_id 为 null，说明该 PDF 块不在任何导览节点的范围内（如页眉、页脚），不触发导航
    const targetNodeId = (clickedBlock as any).node_id || (clickedBlock as any).id; // 优先使用 node_id（精确模式）
    if (!targetNodeId) {
      return; // 没有关联的导览节点，静默返回
    }

    // 第四步：调用 ParagraphGuideV2 的 scrollToNode 方法滚动到该节点
    if (paragraphGuideV2Ref.current?.scrollToNode) {
      paragraphGuideV2Ref.current.scrollToNode(targetNodeId); // 滚动导览面板到目标节点
    }
  }, [blocksData]); // 依赖于块数据（blocksData），数据更新时重新缓存函数

  // ========== 统一搜索栏回调函数 ==========

  /**
   * 处理统一搜索栏的搜索内容变化
   *
   * 由 PdfViewer 中的统一搜索栏在用户输入或切换目标时调用。
   * 根据当前搜索目标，将关键词传递给对应的面板组件：
   * - 'chat'：将关键词传递给 ChatPanel，过滤聊天消息
   * - 'pdf'：PDF 搜索完全在 PdfViewer 内部处理，不需要更新 PaperReaderPage 状态
   *
   * V2 迁移说明：
   * - 移除了 'outline' 搜索（V1 导览的搜索功能）
   * - V2 导览暂不支持搜索功能
   *
   * 为什么每次切换目标都清空其他面板的搜索关键词？
   * - 避免用户从一个面板切到另一个时，之前的面板仍显示被过滤的内容
   * - 用户通常期望切换目标 = 重新开始搜索
   *
   * @param {string} keyword - 用户输入的搜索关键词
   * @param {string} target - 当前搜索目标：'chat' | 'pdf'
   */
  const handleUnifiedSearchChange = useCallback((keyword: any, target: any) => { // useCallback 缓存搜索变化回调，避免子组件不必要的重渲染
    // 只在目标为「聊天」时传递关键词，否则清空聊天搜索状态
    setChatSearchKeyword(target === 'chat' ? keyword : ''); // 设置或清空聊天搜索关键词
    // 'pdf' 目标不需要更新 PaperReaderPage 状态，PDF 搜索完全由 PdfViewer 内部处理
  }, []); // 无外部依赖，setChatSearchKeyword 是稳定引用

  // ========== 拖拽调整宽度处理函数 ==========
  // 已由 usePanelCoordination hook 管理（handleGuideResizeStart, handleChatResizeStart）

  // 渲染加载状态
  if (loading) { // 如果正在加载
    return ( // 返回加载中的 UI
      <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100%' }}> {/* 居中容器 */}
        <Spin size="large" tip={t('loading')}><div /></Spin>
      </div>
    );
  }

  // 渲染错误状态（论文不存在或加载失败）
  if (!paper) { // 如果论文数据为空（加载失败或不存在）
    return ( // 返回错误状态的 UI
      <div style={{ textAlign: 'center', padding: 48 }}> {/* 居中容器 */}
        <p>{t('notFound')}</p> {/* 错误提示文字 */}
        <Button onClick={() => navigate('/')}>{t('backToList')}</Button> {/* 返回列表按钮 */}
      </div>
    );
  }

  // 主渲染：三栏布局（带顶部视图切换栏）
  return ( // 返回主页面内容
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', background: 'var(--bg-primary)', overflow: 'hidden' }}> {/* 纵向 flex：顶部标签栏 + 三栏内容 */}

      {/* ========== 三栏布局内容区 ========== */}
    <div style={{ display: 'flex', flex: 1, overflow: 'hidden' }}> {/* 三栏 flex 布局容器 */}
      {/* 左侧：段落导览侧栏（可折叠，可拖拽调整宽度） */}
      {/* 当查看非原始 PDF 的附件时隐藏导览面板，因为导览数据只针对论文的原始 PDF */}
      {!isViewingNonPrimary && (
      <div style={{ // 左侧导览栏容器
        // 折叠时 0px（完全隐藏），展开时使用可调整的 guideWidth
        width: guideCollapsed ? 0 : guideWidth, // 折叠时完全隐藏，展开时使用拖拽可调的 guideWidth
        minWidth: guideCollapsed ? 0 : guideWidth, // 最小宽度与 width 保持一致，防止 flex 收缩
        borderRight: guideCollapsed ? 'none' : '1px solid var(--border-color)', // 折叠时隐藏边框
        // 平滑过渡动画：拖拽时禁用 transition 避免宽度变化延迟，正常状态保留动画
        transition: isDragging ? 'none' : 'width var(--transition-normal)', // 拖拽时无过渡（跟手），其他时候平滑过渡
        display: 'flex', // flex 布局
        flexDirection: 'column', // 纵向排列子元素
        overflow: 'hidden',  // 防止内容溢出
      }}>
        {/* 功能级错误边界：包裹左侧导览面板，崩溃时只影响此面板 */}
        <FeatureErrorBoundary name={t('feature.guide')}>
          {/* 使用 SidebarPanel 组件（V4 导览面板） */}
          <SidebarPanel
            paperId={paperId as any}
            collapsed={guideCollapsed}
            onToggleCollapse={toggleGuide}
            paperInfo={paper}
            onCoordinateNavigate={handleCoordinateNavigate}
            guideV2Ref={paragraphGuideV2Ref}
          />
        </FeatureErrorBoundary>
      </div>
      )}

      {/* 左侧拖拽手柄：用于调整导览栏宽度 */}
      {/* 仅在导览栏展开且正在查看原始 PDF（非附件模式）时显示 */}
      {!isViewingNonPrimary && !guideCollapsed && ( // 条件渲染：导览栏展开且非附件模式时显示拖拽手柄
        <div
          onMouseDown={handleGuideResizeStart}
          style={{
            width: DRAG_HANDLE.WIDTH,
            height: DRAG_HANDLE.HEIGHT,
            cursor: 'col-resize',
            backgroundColor: 'transparent',
            transition: 'background-color 0.2s',
            zIndex: 10,
            flexShrink: 0,
            alignSelf: 'flex-start',
          }}
          title={t('dragHint.guide')}
          className="resize-handle resize-handle-left"
        />
      )}

      {/* 中间：PDF 阅读器或元数据视图 */}
      <div style={{ flex: 1, overflow: 'hidden', position: 'relative', display: 'flex', flexDirection: 'column' }}> {/* 中间主内容区：flex:1 占据剩余空间 */}
        {/* 附件查看模式提示横幅：当查看非原始 PDF 的附件时显示 */}
        {/* 通过 isViewingNonPrimary 判断，而非简单检查 attachmentId 是否存在 */}
        {/* 因为原始 PDF 也是附件（is_primary=true），不应该显示此横幅 */}
        {isViewingNonPrimary && (
          <div style={{
            padding: '4px 16px',                           // 上下 4px，左右 16px
            background: 'var(--color-primary-bg)',          // 主题色背景
            borderBottom: '1px solid var(--border-color)',  // 底部边框
            fontSize: 12,                                  // 小号字体
            color: 'var(--color-primary)',                  // 主题色文字
            display: 'flex',                               // 弹性布局
            alignItems: 'center',                          // 垂直居中
            gap: 8,                                        // 元素间距
          }}>
            {/* 提示文字：告知用户当前查看的是附件 PDF，导览和 AI 对话仍基于论文的原始 PDF */}
            <span>{t('viewingAttachment')}</span>
            {/* 返回原始 PDF 的链接：导航到论文阅读页（不带 attachmentId，后端默认返回原始 PDF） */}
            <Button
              type="link"
              size="small"
              onClick={() => navigate(`/paper/${paperId}`)}  // 不带 attachmentId，加载原始 PDF
            >
              {t('backToOriginal')}
            </Button>
          </div>
        )}
        {/* 条件渲染：根据论文来源决定显示 PDF 阅读器还是元数据视图 */}
        {paper && ((paper as any).item_source === 'bib_import' || (paper as any).item_source?.startsWith('bibliography_import_')) && !(paper as any).storage_key && (!(paper as any).attachments || (paper as any).attachments.length === 0) ? (
          /* ========== 题录导入记录的元数据视图（无 PDF） ========== */
          <MetadataView
            paper={paper}
            paperId={paperId}
            reloadPaper={reloadPaper}
            message={message}
          />
        ) : (
          /* ========== PDF 上传记录的阅读器视图 ========== */
          <Suspense fallback={
            <div style={{
              display: 'flex',
              justifyContent: 'center',
              alignItems: 'center',
              height: '100%',
              background: 'var(--bg-tertiary)'
            }}>
              <Spin size="large" tip={t('loadingReader')}><div /></Spin>
            </div>
          }>
            {/* 功能级错误边界：包裹 PDF 阅读器，崩溃时只影响此面板 */}
            <FeatureErrorBoundary name={t('feature.pdfReader')}>
              {/* V4 导览阅读体验：ReadingContextBar 容器 */}
              <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
                <ReadingContextBarWithTracking
                  guideNodes={guideNodes}
                  blocksData={blocksData}
                  currentPage={currentPage}
                  onNavigate={handleCoordinateNavigate}
                  currentSection={currentSection}
                  currentGuide={currentGuide}
                  anchors={currentAnchors}
                />
                <div style={{ flex: 1, minHeight: 0 }}>
              {resolvedPdfUrl ? (
              <PdfViewer // PDF 阅读器组件（包含统一搜索栏、缩略图侧边栏和 PDF 渲染区域）
                ref={pdfViewerRef}  // 暴露 ref，供父组件调用 goToPage 方法
                paperId={paperId} // 论文 ID（UUID 字符串）
                pdfUrl={resolvedPdfUrl!} // 已解析的绝对 URL（Tauri 模式下包含 sidecar 地址）
                hoverTranslationEnabled={
                  (paper as any)?.paragraphTranslationStatus === 'done' &&
                  (paper as any)?.hoverTranslationEnabled === true
                }
                onContextInject={handleContextInject}  // 接收选中文本的注入回调
                onCoordinateClick={handlePdfCoordinateClick}  // V2 坐标点击回调，用于反向导航到导览面板
                onSearchChange={handleUnifiedSearchChange as any}  // 统一搜索栏内容变化回调，中继搜索关键词到对应面板
                navigation={coordinateNavigation}  // V2 坐标导航数据，传递给 CoordinateHighlightOverlay
                onPageChange={setCurrentPage}  // V4 导览阅读体验：页码变化回调（传递 setState 函数引用，避免内联箭头函数导致 useEffect 重复触发）
                anchors={currentAnchors}  // V4 导览阅读体验：注意力锚点数组，传递给 AnchorOverlay
              />
              ) : (
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '100%' }}>
                  <Spin tip={t('loadingPdf')} />
                </div>
              )}
              </div>
              </div>
            </FeatureErrorBoundary>
          </Suspense>
        )}
      </div>

      {/* 右侧拖拽手柄：用于调整聊天面板宽度 */}
      {/* 仅在聊天面板展开时显示，折叠时隐藏（折叠时无需调整宽度） */}
      {!chatCollapsed && ( // 条件渲染：聊天面板展开时才显示拖拽手柄
        <div
          onMouseDown={handleChatResizeStart}
          style={{
            width: DRAG_HANDLE.WIDTH,
            height: DRAG_HANDLE.HEIGHT,
            cursor: 'col-resize',
            backgroundColor: 'transparent',
            transition: 'background-color 0.2s',
            zIndex: 10,
            flexShrink: 0,
            alignSelf: 'flex-start',
          }}
          title={t('dragHint.chat')}
          className="resize-handle resize-handle-right"
        />
      )}

      {/* 右侧：AI Chat 面板（可折叠，可拖拽调整宽度） */}
      <div style={{ // 右侧聊天面板容器
        width: chatCollapsed ? 0 : chatWidth,  // 折叠时完全隐藏，展开时使用拖拽可调的 chatWidth
        minWidth: chatCollapsed ? 0 : chatWidth, // 最小宽度与 width 保持一致，防止 flex 收缩
        borderLeft: chatCollapsed ? 'none' : '1px solid var(--border-color)', // 折叠时隐藏边框
        // 拖拽时禁用 transition 避免宽度变化延迟，正常状态保留动画
        transition: isDragging ? 'none' : 'width var(--transition-normal)', // 拖拽时无过渡（跟手），其他时候平滑过渡
        display: 'flex', // flex 布局
        flexDirection: 'column', // 纵向排列
        overflow: 'hidden', // 防止内容溢出
      }}>
        {/* 功能级错误边界：包裹 AI 聊天面板，崩溃时只影响此面板 */}
        <FeatureErrorBoundary name={t('feature.aiChat')}>
          <ChatPanel
            ref={chatPanelRef}
            agentType="paperReader"
            paperId={paperId as any}
            markdown={markdown}
            paperInfo={paper}
            paper={paper}
            collapsed={chatCollapsed}
            onToggleCollapse={toggleChat}
            chatSearchKeyword={chatSearchKeyword}
            onChatSearchClear={() => setChatSearchKeyword('')}
          />
        </FeatureErrorBoundary>
      </div>

    </div> {/* 结束三栏布局容器 */}
    {/* 结束纵向 flex 根容器 */}</div>
  );
}
