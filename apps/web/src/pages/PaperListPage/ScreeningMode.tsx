/**
 * ScreeningMode - 筛选模式三栏布局组件
 *
 * 本文件实现筛选模式（Ctrl+→）的三栏布局，包括：
 * - 左面板：分类树 / 论文详情（标题/摘要+翻译）可切换
 * - 中间面板：论文列表（可点击选中论文）
 * - 右面板：AI 对话面板（针对选中论文或整个列表提问）
 *
 * 筛选模式功能：
 * - 快速浏览论文：点击论文行在左面板显示标题/摘要+中文翻译
 * - AI 辅助筛选：在右面板针对单篇论文或列表级对话
 * - 面板宽度可拖拽调整：左面板 [200, 480]px，右面板 [280, 600]px
 *
 * 与正常模式的区别：
 * - 正常模式：2 栏布局（分类树 + 论文列表），点击标题展开附件
 * - 筛选模式：3 栏布局，点击标题选中论文（不展开附件），左面板切换为论文详情
 *
 * 为什么需要独立组件？
 * - 筛选模式布局独立，包含复杂的面板宽度管理
 * - 翻译缓存、AI 对话等状态与筛选模式强相关
 * - 便于后续扩展（如添加新的筛选模式功能）
 */

import React, { useCallback, useEffect, useMemo, useRef } from 'react'; // 导入 React 核心钩子
import usePaperStore from '../../stores/usePaperStore'; // 导入论文列表 Store
import CategorySidebar from './components/category-sidebar/CategorySidebar'; // 导入分类侧边栏组件
import ScreeningPaperDetail from './ScreeningPaperDetail'; // 导入筛选模式论文详情组件
import ChatPanel from '../../features/ai-chat/components/ChatPanel'; // 导入 AI 对话面板组件
import { useResizablePanel } from '../../hooks/useResizablePanel'; // 导入面板宽度拖拽 hook

/**
 * 筛选模式组件
 *
 * @param {Object} props - 组件属性
 * @param {Array} props.papers - 论文列表数据
 * @param {Function} props.loadPapers - 刷新论文列表的回调
 * @param {Paper|null} props.selectedPaperDetail - 选中文献的完整详情（来自 GET /api/papers/{id}，
 *   含 abstract / title_zh / abstract_zh / paragraphTranslationStatus）
 * @param {boolean} props.detailLoading - 详情是否仍在加载（含翻译生成轮询阶段）
 * @param {React.RefObject} props.categorySidebarRef - 分类侧边栏 ref 引用
 * @param {Function} props.handleCategorySelect - 分类选择回调
 * @param {Function} props.handleDirectOnlySelect - 右键菜单"仅显示直属论文"回调
 * @param {Function} props.handleUploadToCategory - 右键菜单"上传文件"回调
 * @param {Function} props.handleImportBibliography - 右键菜单"导入题录"回调
 * @param {Function} props.renderPaperTable - 渲染论文表格的函数
 * @param {Object|null} props.selectedPaperForScreening - 当前选中的论文对象
 * @param {Function} props.setSelectedPaperForScreening - 设置选中论文回调
 * @param {string} props.leftPanelView - 左面板视图类型 ('category' | 'paper-detail')
 * @param {Function} props.setLeftPanelView - 设置左面板视图回调
 * @returns {JSX.Element} 筛选模式三栏布局
 */
export default function ScreeningMode({
  papers, // 论文列表数据
  loadPapers, // 刷新论文列表回调
  selectedPaperDetail, // 选中文献的完整详情（detail API 单源）
  detailLoading, // 详情加载中（含翻译生成轮询）
  categorySidebarRef, // 分类侧边栏 ref
  handleCategorySelect, // 分类选择回调
  handleDirectOnlySelect, // 直属模式选择回调
  handleUploadToCategory, // 上传到分类回调
  handleImportBibliography, // 导入题录回调
  renderPaperTable, // 渲染论文表格函数
  selectedPaperForScreening, // 当前选中的论文对象
  setSelectedPaperForScreening, // 设置选中论文回调
  leftPanelView, // 左面板视图类型
  setLeftPanelView, // 设置左面板视图回调
}: any) {
  // ========================================
  // 从 Store 读取共享状态
  // ========================================

  /**
   * 从 Zustand Store 读取筛选模式相关的共享状态
   * - screeningMode: 筛选模式开关
   * - selectedCategoryId: 当前选中的分类 ID
   */
  const screeningMode = usePaperStore(s => s.screeningMode); // 筛选模式开关
  const selectedCategoryId = usePaperStore(s => s.selectedCategoryId); // 当前选中的分类 ID

  // ========================================
  // 筛选模式状态管理
  // ========================================

  /**
   * ChatPanel 组件的 ref 引用
   *
   * 用于调用 sendMessage 等命令式方法（如快捷键触发 AI 对话）。
   *
   * @type {React.MutableRefObject}
   */
  const screeningChatPanelRef = useRef(null); // 筛选模式下的 ChatPanel ref
  // 中间论文表格的容器 ref——传给 PaperTable 用于 ResizeObserver 测算 scrollY，
  // 避开"window.innerHeight - 魔法数"那种硬编码（参见 PaperTable 的注释）。
  const tableContainerRef = useRef<HTMLDivElement>(null);

  // ========================================
  // 面板宽度管理
  // ========================================

  /**
   * 左面板（分类树或论文详情）宽度管理
   *
   * 复用 useResizablePanel hook，实现拖拽调整宽度。
   * 默认 280px，可拖拽范围 [200, 480]px。
   */
  const { width: screeningLeftWidth, handleResizeStart: handleLeftResizeStart } = useResizablePanel({
    defaultWidth: 280, // 默认宽度 280px
    minWidth: 200, // 最小宽度 200px
    maxWidth: 480, // 最大宽度 480px
    reverse: false, // 左面板：向右拖拽=变宽（非反向）
  });

  /**
   * 右面板（AI 对话面板）宽度管理
   *
   * 复用 useResizablePanel hook，实现拖拽调整宽度。
   * 默认 380px，可拖拽范围 [280, 600]px。
   * reverse: true 表示向左拖拽=变宽（与 PaperReaderPage 的 ChatPanel 一致）。
   */
  const { width: screeningRightWidth, handleResizeStart: handleRightResizeStart } = useResizablePanel({
    defaultWidth: 380, // 默认宽度 380px
    minWidth: 280, // 最小宽度 280px
    maxWidth: 600, // 最大宽度 600px
    reverse: true, // 右面板：向左拖拽=变宽（反向）
  });

  // ========================================
  // ChatPanel 上下文计算
  // ========================================

  /**
   * 筛选模式 ChatPanel 的 paperId（对话持久化标识）
   *
   * 使用 useMemo 缓存计算结果，避免每次渲染重新计算。
   *
   * 两种场景：
   * - 选中论文时：使用论文真实 ID，ChatPanel 会加载该论文的历史对话
   * - 未选中论文时：使用负数 ID（基于分类 ID 生成），为列表级对话创建独立的持久化空间
   *
   * 为什么未选中时使用负数 ID？
   * - 后端 chat API 不校验 paper_id 是否存在于 papers 表中
   * - 负数 ID 保证不与真实论文 ID 冲突（自增 ID 始终为正数）
   * - 不同分类使用不同负数 ID，确保各分类的列表级对话互不干扰
   *
   * 生成规则：
   * - 全部分类（null）→ -1
   * - 未分类（'uncategorized'）→ -2
   * - 具体分类（数字 ID）→ -(ID + 100)，偏移 100 避免与 -1/-2 冲突
   *
   * @returns {number} ChatPanel 使用的 paperId
   */
  const screeningChatPaperId = useMemo(() => {
    if (selectedPaperForScreening) { // 如果选中了论文
      return selectedPaperForScreening.id; // 使用论文真实 ID
    }

    // 未选中论文时，根据分类生成负数 ID
    if (selectedCategoryId === null) { // 全部分类
      return -1; // 使用 -1 代表"全部分类"的列表级对话
    }
    if (selectedCategoryId === 'uncategorized') { // 未分类
      return -2; // 使用 -2 代表"未分类"的列表级对话
    }
    return -(Number(selectedCategoryId) + 100); // 具体分类：偏移 100 避免与 -1/-2 冲突
  }, [selectedPaperForScreening, selectedCategoryId]); // 依赖选中的论文和分类

  /**
   * 筛选模式 ChatPanel 的 paperInfo（AI 上下文信息）
   *
   * 使用 useMemo 缓存计算结果，避免每次渲染重新构建。
   *
   * 两种场景：
   * - 选中论文时：返回单篇论文的详细信息（标题、作者、摘要）
   *   用于 Layer 1 上下文注入，AI 可以深入讨论该论文的具体内容
   * - 未选中论文时：返回合成信息，包含当前分类下所有论文的粗略列表
   *   用于列表级上下文注入，AI 可以帮助用户横向比较多篇文献
   *
   * 合成信息的格式：
   * - title: 描述性标题，如"全部分类文献列表（共 15 篇）"或"机器学习分类文献列表（共 5 篇）"
   * - abstract: 格式化的论文列表，每篇包含序号、标题、作者、期刊、年份
   *   不限制数量，由 token 预算机制（Layer 分层注入）自动控制上下文长度
   *
   * @returns {Object} ChatPanel 使用的 paperInfo 对象
   */
  const screeningChatPaperInfo = useMemo(() => {
    if (selectedPaperForScreening) { // 如果选中了论文
      // 返回单篇论文的详细信息，用于深度阅读场景
      return {
        title: selectedPaperForScreening.title, // 论文标题
        authors: selectedPaperForScreening.authors, // 作者列表
        abstract: selectedPaperForScreening.abstract, // 摘要内容
      };
    }

    // 未选中论文时，构建列表级合成信息，用于多篇文献横向比较场景
    // 分类名称：根据 selectedCategoryId 生成描述性名称
    const categoryName = selectedCategoryId === null // 判断分类类型
      ? '全部分类' // null 表示所有分类
      : selectedCategoryId === 'uncategorized' // 判断是否为"未分类"
        ? '未分类' // 未分类特殊标签
        : `当前分类`; // 具体分类（分类名需要从侧边栏获取，这里用通用描述）

    // 构建论文列表的摘要文本：每篇一行，包含序号、标题、作者、期刊、年份
    // 不限制数量，由 Layer 分层注入的 token 预算机制自动控制上下文长度
    const paperListSummary = papers.map((p: any, i: any) => { // 遍历全部论文列表
      // 每篇论文构建一行信息
      const parts = [ // 信息片段数组
        `${i + 1}. **${p.title || '未命名'}**`, // 序号 + 标题（加粗）
      ];
      if (p.authors && p.authors.length > 0) parts.push(`作者: ${Array.isArray(p.authors) ? p.authors.join(', ') : p.authors}`); // 作者（如果有）
      if (p.journal_name) parts.push(`期刊: ${p.journal_name}`); // 期刊（如果有）
      if (p.publication_year) parts.push(`年份: ${p.publication_year}`); // 年份（如果有）
      return parts.join(' | '); // 用 | 分隔各信息片段
    }).join('\n'); // 每篇论文占一行

    // 返回合成信息对象
    return {
      title: `${categoryName}文献列表（共 ${papers.length} 篇）`, // 描述性标题
      abstract: paperListSummary, // 格式化的论文列表摘要
    };
  }, [selectedPaperForScreening, selectedCategoryId, papers]); // 依赖选中的论文、分类和论文列表

  // ========================================
  // 快捷键处理
  // ========================================

  // ========================================
  // 渲染
  // ========================================

  return (
    <div style={{ display: 'flex', height: '100vh' }}> {/* flex 布局，占满视口高度 */}

      {/* ========== 左面板：分类树 或 论文详情+翻译 ========== */}
      <div style={{
        width: screeningLeftWidth, // 由 useResizablePanel hook 管理
        minWidth: 200, // 最小宽度
        maxWidth: 480, // 最大宽度
        borderRight: '1px solid var(--border-color)', // 右边框分割线
        display: 'flex', // 弹性布局
        flexDirection: 'column', // 垂直排列
        overflow: 'hidden', // 隐藏溢出
        transition: 'width 0.2s ease', // 宽度变化动画
      }}>
        {leftPanelView === 'category' ? (
          // 显示分类树（复用 CategorySidebar 组件）
          <CategorySidebar
            ref={categorySidebarRef} // ref：暴露 refreshCategories 方法
            selectedCategoryId={selectedCategoryId} // 当前选中的分类 ID
            onSelectCategory={handleCategorySelect} // 分类选择回调
            onPapersUpdate={loadPapers} // 论文列表更新回调
            onSelectDirectOnly={handleDirectOnlySelect} // 右键菜单"仅显示直属论文"回调
            onUploadToCategory={handleUploadToCategory} // 右键菜单"上传文件"回调
            onImportBibliography={handleImportBibliography} // 右键菜单"导入题录"回调
            fillParent // 筛选模式下填满左面板宽度，由外部面板控制宽度和边框
          />
        ) : (
          // 显示论文详情+翻译（使用 ScreeningPaperDetail 组件）
          <ScreeningPaperDetail
            detail={selectedPaperDetail} // 完整详情（含双语）
            loading={detailLoading} // 详情+翻译轮询状态
          />
        )}
      </div>

      {/* 左面板拖拽手柄（绝对定位，不占据布局空间） */}
      <div
        onMouseDown={handleLeftResizeStart} // 绑定拖拽调整宽度事件
        style={{
          position: 'relative', // 相对定位，内部手柄绝对定位
          width: 0, // 宽度为 0，不占据布局空间
          flexShrink: 0, // 不被压缩
          zIndex: 10, // 层级高于内容，确保可点击
        }}
      >
        <div style={{
          position: 'absolute', // 绝对定位，覆盖在面板边界上
          top: 0, // 顶部对齐
          bottom: 0, // 底部对齐
          left: -3, // 向左偏移 3px，使手柄居中于面板边界
          width: 6, // 手柄宽度 6px
          cursor: 'col-resize', // 鼠标指针变为水平调整图标
        }} />
      </div>

      {/* ========== 中间：论文表格 ========== */}
      <div style={{
        flex: 1, // 占据剩余空间
        overflowX: 'hidden', // 隐藏水平滚动条
        overflowY: 'auto', // 垂直方向内容超出时滚动
        display: 'flex', // 弹性布局
        flexDirection: 'column', // 垂直排列
      }}>
        <div ref={tableContainerRef} style={{ flex: 1, overflow: 'auto' }}>
          {renderPaperTable(tableContainerRef)} {/* 渲染论文表格 */}
        </div>
      </div>

      {/* 右面板拖拽手柄（始终显示，绝对定位不占布局空间） */}
      <div
        onMouseDown={handleRightResizeStart} // 绑定拖拽调整宽度事件
        style={{
          position: 'relative', // 相对定位，内部手柄绝对定位
          width: 0, // 宽度为 0，不占据布局空间
          flexShrink: 0, // 不被压缩
          zIndex: 10, // 层级高于内容，确保可点击
        }}
      >
        <div style={{
          position: 'absolute', // 绝对定位，覆盖在面板边界上
          top: 0, // 顶部对齐
          bottom: 0, // 底部对齐
          left: -3, // 向左偏移 3px，使手柄居中于面板边界
          width: 6, // 手柄宽度 6px
          cursor: 'col-resize', // 鼠标指针变为水平调整图标
        }} />
      </div>

      {/* ========== 右面板：AI 对话面板（始终展开） ========== */}
      {/*
       * 无论是否选中论文，右面板始终显示 ChatPanel 组件。
       *
       * 两种上下文模式：
       * - 选中论文（selectedPaperForScreening 存在）：
       *   screeningChatPaperId = 论文真实 ID
       *   screeningChatPaperInfo = 单篇论文详细信息（标题、作者、摘要）
       *   → AI 进行深度单篇论文阅读和讨论
       *
       * - 未选中论文（selectedPaperForScreening 为 null）：
       *   screeningChatPaperId = 负数 ID（基于分类生成，保证独立性）
       *   screeningChatPaperInfo = 当前分类下论文列表的粗略信息（标题、作者、期刊、年份）
       *   → AI 帮助用户横向比较多篇文献，辅助筛选决策
       *
       * onToggleCollapse 设为空函数：
       * - ChatPanel 的 /hide 命令会调用此函数
       * - 筛选模式下不需要折叠功能，传入空函数静默忽略
       */}
      <div style={{
        width: screeningRightWidth, // 由 useResizablePanel hook 管理的宽度
        minWidth: 280, // 最小宽度 280px
        maxWidth: 600, // 最大宽度 600px
        borderLeft: '1px solid var(--border-color)', // 左边框分割线
        display: 'flex', // 弹性布局
        flexDirection: 'column', // 垂直排列
        overflow: 'hidden', // 隐藏溢出
        transition: 'width 0.2s ease', // 宽度变化时的过渡动画
      }}>
        <ChatPanel
          ref={screeningChatPanelRef} // ChatPanel 的 ref 引用
          agentType="screening" // 筛选模式专属 persona（快速分诊多篇）
          paperId={screeningChatPaperId} // 对话持久化 ID：选中论文用真实 ID，未选中用负数 ID
          paperInfo={screeningChatPaperInfo} // AI 上下文：单篇详情或列表粗略信息
          paper={selectedPaperForScreening || undefined} // 完整论文对象（未选中时为 undefined）
          collapsed={false} // 始终展开，不使用折叠功能
          onToggleCollapse={() => {}} // 空函数：筛选模式下忽略折叠操作
        />
      </div>
    </div>
  );
}

/**
 * 导出类型定义（用于 TypeScript 项目）
 * 本项目使用 JavaScript，保留类型注释供参考
 */
/**
 * @typedef {Object} ScreeningModeProps
 * @property {Array} papers - 论文列表数据
 * @property {Function} loadPapers - 刷新论文列表的回调
 * @property {Paper|null} selectedPaperDetail - 选中文献的完整详情（来自 GET /api/papers/{id}）
 * @property {boolean} detailLoading - 详情是否仍在加载（含翻译生成轮询阶段）
 * @property {React.RefObject} categorySidebarRef - 分类侧边栏 ref 引用
 * @property {Function} handleCategorySelect - 分类选择回调
 * @property {Function} handleDirectOnlySelect - 直属模式选择回调
 * @property {Function} handleUploadToCategory - 上传到分类回调
 * @property {Function} handleImportBibliography - 导入题录回调
 * @property {Function} renderPaperTable - 渲染论文表格的函数
 * @property {Object|null} selectedPaperForScreening - 当前选中的论文对象
 * @property {Function} setSelectedPaperForScreening - 设置选中论文回调
 * @property {string} leftPanelView - 左面板视图类型 ('category' | 'paper-detail')
 * @property {Function} setLeftPanelView - 设置左面板视图回调
 */
