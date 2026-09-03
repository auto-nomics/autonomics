/**
 * JayRead 论文列表页组件
 *
 * 这是应用的首页，功能包括：
 * 1. 左侧分类侧边栏：树形分类目录，分门别类管理文献
 * 2. PDF 文件上传：支持拖拽和点击上传
 * 3. 论文列表展示：显示所有已上传的论文及其状态，支持按分类筛选
 * 4. 实时解析进度：通过 SSE 监听后端解析进度推送
 * 5. 论文管理：删除论文、重新解析、分配到分类
 * 6. 筛选模式（Ctrl+→）：3栏布局，快速浏览标题/摘要+AI翻译+AI对话
 *
 * 布局结构：
 * - 左侧：CategorySidebar（可拖拽调整宽度的分类树，默认 240px）
 * - 右侧：主内容区（上传区域 + 论文列表）
 *
 * 为什么使用 SSE 而不是轮询获取解析状态？
 * - SSE 允许服务器主动推送，延迟更低（< 100ms vs 2-5s）
 * - 减少无效请求，降低服务器负载
 * - 浏览器原生支持，自动重连机制
 */
import React, { useState, useEffect, useCallback, useRef, useMemo } from 'react'; // 导入 React 核心钩子：状态、副作用、回调缓存、引用、计算缓存
import { useTranslation } from 'react-i18next'; // 导入 i18n 翻译钩子
import { // 从 Ant Design 导入 UI 组件
  Tag, Space, App, Spin, Tooltip, // Tag 标签、Space 间距、App（提供 message 上下文）、Spin 加载动画、Tooltip 提示
  Dropdown, // 下拉菜单：用于右键菜单渲染（独立定位方式）
  Table, Checkbox, // Table 表格：用于 Zotero/EndNote 风格的论文列表展示；Checkbox 复选框：用于列可见性切换
} from 'antd';
import { // 从 Ant Design Icons 导入图标组件
  // DeleteOutlined, ReloadOutlined, CheckCircleOutlined 已移至 paperListConstants.jsx
  // SearchOutlined, UploadOutlined, CloudDownloadOutlined 已移至 usePaperContextMenu hook
  // 以上图标仅在常量定义和右键菜单项中使用，各自模块内部自行导入
  FolderOutlined, // 文件夹图标：用于标题列 Tooltip 中的分类路径显示
  FileTextOutlined, // 文件文本图标：用于题录导入记录的标识（表格视图标题列需要）
  CloseOutlined, // 关闭图标：用于 Tooltip 中删除分类路径
} from '@ant-design/icons';
import { useNavigate } from 'react-router-dom'; // 导入路由导航钩子，用于编程式跳转
import { // 从论文 API 模块导入接口函数
  getPapers, // 获取列表（deletePaper 已移至 usePaperContextMenu hook）
} from '../../services/papersApi';
import type { GetPapersParams } from '../../types';
// 附件 API 已移至 useAttachmentManager hook 内部导入
// 导入附件列表组件：在展开行中显示论文的所有附件
import AttachmentList from './components/AttachmentList';
// 导入分类侧边栏组件：左侧树形分类目录
import CategorySidebar from './components/category-sidebar/CategorySidebar';
import NiceModal from '@ebay/nice-modal-react';
// 分类 API（assignPapers/unassignPapers）已移至 usePaperContextMenu hook 内部导入
// 此处不再需要导入分类 API，避免冗余

// ========== 从提取的模块导入 ==========
// 导入常量配置（状态、引擎、OA、列定义、排序映射、日期格式化）
import {
  STATUS_CONFIG,           // 解析状态配置映射表（pending/parsing/done/failed）
  OA_STATUS_CONFIG,        // 开放获取状态配置映射表
  ALL_COLUMNS,             // 表格视图的所有可用列定义
  DEFAULT_VISIBLE_COLUMNS, // 默认可见列配置
  SORT_FIELD_MAP,          // 表格列 key 到后端排序字段名的映射
  formatDate,              // 日期格式化工具函数
} from './paperListConstants';
// 导入可拖拽调整宽度的表头单元格组件
import ResizableTitle from './ResizableTitle';
// 导入 SSE 进度监听钩子
import { useSSEProgress } from './hooks/useSSEProgress';
// 导入元数据获取轮询钩子：检测后台元数据获取是否完成，完成后自动刷新列表
// 从共享 hooks 目录导入，供多个页面复用
// 导入键盘快捷键钩子
import useKeyboardShortcut from '../../hooks/useKeyboardShortcut';
// 导入题录导入和去重钩子
import { useBibliographyImport } from './hooks/useBibliographyImport';
// 导入 PDF 文件上传钩子
import { usePaperUpload } from './hooks/usePaperUpload';
// ScreeningPaperDetail、ChatPanel、useResizablePanel 已移至 ScreeningMode 组件内部导入
// 这些模块仅在筛选模式三栏布局中使用，ScreeningMode 组件自行导入和管理
// ========== 从提取的模块导入 ==========
// 导入右键菜单和分类分配钩子
import { usePaperContextMenu } from './hooks/usePaperContextMenu';
// 导入列配置钩子
import { useColumnConfig } from './hooks/useColumnConfig';
// 导入共享常量
import { STORAGE_KEYS, PANEL_CONSTRAINTS } from '../../config/constants';
// 导入活跃列配置钩子
import useActiveColumns from './hooks/useActiveColumns';
// 导入翻译缓存管理钩子
import { useSelectedPaperDetail } from './hooks/useSelectedPaperDetail';
// 导入附件管理钩子
import { useAttachmentManager } from './hooks/useAttachmentManager';
// 导入筛选模式组件
import ScreeningMode from './ScreeningMode';
// 导入论文表格组件
import PaperTable from './components/PaperTable';
// 导入论文列表 Store：读取共享状态
import usePaperStore from '../../stores/usePaperStore';
// 导入 PaperList 上下文
import { PaperListProvider, usePaperListConfig } from './PaperListContext';
// 导入功能级错误边界组件：用于捕获页面渲染错误，提供降级 UI
import FeatureErrorBoundary from '../../components/FeatureErrorBoundary';
import FileDropzoneOverlay from './components/FileDropzoneOverlay';
import { useFileDropzone } from './hooks/useFileDropzone';

/**
 * 自定义表格行组件
 *
 * 为表格的每一行添加拖拽支持和右键上下文菜单。
 * 使用 Ant Design Table 的 components.body.row 属性替换默认的行渲染。
 *
 * 为什么使用 <div> 而不是 <tr>？
 * - antd virtual table 使用 <div> 布局，自定义行组件必须返回 <div>
 * - 返回 <tr> 会导致 DOM 嵌套警告（<tr> 不能作为 <div> 的子元素）
 *
 * 为什么不使用 Dropdown 包裹行元素？
 * - Ant Design 的 Dropdown 内部使用 React.Children.only 验证子元素
 * - 解决方案：在行元素上监听 onContextMenu 事件，通过状态驱动一个独立的 Dropdown
 *
 * 为什么使用 React.memo 而非 useCallback？
 * - useCallback 只缓存函数引用，不防止组件重新渲染
 * - React.memo 对 props 进行浅比较，当 props 未变化时跳过重新渲染
 * - 当 Table 中某一行数据变化时，其他未变化的行可以跳过渲染
 *
 * @param {Object} props - 组件属性
 * @param {string} props['data-row-key'] - 论文 ID
 * @param {boolean} props.screeningMode - 筛选模式开关
 * @param {Object} props.selectedPaperForScreening - 当前选中的筛选论文
 * @param {Function} props.onContextMenu - 右键菜单回调
 * @param {Array} props.papers - 论文列表（用于 onClick 查找论文）
 * @param {Function} props.onSelectPaper - 选中论文回调
 * @param {Function} props.onChangeLeftPanelView - 切换左面板视图回调
 * @returns {JSX.Element} 自定义的行元素
 */
const TableRowWithDragAndContextMenu = React.memo(({
  'data-row-key': paperId,
  screeningMode,
  selectedPaperForScreening,
  onContextMenu,
  papers,
  onSelectPaper,
  onChangeLeftPanelView,
  ...restProps
}: any) => {
  return (
    <div
      {...restProps}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData('application/jayread-paper', String(paperId));
        e.dataTransfer.effectAllowed = 'move';
        requestAnimationFrame(() => {
          e.currentTarget.style.opacity = '0.5';
        });
      }}
      onDragEnd={(e) => {
        e.currentTarget.style.opacity = '1';
      }}
      onClick={(e) => {
        if (screeningMode) {
          const paper = papers.find((p: any) => p.id === paperId);
          if (paper) {
            onSelectPaper(paper);
            onChangeLeftPanelView('paper-detail');
          }
        }
      }}
      onContextMenu={(e) => {
        if (paperId != null) onContextMenu(e, paperId);
      }}
      className={
        screeningMode && paperId === selectedPaperForScreening?.id
          ? `${restProps.className || ''} screening-selected-row`
          : restProps.className
      }
      style={{
        ...restProps.style,
        cursor: 'grab',
      }}
    />
  );
});

/**
 * 论文列表页主组件
 *
 * 状态管理：
 * - papers: 论文列表数据
 * - loading: 初始加载状态
 * - uploading: 上传中的状态
 * - sseRefs: 使用 ref 存储活跃的 SSE 连接（不触发重渲染）
 */

/**
 * 构建论文-分类映射表
 *
 * 后端返回的论文列表中每个论文包含 category_ids 字段，
 * 此函数将其转换为 id -> category_ids 的映射表，便于快速查找。
 *
 * @param {Array} papers - 论文列表数组
 * @returns {Object} 论文 ID 到分类 ID 数组的映射 { paperId: [categoryId1, categoryId2, ...] }
 */
function buildPaperCategoryMap(papers: any[]) {
  if (!papers || papers.length === 0) {
    return {};
  }
  const newMap: any = {};
  papers.forEach((paper: any) => {
    newMap[paper.id] = paper.category_ids || [];
  });
  return newMap;
}

/**
 * Inner component that has access to the PaperList config
 */
function PaperListPageInner() {
  // 路由导航函数，用于跳转到阅读器页面
  const navigate = useNavigate(); // 获取路由导航函数
  const { message: messageApi } = App.useApp(); // 获取 antd message 上下文（避免静态方法警告）
  const { t } = useTranslation('paperList'); // i18n 翻译函数

  // 获取 PaperList 配置 ref（用于在 hooks 间共享回调）
  const configRef = usePaperListConfig();

  // ========== 从 Zustand Store 读取共享状态 ==========
  // 论文列表数据、加载状态、排序配置、分类筛选
  const papers = usePaperStore((s: any) => s.papers); // 论文列表数据
  const loading = usePaperStore((s: any) => s.loading); // 加载状态
  const setPapers = usePaperStore((s: any) => s.setPapers); // 设置论文列表
  const setLoading = usePaperStore((s: any) => s.setLoading); // 设置加载状态
  const sortField = usePaperStore((s: any) => s.sortField); // 排序字段
  const setSortField = usePaperStore((s: any) => s.setSortField); // 设置排序字段
  const sortOrder = usePaperStore((s: any) => s.sortOrder); // 排序方向
  const setSortOrder = usePaperStore((s: any) => s.setSortOrder); // 设置排序方向
  const selectedCategoryId = usePaperStore((s: any) => s.selectedCategoryId); // 选中的分类 ID
  const setSelectedCategoryId = usePaperStore((s: any) => s.setSelectedCategoryId); // 设置选中的分类 ID
  const directOnly = usePaperStore((s: any) => s.directOnly); // 是否仅显示直属论文
  const setDirectOnly = usePaperStore((s: any) => s.setDirectOnly); // 设置直属模式
  const screeningMode = usePaperStore((s: any) => s.screeningMode); // 筛选模式状态
  const setScreeningMode = usePaperStore((s: any) => s.setScreeningMode); // 设置筛选模式状态的函数，用于快捷键切换

  /**
   * 筛选模式快捷键：Ctrl+→ 切换筛选模式开/关
   *
   * 此监听器注册在 PaperListPage 而非 ScreeningMode 组件中，
   * 因为 ScreeningMode 仅在筛选模式开启时才挂载，
   * 放在那里会导致关闭状态下无法通过快捷键重新开启（鸡生蛋问题）。
   * PaperListPage 作为路由页面始终挂载，确保快捷键在任意状态下都能响应。
   *
   * 快捷键在以下元素获得焦点时被忽略，避免干扰正常输入：
   * - INPUT 输入框（如搜索框）
   * - TEXTAREA 文本域（如备注编辑）
   * - contentEditable 富文本编辑器
   */
  useKeyboardShortcut(
    (e: any) => e.ctrlKey && e.key === 'ArrowRight',
    (e: any) => {
      e.preventDefault();
      setScreeningMode(!usePaperStore.getState().screeningMode);
    },
    [setScreeningMode]
  );

  // 上传状态已移至 usePaperUpload hook，通过解构获取 uploading

  // ===== 表格视图相关状态 =====
  // visibleColumns, columnWidths, headerContextMenu 已移至 useColumnConfig hook

  // ===== 附件展开行相关状态 =====

  // 已展开的论文行 key 列表（论文 ID 数组）
  // ===== 附件展开行相关状态 =====

  // 表格容器 ref：用于挂载原生拖拽事件监听器（外部文件拖入论文行上传为附件）
  const tableWrapperRef = useRef(null);
  // loadPapers ref：供 hook 在 handleSetPrimary 中延迟调用
  const loadPapersRef = useRef<any>(null);

  // 附件管理 hook
  const {
    expandedRowKeys, setExpandedRowKeys,
    attachmentCache, attachmentCacheRef,
    attachmentLoading,
    loadAttachments, handleUploadAttachments,
    handleDeleteAttachment, handleSetPrimary,
    handleOpenAttachment, handleExpand,
  } = useAttachmentManager({
    navigate,
    tableWrapperRef,
    onSetPrimary: useCallback(async () => { (loadPapersRef.current as any)?.(); }, []),
  });

  // ===== 筛选模式（Screening Mode）状态 =====
  // 筛选模式相关状态（screeningMode, selectedPaperForScreening, leftPanelView, 面板宽度等）
  // 已移至 ScreeningMode 组件，通过 usePaperStore 读取 screeningMode 状态

  // 筛选模式右面板（AI 对话）始终展开，不再使用折叠逻辑
  // 原因：无论是否选中论文，AI 面板都保持可见
  // - 选中论文时：注入单篇论文详细信息，便于深度阅读
  // - 未选中论文时：注入当前文献列表的粗略信息，便于横向比较多篇文献

  // 筛选模式 ChatPanel 的 paperId 和 paperInfo 计算逻辑
  // 已移至 ScreeningMode 组件内部管理，此处不再需要
  // 原因：ScreeningMode 组件拥有自己的 selectedPaperForScreening 状态，
  // 能更准确地计算 ChatPanel 所需的 paperId 和 paperInfo

  // 筛选模式快捷键：Ctrl+→ 切换 - 已移至 ScreeningMode 组件

  // 附件管理已提取至独立 hook（useAttachmentManager）

  // 组件卸载时清理所有 timeout，防止内存泄漏
  useEffect(() => {
    return () => {
      timeoutRefs.current.forEach(id => clearTimeout(id));
    };
  }, []);

  // ResizableTitle 组件已提取至 ./components/ResizableTitle.jsx，通过顶部 import 导入
  // 此处保留对组件的引用，便于在 Table components 配置中使用
  // 注意：提取的组件接口与原内联版本完全一致（props: onResize, width, onClick, style, className, ...restProps）

  // ========== 使用提取的 Hooks ==========

  // SSE 重连控制 ref：确保页面刷新后的初始 SSE 重连只执行一次
  // 使用 ref 而非 state，因为重连是一次性操作，不需要触发重渲染
  const hasReconnectedRef = useRef(false); // ref：是否已执行过初始 SSE 重连，初始为 false
  const categorySidebarRef = useRef<any>(null); // ref：CategorySidebar 组件引用，用于在删除论文后刷新分类计数
  // 存储 setTimeout ID，用于组件卸载时清理，防止内存泄漏
  const timeoutRefs = useRef<any[]>([]); // ref：存储所有 timeout ID，用于 cleanup

  /**
   * 加载论文列表（支持按分类筛选）
   *
   * 根据 selectedCategoryId 状态构建不同的查询参数：
   * - null：不加过滤，返回所有论文
   * - 'uncategorized'：只返回未归入任何分类的论文
   * - 数字 ID：只返回该分类下的论文
   *
   * 同时加载分类关联数据，用于在列表项上显示分类标签。
   *
   * 为什么使用 useCallback？
   - 作为 useEffect 的依赖项，避免无限循环
   - 函数引用稳定，减少子组件不必要的重渲染
   */
  const loadPapers = useCallback(async () => { // 用 useCallback 缓存函数，避免每次渲染重建
    try { // 开始 try-catch 错误处理
      // 构建查询参数：基础排序 + 分类筛选 + page_size（与后端 crud.rs clamp 上限对齐）
      // PaperListPage 用 antd 虚拟列表 + 无分页 UI，期望一次拉完。
      // 真到 1000+ 篇时再加无限滚动；现阶段先靠这个硬上限撑住。
      const params: GetPapersParams = {
        sort: sortField,
        order: sortOrder,
        pageSize: 1000,
      };

      // 根据 selectedCategoryId 添加分类筛选参数
      if (selectedCategoryId === 'uncategorized') { // 如果选中的是"未分类"
        params.uncategorized = true; // 设置未分类筛选标志
      } else if (selectedCategoryId !== null) { // 如果选中的是具体分类（UUID）
        params.categoryId = selectedCategoryId; // 设置分类 ID 过滤参数（camelCase 匹配后端 serde rename）
        // 后端 SQL `ic."categoryId" = $N` 默认只查直属分类的论文。
        // directOnly 是前端状态标识（用于 UI 高亮 / 后续行为分支），
        // 当前不需要给后端发任何额外参数——后端已经默认是"仅直属"。
        // 后续若实现"包含子孙分类"作为默认, 再加 direct_only=false 切换。
      }

      // 调用 API 获取论文列表，传递排序和过滤参数
      const data = await getPapers(params); // 异步调用获取论文列表接口
      const fetched = data.papers || []; // 防御性处理 null/undefined
      setPapers(fetched); // 设置论文列表状态

      // 截断告警：total > fetched.length 说明触顶了，需要做无限滚动
      if (typeof data.total === 'number' && data.total > fetched.length) {
        console.warn(
          `[loadPapers] 列表已截断：返回 ${fetched.length} 条，实际 ${data.total} 条。` +
          `当前 page_size=1000 上限被触顶，需要做无限滚动才能看到全部。`
        );
      }

      // 加载论文的分类关联数据（用于显示分类标签）
      // 后端现在直接在论文列表响应中包含 category_ids，无需额外查询（已消除 N+1 查询问题）
      setPaperCategoryMap(buildPaperCategoryMap(fetched));

      // 分类树数据缓存（categoriesCache）已移至 usePaperContextMenu hook 管理
      // hook 在组件挂载时自动调用 getCategoryTree() 加载分类树数据
      // 此处不再重复加载，避免冗余 API 请求
    } catch (err) { // 捕获网络或接口错误
      const errMsg = err instanceof Error ? err.message : String(err);
      // sidecar 启动时的连接错误静默处理
      if (!errMsg.includes('error sending request') && !errMsg.includes('Failed to fetch')) {
        messageApi.error(t('message.loadFailed', { error: errMsg })); // 全局错误提示
      }
    } finally { // 无论成功失败都执行
      setLoading(false); // 关闭加载动画
    }
  }, [sortField, sortOrder, selectedCategoryId]); // 依赖排序字段、方向、分类筛选。directOnly 不影响 SQL (后端默认仅直属), 不进 deps 避免无谓 reload。
  // 保存 loadPapers 引用供 hook 使用
  loadPapersRef.current = loadPapers;

  // 组件挂载时加载论文列表，或当筛选条件变化时重新加载
  useEffect(() => { // useEffect 副作用钩子
    loadPapers(); // 调用加载论文列表函数
  }, [loadPapers]); // 依赖 loadPapers

  // ========== SSE 进度监听钩子 ==========
  // 使用提取的 useSSEProgress hook 替代原来的内联 SSE 管理代码（约 200 行）
  // 传入的回调函数保持与原内联代码相同的行为
  const { listenToParseStatus, reconnectPendingPapers, cleanup: sseCleanup } = useSSEProgress({
    // 进度更新回调：更新论文列表中对应论文的解析进度和阶段
    onProgress: useCallback(({ paperId, percent, stage }: any) => { // useCallback 缓存回调
      setPapers((prev: any) => prev.map((p: any) => // 遍历论文列表
        p.id === paperId // 匹配目标论文
          ? { ...p, parse_status: 'parsing', parse_progress: percent, parse_stage: stage } // 更新进度和阶段
          : p // 其他论文保持不变
      ));
    }, []), // 无外部依赖
    // 解析完成回调：先设 100% 进度（400ms 过渡），再切换为 done 状态，并自动获取元数据
    onDone: useCallback(({ paperId, parseEngine, markdownLength }: any) => { // useCallback 缓存回调
      // 第一步：先将进度设为 100%，让用户看到满格进度条的"解析完成！"状态
      setPapers((prev: any) => prev.map((p: any) => // 遍历论文列表
        p.id === paperId // 找到目标论文
          ? { ...p, parse_status: 'parsing', parse_progress: 100, parse_stage: t('message.reparseDone') } // 保持 parsing 状态，设 100%
          : p // 其他论文保持不变
      ));
      // 第二步：400 毫秒后切换为最终的"已就绪"状态
      const timerId1 = setTimeout(() => { // 400ms 延迟，让用户看到满格进度条
        setPapers((prev: any) => prev.map((p: any) => // 再次遍历论文列表
          p.id === paperId // 找到目标论文
            ? { ...p, parse_status: 'done', parse_engine: parseEngine, markdown_length: markdownLength, parse_progress: undefined, parse_stage: undefined } // 切换为 done 状态
            : p // 其他论文保持不变
        ));
      }, 400); // 延迟 400 毫秒
      timeoutRefs.current.push(timerId1); // 存储 timeout ID，用于 cleanup
    }, [loadPapers]), // 依赖 loadPapers
    // 解析失败回调：更新论文状态为 failed 并保存错误信息
    onError: useCallback(({ paperId, message: errMsg }: any) => { // useCallback 缓存回调，重命名避免与 antd message 冲突
      setPapers((prev: any) => prev.map((p: any) => // 遍历论文列表
        p.id === paperId // 匹配目标论文
          ? { ...p, parse_status: 'failed', parse_error: errMsg } // 更新状态为 failed
          : p // 其他论文保持不变
      ));
    }, []), // 无外部依赖
  });

  // 初始 SSE 重连：页面加载/刷新后，为正在解析的论文自动重建 SSE 监听
  useEffect(() => { // useEffect 副作用钩子
    // 使用 hook 提供的 reconnectPendingPapers 函数，传入重连控制 ref
    reconnectPendingPapers(papers, loading, hasReconnectedRef); // 调用重连函数
  }, [papers, loading, reconnectPendingPapers]); // 依赖：论文列表、加载状态、重连函数

  // ========== PDF 文件上传钩子 ==========
  // 使用提取的 usePaperUpload hook 替代原来的内联上传代码（约 80 行）
  const { uploading, handleUpload, handleUploadToCategory } = usePaperUpload(setPapers, listenToParseStatus);

  // ========== 题录导入和去重钩子 ==========
  // 使用提取的 useBibliographyImport hook 替代原来的内联题录导入代码（约 200 行）
  const {
    importingBib,                // 题录导入中状态
    uploadingPdfIds,             // 正在上传 PDF 的论文 ID 集合
    handleImportBibliography,    // 处理题录文件导入
    handleAttachPdf,             // 为元数据记录手动上传 PDF（注意：hook 的签名是 (paperId, file, listenToParseStatus)）
  } = useBibliographyImport(loadPapers);

  // 包装题录导入 hook 中的 PDF 操作函数，自动补充 listenToParseStatus 参数
  // 原因：useBibliographyImport hook 中的 handleAttachPdf 签名是
  //   (paperId, file, listenToParseStatus)
  // 但右键菜单的 onClick 只关心 (paperId, file)，不需要了解 SSE 细节
  // 所以用 useCallback 包装一层，在内部自动注入 listenToParseStatus
  const wrappedHandleAttachPdf = useCallback((paperId: any, file: any) => { // 包装函数：隐藏 listenToParseStatus 参数
    return handleAttachPdf(paperId, file, listenToParseStatus); // 调用 hook 原函数，自动补充第三个参数
  }, [handleAttachPdf, listenToParseStatus]); // 依赖：hook 导出的上传函数（状态变化时更新）、SSE 监听函数（连接变化时更新）

  // ========== 全局文件拖拽导入钩子 ==========
  // 支持从文件管理器或 Chrome 下载气泡直接拖入文件导入（题录文件或 PDF）
  const { isDragging } = useFileDropzone({
    onImportBibliography: handleImportBibliography,
    onUploadToCategory: handleUploadToCategory,
    onRefreshCategories: useCallback(() => {
      categorySidebarRef.current?.refreshCategories();
    }, []),
  });

  // ========== 列配置钩子 ==========
  // 使用提取的 useColumnConfig hook 管理列可见性、列宽、排序、表头右键菜单
  // 排序状态现在直接从 Store 读取，hook 不再维护本地状态
  const {
    visibleColumns, // 当前可见的列 key 数组
    toggleColumnVisibility, // 切换列可见性函数（标题列强制包含 + localStorage 持久化）
    columnWidths, // 列宽配置对象 { columnKey: width }
    handleResize, // 拖拽调整列宽函数
    handleSort, // 处理排序变更函数（直接写入 Store）
    headerContextMenu, // 表头右键菜单位置 { x, y } | null
    setHeaderContextMenu, // 设置表头右键菜单位置
    // closeHeaderContextMenu, handleHeaderContextMenu: hook 内部管理关闭和打开逻辑
    tableScrollX, // 表格总宽度（用于 scroll.x）
    // ALL_COLUMNS, SORT_FIELD_MAP: hook 内部使用，外部通过 paperListConstants 导入
  } = useColumnConfig(loadPapers);

  // ========== 筛选模式状态 ==========
  // 筛选模式相关状态（selectedPaperForScreening, leftPanelView 等）
  // 这些状态保留在主组件中，因为它们需要在正常模式和筛选模式之间共享
  const [selectedPaperForScreening, setSelectedPaperForScreening] = useState<any>(null); // 选中的论文对象
  const [leftPanelView, setLeftPanelView] = useState('category'); // 左面板视图类型

  // 选中文献详情：detail API 单源，替代旧 useTranslationCache 双源（list item + translate API）
  // 修复筛选界面上下半面板数据源不对称导致的「上无摘要 / 下有中文翻译」错位
  const { detail: selectedPaperDetail, loading: detailLoading } = useSelectedPaperDetail(selectedPaperForScreening);

  // ========== 左侧面板宽度状态 ==========
  const [leftPanelWidth, setLeftPanelWidth] = useState(() => {
    const saved = localStorage.getItem(STORAGE_KEYS.LEFT_PANEL_WIDTH);
    return saved ? parseInt(saved, 10) : PANEL_CONSTRAINTS.leftPanel.default;
  });
  const [leftPanelCollapsed, setLeftPanelCollapsed] = useState(false);
  const leftPanelWidthBeforeCollapse = useRef(leftPanelWidth);

  // ========== 左侧面板水平拖拽调整宽度 ==========
  const [isHorizontalResizing, setIsHorizontalResizing] = useState(false);

  const handleHorizontalResizeStart = useCallback((e: any) => {
    e.preventDefault();
    setIsHorizontalResizing(true);
    const startX = e.clientX;
    const startWidth = leftPanelWidth;

    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    const handleMove = (moveEvent: any) => {
      const delta = moveEvent.clientX - startX;
      const newWidth = Math.min(Math.max(startWidth + delta, 180), 480);
      setLeftPanelWidth(newWidth);
    };

    const handleUp = () => {
      setIsHorizontalResizing(false);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', handleMove);
      document.removeEventListener('mouseup', handleUp);
      setLeftPanelWidth((w: any) => {
        localStorage.setItem(STORAGE_KEYS.LEFT_PANEL_WIDTH, String(w));
        return w;
      });
    };

    document.addEventListener('mousemove', handleMove);
    document.addEventListener('mouseup', handleUp);
  }, [leftPanelWidth]);

  // Ctrl+Left 切换左侧面板折叠/展开
  useKeyboardShortcut(
    (e: any) => e.ctrlKey && e.key === 'ArrowLeft',
    (e: any) => {
      e.preventDefault();
      setLeftPanelCollapsed(prev => {
        if (!prev) {
          leftPanelWidthBeforeCollapse.current = leftPanelWidth;
        }
        return !prev;
      });
    },
    [leftPanelWidth],
    { excludeInputs: false }
  );

  // 折叠状态变化时同步
  useEffect(() => {
    if (!leftPanelCollapsed) {
      setLeftPanelWidth(leftPanelWidthBeforeCollapse.current);
    }
  }, [leftPanelCollapsed]);

  // ========== 设置 PaperList 共享配置 ==========
  // 将共享回调注册到 configRef，供 hooks 使用
  (configRef.current as any).loadPapers = loadPapers;
  (configRef.current as any).onRefreshCategories = useCallback(() => { // 刷新分类树计数的回调
    if (categorySidebarRef.current) { // 确保 ref 已绑定
      categorySidebarRef.current.refreshCategories(); // 调用 CategorySidebar 暴露的刷新方法
    }
  }, []); // 空依赖，categorySidebarRef 是 ref 不会变化

  // ========== 右键菜单和分类分配钩子 ==========
  // 使用提取的 usePaperContextMenu hook 管理右键菜单和分类相关操作
  // 仅传递 hook 特定的参数，共享参数从 context 读取
  const {
    tableContextMenu, // 右键菜单状态 { paperId, x, y } | null
    paperCategoryMap, // 论文-分类映射表 { paperId: [categoryId1, categoryId2, ...] }
    categoriesCache, // 分类树数据缓存 [{ id, name, parent_id }, ...]
    setPaperCategoryMap, // 更新论文-分类映射表的 setter（用于主组件在加载论文列表时更新）
    handleContextMenu, // 处理右键菜单点击事件
    closeContextMenu, // 关闭右键菜单
    handleAssignToCategory, // 分配论文到分类
    handleUnassignFromCategory, // 从分类移除论文
    buildContextMenuItems, // 构建右键菜单项数组
    getCategoryPath, // 获取分类完整路径
  } = usePaperContextMenu({
    wrappedHandleAttachPdf, // 包装后的上传 PDF 回调
    loadAttachments, // 加载附件回调
    attachmentCacheRef, // 附件缓存 ref
    uploadingPdfIds, // 上传中 PDF 集合
  });

  // 点击页面其他区域时关闭表格行右键菜单 - 已移至 usePaperContextMenu hook

  // handleDelete 已移至 usePaperContextMenu hook
  // hook 中的 handleDelete 通过右键菜单调用，包含删除论文 + 关闭菜单 + 刷新列表逻辑
  // 此处不再需要独立的 handleDelete 函数

  /**
   * 处理分类选择事件
   *
   * 当用户在侧边栏点击某个分类时触发。
   * 更新 selectedCategoryId 状态，loadPapers 会自动重新加载（因为 selectedCategoryId 是其依赖项）。
   *
   * @param {string|number|null} categoryId - 选中的分类 ID
   *   - null：全部论文
   *   - 'uncategorized'：未分类论文
   *   - 数字：具体分类 ID
   */
  const handleCategorySelect = useCallback((categoryId: any) => { // 用 useCallback 缓存分类选择处理函数
    // 更新选中的分类 ID 状态
    setSelectedCategoryId(categoryId); // 设置选中的分类 ID
    // 常规点击分类时重置直属模式（显示包含子分类的所有论文）
    setDirectOnly(false);
    // 不需要手动调用 loadPapers，因为 loadPapers 依赖 selectedCategoryId，
    // 状态更新后 useEffect 会自动触发重新加载
  }, []); // 空依赖数组，函数引用稳定

  /**
   * 处理右键菜单"仅显示直属论文"事件
   *
   * 设置直属模式并选中该分类，直接发起 API 请求。
   * 不依赖 useEffect 自动触发，而是手动构建参数并请求，
   * 避免因 React batching 导致 state 尚未更新时就触发 loadPapers。
   *
   * @param {number} categoryId - 分类 ID
   */
  const handleDirectOnlySelect = useCallback(async (categoryId: any) => {
    setSelectedCategoryId(categoryId); // 更新选中状态（同步侧边栏高亮）
    setDirectOnly(true); // 开启直属模式（前端状态, 用于 UI 区分）

    // 直接构建参数并发起请求，不依赖 state 闭包。
    // 注：曾传 directOnly: true 给后端, 但 ListPapersQuery 没有该字段,
    // 被 serde 默默丢弃——后端默认 SQL `ic."categoryId" = $N` 本来就只查直属,
    // 所以这个菜单的语义和普通分类点击等价。删除 directOnly 参数避免
    // "前端发了后端不认的字段"这一静默降级。后续若实现"包含子孙分类"
    // 模式, 直接在 ListPapersQuery 加 direct_only: Option<bool> 并在
    // ItemRepo::list 切换 SQL 即可。
    try {
      const params = { sort: sortField, order: sortOrder, categoryId: categoryId };
      const data = await getPapers(params);
      setPapers(data.papers || []);

      // 加载分类关联数据（后端已直接返回 category_ids，无需额外查询）
      setPaperCategoryMap(buildPaperCategoryMap(data.papers));
    } catch (err) {
      messageApi.error(t('message.loadFailed', { error: (err as any).message }));
    }
  }, [sortField, sortOrder]);

  // handleAssignToCategory, handleUnassignFromCategory, getCategoryAssignMenu, buildContextMenuItems 已移至 usePaperContextMenu hook

  /**
   * 表格排序变更处理函数
   *
   * 当用户点击表格列标题排序时触发，将表格列 key 映射为后端排序字段名，
   * 更新 sortField 和 sortOrder 状态，触发 loadPapers 重新加载数据。
   *
   * @param {Object} _pagination - 分页信息（未使用）
   * @param {Object} _filters - 过滤信息（未使用）
   * @param {Object} sorter - 排序信息，包含 field（列 key）和 order（ascend/descend）
   */
  /**
   * 构建表格视图的活跃列配置
   *
   * 使用提取的 useActiveColumns hook 替代原来的内联 useMemo 代码（约 214 行）。
   * hook 根据 visibleColumns 状态过滤 ALL_COLUMNS，生成 Ant Design Table 的 columns 属性。
   *
   * 为什么标题列的 render 需要在组件内部定义？
   * - 标题列的渲染依赖 navigate（路由导航）、categoriesCache（分类数据）等组件级变量
   * - 这些变量在 ALL_COLUMNS 常量定义时还不可用
   * - 所以 ALL_COLUMNS 中标题列的 render 为 null，在此处动态覆盖
   */
  const activeColumns = useActiveColumns({
    visibleColumns,
    columnWidths,
    paperCategoryMap,
    categoriesCache,
    navigate,
    handleUnassignFromCategory,
    getCategoryPath,
    handleResize,
    sortField,
    sortOrder,
    screeningMode,
    handleExpand,
    expandedRowKeys,
    setLeftPanelView,
    setSelectedPaperForScreening,
    setSortField,
    setSortOrder,
  });

  // 表格总宽度：所有可见列宽度之和，用于 scroll.x - 已移至 useColumnConfig hook

  // 初始加载状态：显示居中的加载动画
  if (loading) { // 如果正在加载
    return ( // 返回加载中的 UI
      <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100%' }}> {/* 居中容器 */}
        <Spin size="large" /> {/* 大号加载动画 */}
      </div>
    );
  }

  // ========== 渲染论文表格（正常模式和筛选模式共用） ==========
  /**
   * 渲染论文表格内容
   *
   * 使用提取的 PaperTable 组件替代原来的内联 JSX（约 90 行）。
   * 正常模式和筛选模式共用同一个表格，区别在于：
   * - 正常模式：expandable 启用，点击标题展开附件
   * - 筛选模式：expandable 禁用，点击标题选中论文
   */
  const renderPaperTable = (containerRef = tableWrapperRef) => (
    <PaperTable
      containerRef={containerRef}
      papers={papers}
      activeColumns={activeColumns}
      tableScrollX={tableScrollX}
      screeningMode={screeningMode}
      expandedRowKeys={expandedRowKeys}
      handleExpand={handleExpand}
      attachmentCache={attachmentCache}
      attachmentLoading={attachmentLoading}
      handleDeleteAttachment={handleDeleteAttachment}
      handleOpenAttachment={handleOpenAttachment}
      handleSetPrimary={handleSetPrimary}
      TableRowWithDragAndContextMenu={TableRowWithDragAndContextMenu}
      headerContextMenu={headerContextMenu}
      setHeaderContextMenu={setHeaderContextMenu}
      visibleColumns={visibleColumns}
      toggleColumnVisibility={toggleColumnVisibility}
      selectedPaperForScreening={selectedPaperForScreening}
      handleContextMenu={handleContextMenu}
      setSelectedPaperForScreening={setSelectedPaperForScreening}
      setLeftPanelView={setLeftPanelView}
    />
  );

  // 主渲染：根据筛选模式切换 2 栏/3 栏布局
  return (
    <>
      {screeningMode ? (
        // ========== 筛选模式：3 栏布局 ==========
        // 使用独立的 ScreeningMode 组件，包含左面板、中间表格、右面板 AI 对话
        <ScreeningMode
          papers={papers} // 论文列表数据
          loadPapers={loadPapers} // 刷新论文列表回调
          selectedPaperDetail={selectedPaperDetail} // 选中文献完整详情（detail API 单源）
          detailLoading={detailLoading} // 详情+翻译轮询加载状态
          categorySidebarRef={categorySidebarRef} // 分类侧边栏 ref
          handleCategorySelect={handleCategorySelect} // 分类选择回调
          handleDirectOnlySelect={handleDirectOnlySelect} // 直属模式选择回调
          handleUploadToCategory={handleUploadToCategory} // 上传到分类回调
          handleImportBibliography={handleImportBibliography} // 导入题录回调
          renderPaperTable={renderPaperTable} // 渲染论文表格函数
          selectedPaperForScreening={selectedPaperForScreening} // 选中的论文对象
          setSelectedPaperForScreening={setSelectedPaperForScreening} // 设置选中论文回调
          leftPanelView={leftPanelView} // 左面板视图类型
          setLeftPanelView={setLeftPanelView} // 设置左面板视图回调
        />
      ) : (
        // ========== 正常模式：左侧分类树 + 右侧论文表格 ==========
        <div style={{ display: 'flex', height: '100vh' }}>
          {/* 左侧面板：分类树 */}
          {!leftPanelCollapsed && (
          <div style={{
            display: 'flex',
            flexDirection: 'column',
            width: leftPanelWidth,
            minWidth: PANEL_CONSTRAINTS.leftPanel.min,
            maxWidth: PANEL_CONSTRAINTS.leftPanel.max,
            borderRight: '1px solid var(--border-color)',
            userSelect: isHorizontalResizing ? 'none' : 'auto',
            position: 'relative',
          }}>
            <CategorySidebar
              ref={categorySidebarRef}
              selectedCategoryId={selectedCategoryId}
              onSelectCategory={handleCategorySelect}
              onPapersUpdate={loadPapers}
              onSelectDirectOnly={handleDirectOnlySelect}
              onUploadToCategory={handleUploadToCategory}
              onImportBibliography={handleImportBibliography}
              fillParent
            />

            {/* 左侧面板右边拖拽手柄 */}
            <div
              onMouseDown={handleHorizontalResizeStart}
              style={{
                position: 'absolute',
                top: 0,
                right: -3,
                bottom: 0,
                width: 6,
                cursor: 'col-resize',
                zIndex: 10,
              }}
            />
          </div>
          )}

          {/* 右侧：论文表格 */}
          <div style={{
            flex: 1,
            overflowX: 'hidden',
            overflowY: 'auto',
            display: 'flex',
            flexDirection: 'column',
          }}>
            <div ref={tableWrapperRef} style={{ flex: 1, overflow: 'auto' }}>
              {renderPaperTable()}
            </div>
          </div>
        </div>
      )}

      {/* ========== 重复文献解决对话框（通过 NiceModal 命令式调用） ========== */}

      {/* ========== 表格行右键菜单（独立定位） ========== */}
      {/* 不在 <tr> 外包裹 Dropdown，而是通过 onContextMenu 事件 + 状态驱动的独立 Dropdown */}
      {tableContextMenu && (() => {
        const paper = papers.find((p: any) => p.id === tableContextMenu.paperId);
        if (!paper) return null;
        const items = buildContextMenuItems(paper);
        return (
          <Dropdown
            menu={{ items: items as any }}
            open={true}
            onOpenChange={(open) => { if (!open) closeContextMenu(); }}
            trigger={[]}
          >
            <div
              style={{
                position: 'fixed',
                left: tableContextMenu.x,
                top: tableContextMenu.y,
                width: 1,
                height: 1,
              }}
            />
          </Dropdown>
        );
      })()}

      {/* ========== 全局文件拖拽覆盖层 ========== */}
      {/* 从文件管理器或 Chrome 下载气泡拖入文件时显示，释放后自动导入 */}
      <FileDropzoneOverlay visible={isDragging} />
    </>
  );
}

/**
 * 论文列表页主组件（带 Context Provider 和错误边界）
 *
 * 使用 FeatureErrorBoundary 包裹页面内容，当页面渲染出现错误时：
 * - 显示友好的错误提示 UI（而非整个应用白屏）
 * - 提供"重试"按钮，用户可以尝试恢复
 * - 保留其他页面（如阅读器页）的正常功能
 */
export default function PaperListPage() {
  const { t } = useTranslation('paperList');
  return (
    <PaperListProvider>
      <FeatureErrorBoundary name={t('feature.paperList')}>
        <PaperListPageInner />
      </FeatureErrorBoundary>
    </PaperListProvider>
  );
}
