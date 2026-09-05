/**
 * usePaperContextMenu - 论文右键菜单和分类分配钩子
 *
 * 本文件封装了论文列表的右键上下文菜单功能，包括：
 * - 右键菜单显示/隐藏状态管理
 * - 论文分类关联数据管理（每篇论文所属的分类列表）
 * - 分类树数据缓存（用于构建分类分配菜单）
 * - 分类路径计算（如 "机器学习/深度学习"）
 * - 分配论文到分类
 * - 从分类移除论文
 * - 构建完整的右键菜单项（包含分类分配、元数据获取、删除等）
 *
 * 菜单项类型：
 * - 分配到分类：弹出子菜单，显示所有分类，已归入的显示 ✓ 标记
 * - 获取期刊信息：有期刊名且尚无 IF/分区指标的论文显示（EasyScholar）
 * - 上传附件：仅对题录导入记录显示
 * - 获取PDF：仅对有 DOI 的题录导入记录显示
 * - 删除论文：危险操作，红色高亮
 *
 * 为什么需要独立 hook？
 * - 右键菜单逻辑复杂，包含多个状态和回调
 * - 需要在多个地方复用（表格视图、未来可能的卡片视图）
 * - 集中管理分类相关操作，便于维护
 */

import { useState, useCallback, useEffect } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // 导入 Ant Design App 组件，用于获取 modal 实例
import { useTranslation } from 'react-i18next'; // 导入 i18n 翻译钩子
import type { Paper, Category } from '@/types';
import { usePaperListConfig } from '../PaperListContext'; // 导入 PaperList 上下文
import { // 导入图标组件
  DeleteOutlined, // 删除图标
  UploadOutlined, // 上传图标（上传附件）
  FolderOutlined, // 文件夹图标（分类分配）
  FileTextOutlined, // 文件文本图标（题录导入记录标识）
  CopyOutlined, // 复制图标（复制引用）
  SearchOutlined, // 搜索图标（获取期刊信息——从 EasyScholar 查询）
} from '@ant-design/icons';
import { // 导入论文 API
  deletePaper, // 删除论文
  fetchMetadata, // 获取期刊指标（EasyScholar）
} from '../../../services/papersApi';
import { // 导入分类 API
  assignPapers, // 分配论文到分类
  unassignPapers, // 从分类移除论文
  getCategoryTree, // 获取分类树数据
} from '../../../services/categoriesApi';
import usePaperStore from '../../../stores/usePaperStore';
import { bridgePaperDeleted } from '../../../bus/sseBridge';
import { useCopyCitation } from '../../../hooks/useCopyCitation';

/**
 * 论文右键菜单和分类分配钩子
 *
 * @param {Object} params - 参数对象
 * @param {Function} params.wrappedHandleAttachPdf - 包装后的上传 PDF 回调
 * @param {Function} params.loadAttachments - 加载附件列表的回调函数
 * @param {React.MutableRefObject} params.attachmentCacheRef - 附件缓存 ref 引用
 * @param {Set} params.uploadingPdfIds - 正在上传 PDF 的论文 ID 集合
 * @returns {Object} 返回状态和处理函数的对象
 */

function isBibImport(source: string | null | undefined): boolean {
  return source === 'bib_import' || !!source?.startsWith('bibliography_import_');
}

/** 右键菜单状态 */
interface ContextMenuState {
  paperId: string;
  x: number;
  y: number;
}

/** usePaperContextMenu 参数 */
interface UsePaperContextMenuParams {
  wrappedHandleAttachPdf: (paperId: string, file: File) => Promise<void>;
  loadAttachments: (paperId: string) => Promise<void>;
  attachmentCacheRef: React.MutableRefObject<Record<string, unknown>>;
  uploadingPdfIds: Set<string>;
}

export function usePaperContextMenu({
  wrappedHandleAttachPdf,
  loadAttachments,
  attachmentCacheRef,
  uploadingPdfIds,
}: UsePaperContextMenuParams) {
  // Read shared config from context
  const configRef = usePaperListConfig();
  const { t } = useTranslation('paperList'); // i18n 翻译函数
  // 获取 Ant Design App 上下文中的 modal 和 message 实例
  // 必须使用 App.useApp() 而不是静态 Modal.confirm / message.xxx，
  // 因为静态方法在 ConfigProvider 外部渲染，无法继承深色主题
  const { modal, message } = App.useApp();
  // 复制引用（共享 hook：默认样式解析 + citeproc-js 渲染 + 剪贴板写入）
  const copyCitation = useCopyCitation();
  // 状态定义
  // ========================================

  /**
   * 表格行右键菜单状态
   *
   * 记录右键点击的位置和目标论文，用于渲染独立定位的 Dropdown 组件。
   * 不使用 Dropdown 包裹 <tr> 的方式，因为：
   * - Ant Design 的 Dropdown 内部使用 React.Children.only 验证子元素
   * - 直接包裹 <tr> 会在 Table 的自定义行组件上下文中报错
   *
   * @type {{ paperId: number, x: number, y: number } | null}
   */
  const [tableContextMenu, setTableContextMenu] = useState<ContextMenuState | null>(null); // null 表示隐藏菜单，对象表示显示位置和目标论文

  /**
   * 论文分类关联映射表
   *
   * 存储每篇论文所属的分类 ID 列表，用于：
   * - 在标题列显示分类标签
   * - 在右键菜单中显示已归入分类的 ✓ 标记
   *
   * 数据格式：{ paperId: [categoryId1, categoryId2, ...] }
   *
   * @type {Object}
   */
  const [paperCategoryMap, setPaperCategoryMap] = useState<Record<string, string[]>>({}); // 论文 ID → 分类 ID 数组的映射

  /**
   * 分类树数据缓存
   *
   * 存储从后端获取的扁平分类列表，用于：
   * - 构建分类分配菜单
   * - 计算分类完整路径（如 "机器学习/深度学习"）
   *
   * 为什么需要缓存？
   * - 避免每次打开右键菜单都请求分类接口
   * - 分类数据变化不频繁，缓存后性能更好
   *
   * @type {Array}
   */
  const [categoriesCache, setCategoriesCache] = useState<Category[]>([]); // 分类数组，每项包含 id, name, parent_id

  // ========================================
  // 辅助函数
  // ========================================

  /**
   * 根据分类 ID 构建完整路径
   *
   * 从当前分类向上追溯到根分类，拼接成完整路径字符串。
   * 例如：分类 "深度学习" 的父分类是 "机器学习"，路径为 "机器学习/深度学习"。
   *
   * @param {number} catId - 分类 ID
   * @returns {string} 完整路径，如 "机器学习/深度学习"
   */
  const getCategoryPath = useCallback((catId: string): string => {
    const parts = []; // 路径片段数组

    // 从当前分类开始，向上追溯父分类
    let current = categoriesCache.find(c => c.id === catId); // 在缓存中查找当前分类

    // while 循环向上追溯，直到没有父分类为止
    while (current) {
      // 将分类名添加到路径头部（unshift 保证从根到叶的顺序）
      parts.unshift(current.name); // 将当前分类名插入数组头部

      // 查找父分类
      current = current.parent_id // 如果有父分类 ID
        ? categoriesCache.find(c => c.id === current!.parent_id) || undefined // 在缓存中查找父分类
        : undefined; // 没有父分类，终止循环
    }

    // 用 "/" 连接所有路径片段
    return parts.join('/'); // 返回完整路径字符串
  }, [categoriesCache]); // 依赖分类缓存数据

  /**
   * 加载分类树数据
   *
   * 组件挂载时加载一次，用于构建分类分配菜单。
   * 静默失败，分类加载失败不影响论文列表显示。
   */
  const loadCategories = useCallback(async () => {
    try {
      const catData = await getCategoryTree(); // 调用后端接口获取分类树
      setCategoriesCache(catData.categories || []); // 缓存分类数据
    } catch {
      // 静默忽略错误，分类加载失败不影响主要功能
    }
  }, []); // 无外部依赖

  // 组件挂载时加载分类数据
  useEffect(() => {
    loadCategories(); // 调用加载分类函数
  }, [loadCategories]); // 依赖 loadCategories 函数引用

  // ========================================
  // 分类操作函数
  // ========================================

  /**
   * 处理论文分配到分类
   *
   * 将指定的论文分配到选定的分类。
   * 使用右键菜单触发，用户点击分类项时调用。
   *
   * @param {number} paperId - 论文 ID
   * @param {number} categoryId - 分类 ID
   */
  const handleAssignToCategory = useCallback(async (paperId: string, categoryId: string) => {
    try { // 开始 try-catch
      // 调用 API 分配论文到分类
      await assignPapers([paperId], [categoryId]); // 异步调用分配接口

      // 显示成功提示
      message.success(t('message.addedToCategory')); // 弹出成功提示

      // 重新加载论文列表以更新分类标签显示
      configRef.current.loadPapers?.(); // 刷新论文列表
    } catch (err) { // 捕获分配错误
      // 显示错误提示
      message.error(t('message.assignCategoryFailed', { error: (err as any).message })); // 弹出错误提示
    }
  }, []); // Refs don't need to be in dependencies

  /**
   * 处理从分类中移除论文
   *
   * 将论文从指定分类中移除。
   * 通过分类标签上的关闭按钮触发。
   *
   * @param {Object} e - 事件对象
   * @param {number} paperId - 论文 ID
   * @param {number} categoryId - 分类 ID
   */
  const handleUnassignFromCategory = useCallback(async (e: { stopPropagation: () => void }, paperId: string, categoryId: string) => {
    // 阻止事件冒泡，避免触发论文列表项的点击事件
    e.stopPropagation(); // 阻止冒泡

    try { // 开始 try-catch
      // 调用 API 从分类移除论文
      await unassignPapers([paperId], [categoryId]); // 异步调用取消分配接口

      // 显示成功提示
      message.success(t('message.removedFromCategory')); // 弹出成功提示

      // 重新加载论文列表以更新分类标签显示
      configRef.current.loadPapers?.(); // 刷新论文列表
    } catch (err) { // 捕获移除错误
      // 显示错误提示
      message.error(t('message.removeCategoryFailed', { error: (err as any).message })); // 弹出错误提示
    }
  }, []); // Refs don't need to be in dependencies

  /**
   * 删除论文（乐观更新）
   *
   * 立即从列表中移除论文，然后调用 API。
   * 如果 API 失败，将论文添加回列表。
   *
   * @param {number} paperId - 要删除的论文 ID
   */
  const handleDelete = useCallback(async (paperId: string) => {
    // Save paper for rollback
    const store = usePaperStore.getState();
    const paper = store.papers.find(p => p.id === paperId);

    // Optimistic update: remove from list immediately
    store.setPapers(prev => prev.filter(p => p.id !== paperId));

    // Close context menu
    setTableContextMenu(null);

    try {
      // Call API to delete on server
      await deletePaper(paperId);

      // 通知其他模块：论文已删除
      bridgePaperDeleted(paperId);

      message.success(t('message.deleteSuccess'));

      // Refresh category counts
      configRef.current.onRefreshCategories?.();
    } catch (err) {
      // Rollback: add paper back to list
      if (paper) {
        store.setPapers(prev => [...prev, paper]);
      }

      message.error(t('message.deleteFailed', { error: (err as any).message }));
    }
  }, [configRef]);

  /**
   * 手动获取某篇论文的期刊指标（右键菜单「获取期刊信息」）
   *
   * 后端 fetch-metrics 是阻塞式端点：返回时 EasyScholar 结果已写入
   * journal_metrics 期刊级缓存（同一期刊只查一次），因此不需要 jayread
   * 时代的 3 秒轮询 —— 按命中与否提示，然后刷新列表（指标在组装层
   * 注入，刷新即见；命中时整刊论文同时补齐）。
   */
  const handleFetchMetadata = useCallback(async (paperId: string) => {
    message.info(t('message.fetchJournalInfo')); // 端点要等一次 EasyScholar 往返，先给即时反馈
    try {
      const res = await fetchMetadata(paperId);
      if (res.fetched) {
        message.success(t('message.fetchJournalDone', { journal: res.journal ?? '' }));
      } else {
        message.warning(t('message.fetchJournalMiss'));
      }
      configRef.current.loadPapers?.();
    } catch (err) {
      message.error(t('message.fetchJournalFailed', { error: (err as any).message }));
    }
  }, [configRef, message, t]);

  // ========================================
  // 菜单构建函数
  // ========================================

  /**
   * 构建论文的右键上下文菜单项
   *
   * 菜单项根据论文状态动态生成：
   * - 分配到分类子菜单：始终显示，包含所有分类，已归入的显示 ✓ 标记
   * - 获取期刊信息：有期刊名且尚无 IF/分区指标的论文显示
   * - 上传附件：仅对题录导入记录显示
   * - 获取PDF：仅对有 DOI 的题录导入记录显示
   * - 分割线
   * - 删除论文：危险操作，红色高亮
   *
   * @param {Object} paper - 论文对象，包含 id、parse_status、source 等字段
   * @returns {Array} Ant Design Menu 的 items 数组
   */
  const buildContextMenuItems = useCallback((paper: Paper) => {
    // 获取该论文当前已归入的所有分类 ID 列表
    // 用于在子菜单中为已归入的分类显示 ✓ 标记
    const currentCats = paperCategoryMap[paper.id] || []; // 从映射表获取，不存在则默认空数组

    // 构建分类子菜单项列表
    // 三元运算符处理两种情况：
    // - 分类缓存为空时：显示一条禁用的提示信息
    // - 分类缓存非空时：为每个分类生成一个可点击的菜单项
    const categorySubMenuItems = categoriesCache.length === 0 // 检查分类缓存是否为空
      ? [ // 空分类时的提示菜单项（禁用，不可点击）
          { key: 'empty', label: t('menu.noCategories'), disabled: true } // disabled 使菜单项变灰且不可点击
        ]
      : categoriesCache.map(cat => ({ // 遍历分类缓存，为每个分类创建一个菜单项
          key: `cat-${cat.id}`, // 菜单项唯一标识，使用 "cat-" 前缀避免 key 冲突
          icon: <FolderOutlined />, // 文件夹图标
          // 标签内容：分类完整路径 + 已归入的勾选标记
          label: (
            <span>
              {getCategoryPath(cat.id)} {/* 分类完整路径 */}
              {/* 如果论文已归入该分类，显示主题色 ✓ 标记 */}
              {currentCats.includes(cat.id) && ( // 检查是否已归入
                <span style={{ color: 'var(--ai-accent)', marginLeft: 4 }}>✓</span> // 主题色对勾
              )}
            </span>
          ),
          // 点击分类菜单项的回调：切换归入/移出状态
          onClick: () => {
            if (currentCats.includes(cat.id)) { // 如果已归入
              // 从分类中移除论文
              handleUnassignFromCategory({ stopPropagation: () => {} }, paper.id, cat.id); // 调用移除函数
            } else { // 如果未归入
              // 将论文分配到该分类
              handleAssignToCategory(paper.id, cat.id); // 调用分配函数
            }
          },
        }));

    // 返回完整的右键菜单项数组
    return [
      // 第一项：分配到分类子菜单
      {
        key: 'assign-category', // 菜单项唯一标识
        label: t('menu.assignToCategory'), // 菜单项显示文本
        icon: <FolderOutlined />, // 文件夹图标
        children: categorySubMenuItems, // 子菜单项数组
      },
      // 获取期刊信息：有期刊名但 IF/分区尚为空时显示。
      // 指标是期刊级缓存：查一次 EasyScholar，刷新后整刊论文同时补齐。
      !!paper.journal_name && paper.impact_factor == null && {
        key: 'fetch-metadata', // 菜单项唯一标识
        label: t('menu.fetchMetadata'), // 菜单项显示文本
        icon: <SearchOutlined />, // 搜索图标，暗示从外部数据源查询
        onClick: () => handleFetchMetadata(paper.id), // 查 EasyScholar 并刷新列表
      },
      // 上传附件：仅对题录导入记录显示
      isBibImport(paper.item_source) && {
        key: 'upload-pdf', // 菜单项唯一标识
        label: t('menu.uploadAttachment'), // 菜单项显示文本
        icon: <UploadOutlined />, // 上传图标
        disabled: uploadingPdfIds.has(paper.id), // 正在上传时禁用此菜单项，防止重复上传
        onClick: () => {
          // 动态创建隐藏的 <input type="file"> 元素触发浏览器文件选择对话框
          // 不使用静态 <input> 是因为此菜单项在右键菜单内，无法包含可见的表单元素
          const input = document.createElement('input'); // 创建 input DOM 元素
          input.type = 'file'; // 设置为文件选择类型
          input.onchange = async (ev) => { // 文件选择完成后的异步回调
            const file = (ev.target as HTMLInputElement).files?.[0]; // 获取用户选择的第一个文件（可选链防止空选）
            if (file) { // 确认文件存在
              await wrappedHandleAttachPdf(paper.id, file); // 调用包装后的上传函数，传入论文 ID 和文件对象
              // 上传成功后清除该论文的附件缓存，确保下次展开时重新加载最新附件列表
              // 如果不清除，曾经点击过展开（缓存为空数组 []）后上传附件，再次展开时会使用缓存的空数组而显示"暂无附件"
              delete attachmentCacheRef.current[paper.id]; // 删除 ref 中的缓存
              await loadAttachments(paper.id); // 重新加载附件
            }
          };
          input.click(); // 编程式触发文件选择对话框弹出
        },
      },
      // 复制引用（CSL 样式渲染）
      {
        key: 'copy-citation',
        label: t('menu.copyCitation', { defaultValue: '复制引用' }),
        icon: <CopyOutlined />,
        onClick: () => {
          copyCitation(paper.id);
        },
      },
      // 分割线
      { type: 'divider' }, // Ant Design Menu 分割线类型
      // 删除论文（危险操作）
      {
        key: 'delete', // 菜单项唯一标识
        label: t('menu.deletePaper'), // 菜单项显示文本
        icon: <DeleteOutlined />, // 删除图标
        danger: true, // 红色高亮，提示危险操作
        onClick: () => { // 点击回调
          modal.confirm({ // 弹出确认对话框
            title: t('menu.confirmDeletePaper'), // 确认提示
            okText: t('common:confirm'), // 确认按钮文字
            cancelText: t('common:cancel'), // 取消按钮文字
            okButtonProps: { danger: true }, // 确认按钮红色
            onOk: () => handleDelete(paper.id), // 确认后调用删除函数
          });
        },
      },
    ];
  }, [
    paperCategoryMap, // 分类映射数据，变化时需要重新计算已归入分类的 ✓ 标记
    categoriesCache, // 分类缓存数据，变化时需要重新生成子菜单
    getCategoryPath, // 获取分类路径函数，变化时需要重新计算路径文本
    handleAssignToCategory, // 分配函数，变化时需要重新绑定
    handleUnassignFromCategory, // 移除函数，变化时需要重新绑定
    handleDelete, // 删除函数，变化时需要重新绑定
    handleFetchMetadata, // 获取期刊指标函数，变化时需要重新绑定
    uploadingPdfIds, // 上传中集合，变化时需要更新禁用状态
    wrappedHandleAttachPdf, // 上传函数，变化时需要重新绑定
    loadAttachments, // 加载附件函数，变化时需要重新绑定
    modal, // modal 实例，变化时需要重新绑定
    message, // message 实例，变化时需要重新绑定
    copyCitation, // 复制引用函数，变化时需要重新绑定
    t, // i18n 翻译函数
  ]); // 依赖项：所有菜单项构建所需的状态和函数

  // ========================================
  // 右键菜单处理函数
  // ========================================

  /**
   * 关闭右键菜单
   *
   * 点击页面其他区域或右键其他位置时调用。
   */
  const closeContextMenu = useCallback(() => {
    setTableContextMenu(null); // 隐藏菜单
  }, []); // 无外部依赖

  /**
   * 处理右键菜单点击事件
   *
   * 在表格行上右键时触发，记录鼠标位置和目标论文 ID。
   *
   * @param {Object} e - 右键事件对象
   * @param {number} paperId - 右键点击的论文 ID
   */
  const handleContextMenu = useCallback((e: React.MouseEvent, paperId: string) => {
    e.preventDefault(); // 阻止浏览器默认右键菜单
    e.stopPropagation(); // 阻止事件冒泡
    // 设置右键菜单状态：记录目标论文 ID 和鼠标位置
    setTableContextMenu({ paperId, x: e.clientX, y: e.clientY }); // 保存位置信息供 Dropdown 定位
  }, []); // 无外部依赖

  // ========================================
  // 副作用：点击外部关闭菜单
  // ========================================

  /**
   * 点击页面其他区域时关闭表格行右键菜单
   *
   * 监听全局点击和右键事件，当点击不在 Dropdown 内部时关闭菜单。
   */
  useEffect(() => {
    // 如果菜单未显示，跳过
    if (!tableContextMenu) return; // 无右键菜单时跳过

    // 关闭回调
    const close = (e: MouseEvent) => { // 事件处理函数
      if ((e.target as HTMLElement)?.closest('.ant-dropdown')) return; // 点击 Dropdown 内部不关闭
      setTableContextMenu(null); // 关闭右键菜单
    };

    // 注册事件监听器
    document.addEventListener('click', close); // 注册点击事件
    document.addEventListener('contextmenu', close); // 注册右键事件

    // 清理函数：移除事件监听器
    return () => { // 组件卸载或 tableContextMenu 变化时执行
      document.removeEventListener('click', close); // 移除点击事件
      document.removeEventListener('contextmenu', close); // 移除右键事件
    };
  }, [tableContextMenu]); // 依赖右键菜单状态

  // ========================================
  // 导出公共接口
  // ========================================

  return {
    // 状态
    tableContextMenu, // 右键菜单状态 { paperId, x, y } | null
    paperCategoryMap, // 论文-分类映射表 { paperId: [categoryId1, categoryId2, ...] }
    categoriesCache, // 分类树数据缓存 [{ id, name, parent_id }, ...]
    setPaperCategoryMap, // 更新论文-分类映射表的 setter（用于主组件在加载论文列表时更新）

    // 操作函数
    handleContextMenu, // 处理右键菜单点击事件
    closeContextMenu, // 关闭右键菜单
    buildContextMenuItems, // 构建右键菜单项数组
    handleAssignToCategory, // 分配论文到分类
    handleUnassignFromCategory, // 从分类移除论文
    getCategoryPath, // 获取分类完整路径
  };
}

/**
 * 导出类型定义（用于 TypeScript 项目）
 * 本项目使用 JavaScript，保留类型注释供参考
 */
/**
 * @typedef {Object} ContextMenuState
 * @property {number} paperId - 右键点击的论文 ID
 * @property {number} x - 鼠标 X 坐标
 * @property {number} y - 鼠标 Y 坐标
 */
