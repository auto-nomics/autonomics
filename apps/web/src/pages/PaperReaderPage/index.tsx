/**
 * 论文阅读器页面组件
 *
 * 应用的核心页面，提供双栏布局的论文阅读体验：
 * - 左侧：PDF 阅读器（PDFium 渲染），支持选中文本注入到 AI 对话；
 *   题录记录（无 PDF）时显示 MetadataView 元数据视图
 * - 右侧：AI Chat 面板，与论文内容进行对话，支持上下文注入
 *
 * 布局设计：
 * - 双栏布局充分利用宽屏空间，左侧面板自适应剩余宽度
 * - 右侧面板可折叠（Ctrl+Right），专注阅读模式
 * - 面板宽度可拖拽调整，约束在最小/最大值之间
 *
 * 注：jayread 时代的左侧段落导览面板（ParagraphGuideV2/SidebarPanel/
 * ReadingContextBar）已随 guideApi 一并移除 —— autonomics 后端没有
 * MinerU 结构化解析，不存在导览数据源（plan Phase 5 清扫）。
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
import { getPaper, getPdfUrl, getMarkdown } from '../../services/papersApi';
import { getAttachmentUrl, getAttachments } from '../../services/attachmentsApi';
import { isTauri, getTauriBaseUrl } from '../../services/client';

const PdfViewer = lazy(() => import('../../features/pdf-viewer/components/WasmPdfViewer'));
import ChatPanel from '../../features/ai-chat/components/ChatPanel';
import ContextBridge, { buildContextPrompt } from '../../components/ContextBridge';
import FeatureErrorBoundary from '../../components/FeatureErrorBoundary';
import { getTypeLabel, getTypeColor, getTypeFields, METADATA_FIELD_LABELS } from '../../utils/literatureTypes';

import MetadataView from './components/MetadataView';

import { useReaderShortcuts } from './hooks/useReaderShortcuts';
import { usePanelCoordination } from './hooks/usePanelCoordination';
import { DRAG_HANDLE } from './constants';

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


  // 当前是否正在查看非原始 PDF 的附件（用于决定是否显示提示横幅）
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
    chatCollapsed,
    chatWidth,
    toggleChat,
    handleChatResizeStart,
    isDragging,
  } = usePanelCoordination();

  // ========== 统一搜索栏相关状态 ==========
  const [chatSearchKeyword, setChatSearchKeyword] = useState('');

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
    console.log(`[PaperReaderPage render #${renderCountRef.current}]`, { paperId, attachmentId, loading, paper: !!paper, resolvedPdfUrl });
  }

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
        // 如果不是原始 PDF，则设置 isViewingNonPrimary 为 true，用于显示
        // "正在查看附件 PDF"提示横幅
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
  // 已由 usePanelCoordination hook 管理（handleChatResizeStart）

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
                <div style={{ flex: 1, minHeight: 0 }}>
              {resolvedPdfUrl ? (
              <PdfViewer // PDF 阅读器组件（包含统一搜索栏、缩略图侧边栏和 PDF 渲染区域）
                ref={pdfViewerRef}  // 暴露 ref，供父组件调用 goToPage 方法
                paperId={paperId} // 论文 ID（UUID 字符串）
                pdfUrl={resolvedPdfUrl!} // 已解析的绝对 URL（Tauri 模式下包含 sidecar 地址）
                onContextInject={handleContextInject}  // 接收选中文本的注入回调
                onSearchChange={handleUnifiedSearchChange as any}  // 统一搜索栏内容变化回调，中继搜索关键词到对应面板
              />
              ) : (
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '100%' }}>
                  <Spin tip={t('loadingPdf')} />
                </div>
              )}
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
