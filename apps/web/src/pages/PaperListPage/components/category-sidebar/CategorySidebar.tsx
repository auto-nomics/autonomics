/**
 * JayRead 分类侧边栏组件
 *
 * 左侧树形目录侧边栏，用于分门别类管理已导入的文献。
 * 类似 Zotero/Mendeley 的 Collections 面板。
 *
 * 功能：
 * 1. 树形分类目录：支持多级嵌套，带文件夹图标和展开/折叠箭头
 * 2. 虚拟分类："全部论文"和"未分类"作为内置的虚拟分类
 * 3. 右键菜单：新建子分类、重命名、删除
 * 4. 顶部"+"按钮：创建根分类
 * 5. 点击分类：触发 onSelectCategory 回调，通知父组件过滤论文列表
 * 6. 拖拽排序：通过拖拽分类节点调整排序位置或改变层级关系
 * 7. 外部拖放：将桌面文件或论文列表中的论文拖到分类节点上（上传/分配）
 *
 * 【拖拽架构说明】
 * 本组件存在两套独立的拖拽系统，服务于不同的使用场景：
 *
 * 系统 A：rc-tree 内置的分类重排序拖拽
 *   - 由 Ant Design Tree 的 draggable 属性启用
 *   - 用于在同一棵树内调整分类的层级关系和排序位置
 *   - 拖拽源和放置目标都是树节点，由 rc-tree 内部处理
 *   - 事件链：TreeNode.onDragStart → Tree.onNodeDragOver → Tree.onNodeDrop
 *   - 触发 handleDrop（本组件），计算新 parent_id 和 sort_order 并调用 API
 *
 * 系统 B：外部文件/论文的拖放高亮与放置（useEffect 实现）
 *   - 拖拽源：桌面文件管理器中的 PDF 文件，或论文列表表格中的论文行
 *   - 放置目标：分类侧边栏中的树节点
 *   - 事件链：原生 DOM capture 阶段 dragover → 高亮目标节点 → drop → 处理放置
 *   - 为什么用原生 DOM capture 而不是 React 事件？
 *     → rc-tree 的 onNodeDragOver 第一行检查 if (!this.dragNodeProps) { return; }
 *     → 外部拖拽时 dragNodeProps 为 null，rc-tree 直接 return 不处理
 *     → 且 rc-tree 在每个树节点上调用 e.stopPropagation()，阻止事件冒泡
 *     → 原生 capture 监听器先于 rc-tree 执行，可靠拦截外部拖拽事件
 *
 * 设计说明：
 * - 分类树数据从后端获取扁平列表，前端递归构建树形结构
 * - 使用 Ant Design Tree 组件渲染树形目录
 * - 使用 Ant Design Dropdown + Menu 实现右键菜单
 * - 内联编辑（Input）用于新建和重命名操作
 * - 外部拖放高亮采用直接 DOM 操作（classList），不经过 React state，性能最优
 */
import { useState, useRef, useEffect, forwardRef, useMemo, useCallback } from 'react';
import { Tree, Input, App } from 'antd';
import { useTranslation } from 'react-i18next';
import { deleteCategory, moveCategory } from '../../../../services/categoriesApi';
import { useCategoryTree, useCategoryTreeRef } from './hooks/useCategoryTree';
import { useResizableSidebar } from './hooks/useResizableSidebar';
import { useInlineEditing } from './hooks/useInlineEditing';
import { useCategoryDragDrop } from './hooks/useCategoryDragDrop';
import CategoryTreeNode, { buildVirtualContextMenu, buildContextMenu } from './CategoryTreeNode';
import { VIRTUAL_NODE_COUNT, FILE_INPUT_ACCEPT, BIBLIOGRAPHY_EXTENSIONS } from './constants';


/**
 * （已移除）原 aggregatePaperCount 函数
 *
 * 论文数量聚合（含后代分类去重）已移至后端递归 CTE 查询中完成。
 * 后端返回的 paper_count 已包含该分类及其所有后代分类的去重论文总数，
 * 前端无需再自行累加，避免了同一篇论文被多个子分类重复计数的问题。
 */

/**
 * （已移除）原 buildTreeData 函数
 *
 * 扁平列表→树形结构转换逻辑已移至 useCategoryTree hook 中封装。
 * hook 内部的 buildTreeData 与此函数功能完全相同，但接受 renderCategoryTitle 作为参数。
 */


/**
 * 判断是否允许拖拽放置（纯函数，无闭包依赖，提取到组件外部避免每次渲染重建）
 *
 * @see CategorySidebar 内的 allowDrop 文档注释了解详细规则
 */
const allowDrop = ({ dragNode, dropNode, dropPosition }: any) => {
  if (dropNode.key === 'all' || dropNode.key === 'uncategorized') {
    if (dropPosition === 0) return false;
  }
  if (dragNode.key === dropNode.key) return false;
  return true;
};

/**
 * 拖拽配置（静态对象，无闭包依赖，提取到组件外部避免每次渲染重建）
 */
const draggableConfig = {
  icon: false,
  nodeDraggable: (node: any) => node.key !== 'all' && node.key !== 'uncategorized',
};

/**
 * 渲染分类标题（名称 + 论文数量角标）
 *
 * 显示格式："分类名称 (3)" —— 括号内为该分类下的论文数量
 * 如果论文数量为 0，则不显示括号部分，保持界面简洁
 *
 * @param {string} name - 分类名称
 * @param {number} count - 论文数量
 * @returns {JSX.Element} 渲染的分类标题元素
 */
function renderCategoryTitle(name: any, count: any) {
  // 使用 flex 布局：名称靠左，数量靠右
  return (
    <span style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', paddingRight: 4 }}>
      {/* 分类名称部分 */}
      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', flex: 1 }}>
        {name} {/* 显示分类名称 */}
      </span>
      {/* 论文数量角标：仅在数量大于 0 时显示 */}
      {count > 0 && (
        <span style={{
          fontSize: 11, // 小字号
          color: 'var(--text-tertiary)', // 灰色辅助文字颜色
          marginLeft: 6, // 与名称的间距
          flexShrink: 0, // 防止被压缩
        }}>
          {count} {/* 显示论文数量 */}
        </span>
      )}
    </span>
  );
}


/**
 * CategorySidebar 组件
 *
 * Props：
 * @param {string|null} selectedCategoryId - 当前选中的分类 ID，null 表示"全部论文"
 * @param {Function} onSelectCategory - 分类选择回调，参数为分类 ID（'all'、'uncategorized'、或数字 ID）
 * @param {Function} onPapersUpdate - 论文列表更新回调，在分类分配变更后触发刷新
 * @param {Function} onSelectDirectOnly - 右键菜单"仅显示直属论文"回调，参数为分类 ID
 * @param {Function} onUploadToCategory - 右键菜单"导入文献"回调（PDF），参数为 (file, categoryId)，由父组件处理上传逻辑
 * @param {Function} onImportBibliography - 右键菜单"导入文献"回调（题录），参数为 (file, categoryId)，由父组件处理题录导入逻辑
 * @param {number} [externalMaxWidth] - 外部约束的最大宽度（可选），筛选模式下由左面板宽度决定
 *   传入时，侧边栏的拖拽最大宽度会被限制为此值与默认 maxWidth(480) 的较小值
 * @param {boolean} [fillParent] - 是否填满父容器宽度（可选），筛选模式下由左面板使用
 *   传入时，侧边栏宽度设为 100%，不显示右边框和拖拽手柄，由外部面板控制宽度和边框
 */
const CategorySidebar = forwardRef<any, any>(function CategorySidebar({ selectedCategoryId, onSelectCategory, onPapersUpdate, onSelectDirectOnly, onUploadToCategory, onImportBibliography, externalMaxWidth, fillParent }, ref) {
  // 获取 Ant Design App 上下文中的 modal 和 message 实例（支持深色主题）
  const { modal, message } = App.useApp();
  const { t } = useTranslation('paperList');
  // ========== 状态管理 ==========

  // --- 使用 useResizableSidebar hook 管理侧边栏拖拽宽度 ---
  // 替换原有的 sidebarWidth/isResizing/startXRef/startWidthRef/handleResizeStart
  // 传入 externalMaxWidth 参数，筛选模式下限制侧边栏最大宽度
  const { sidebarWidth, handleResizeStart, isResizing } = useResizableSidebar({ externalMaxWidth }); // 调用拖拽宽度 hook，传入外部宽度约束

  // --- 使用 useCategoryTree hook 管理分类树数据加载 ---
  // 替换原有的 categories/totalCount/uncategorizedCount/loading/expandedKeys/loadCategories/treeData
  const {
    categories,     // 所有分类的扁平列表
    totalCount,     // 论文总数
    uncategorizedCount, // 未分类论文数
    loading,        // 加载状态
    expandedKeys,   // 展开的节点 key 数组
    setExpandedKeys, // 设置展开节点的方法
    loadCategories, // 手动加载分类数据的函数
    treeData,       // 完整的树形数据（包含虚拟节点）
  } = useCategoryTree({ renderCategoryTitle }); // 调用分类树数据加载 hook

  // --- 使用 useInlineEditing hook 管理内联编辑（新建/重命名）---
  // 替换原有的 editingNode/inputRef/handleCreate/handleRename/handleInputKeyDown/handleInputBlur/renderEnhancedTitle/renderRootCreateInput
  const {
    editingNode,                    // 当前正在编辑的节点状态
    inputRef,                       // 输入框的引用（自动聚焦用）
    handleInputKeyDown,             // 输入框按键事件处理（Enter 确认，Escape 取消）
    handleInputBlur,                // 输入框失焦事件处理（自动提交）
    createEnhancedTitleRenderer,    // 创建增强标题渲染器的函数
    renderRootCreateInput,          // 渲染根级别创建输入框的函数
    setEditingNode,                 // 直接设置编辑节点的方法（右键菜单用）
  } = useInlineEditing({ onEditComplete: loadCategories }); // 编辑完成后刷新分类树

  // --- 使用 useCategoryTreeRef hook 暴露刷新方法给父组件 ---
  useCategoryTreeRef(ref, loadCategories);

  // 右键菜单状态：记录当前右键点击的分类信息（本组件仍需管理）
  const [contextMenuInfo, setContextMenuInfo] = useState(null);

  // ========== 右键菜单"导入文献"功能的引用 ==========
  const importFileInputRef = useRef(null);
  const importTargetCategoryIdRef = useRef(null);

  // ========== 拖拽分类（论文/文件拖入分类）==========
  // 使用 useCategoryDragDrop hook 处理外部文件/论文拖拽
  const { treeWrapperRef } = useCategoryDragDrop({
    onUploadToCategory,
    onImportBibliography,
    loadCategories,
    onPapersUpdate,
    message,
  });


  // （已移除）拖拽调整宽度逻辑 → 已由 useResizableSidebar hook 封装
  // （已移除）数据加载逻辑 → 已由 useCategoryTree hook 封装
  // （已移除）useImperativeHandle → 已由 useCategoryTreeRef hook 封装
  // （已移除）组件挂载时加载分类数据 → 已由 useCategoryTree hook 内部 useEffect 处理
  // （已移除）自动聚焦输入框 → 已由 useInlineEditing hook 内部 useEffect 处理


  // ========== 分类操作 ==========

  /**
   * 处理创建分类（确认提交）→ 已由 useInlineEditing hook 的 handleCreate 封装
   * 本组件不再直接定义此函数，右键菜单通过 setEditingNode 触发编辑状态即可。
   */


  /**
   * 处理重命名分类（确认提交）→ 已由 useInlineEditing hook 的 handleRename 封装
   * 本组件不再直接定义此函数。
   */


  /**
   * 处理删除分类
   *
   * 弹出确认对话框，确认后调用 API 删除分类。
   * 删除操作会级联删除子分类和论文关联（数据库 CASCADE）。
   *
   * @param {object} category - 要删除的分类对象 { id, name, ... }
   */
  const handleDelete = (category: any) => { // 删除分类处理函数
    // 弹出确认对话框，防止误删
    modal.confirm({ // 确认对话框
      title: t('menu.confirmDeleteCategory'), // 对话框标题
      // 确认内容：显示分类名称，提醒用户删除的后果
      content: t('menu.confirmDeleteCategoryContent', { name: category.name }), // 确认提示文字
      okText: t('common:delete'), // 确认按钮文字
      okType: 'danger', // 确认按钮类型为危险（红色）
      cancelText: t('common:cancel'), // 取消按钮文字
      // 用户点击确认后执行的回调
      onOk: async () => { // 异步确认回调
        try { // 开始 try-catch
          // 调用 API 删除分类
          await deleteCategory(category.id); // 异步调用删除接口
          // 显示成功提示
          message.success(t('message.categoryDeleted')); // 弹出成功提示
          // 重新加载分类树数据
          await loadCategories(); // 异步刷新分类列表
          // 如果删除的是当前选中的分类，切换到"全部论文"
          if (String(selectedCategoryId) === String(category.id)) { // 检查是否删除了当前选中分类
            onSelectCategory(null); // 切换到"全部论文"视图
          }
          // 通知父组件刷新论文列表（分类关联变更可能影响筛选结果）
          if (onPapersUpdate) onPapersUpdate(); // 调用父组件的刷新回调
        } catch (err) { // 捕获删除错误
          // 显示错误提示
          message.error(t('message.deleteCategoryFailed', { error: (err as any).message })); // 弹出错误提示
        }
      },
    });
  };


  // （已移除）handleInputKeyDown/handleInputBlur → 已由 useInlineEditing hook 封装
  // （已移除）treeData 构建 → 已由 useCategoryTree hook 封装（含虚拟节点）


  // ========== 树形数据（已由 hook 提供）==========
  // treeData 由 useCategoryTree hook 返回，包含虚拟节点 + 用户分类树


  // ========== 右键菜单构建 ==========

  /**
   * 构建右键菜单项
   *
   * 根据右键点击的分类节点，生成对应的操作菜单。
   * 包含三个操作：新建子分类、重命名、删除。
   *
   * @param {object} categoryData - 被右键点击的分类数据
   * @returns {object} Ant Design Menu 菜单配置
   */
  /**
   * 构建虚拟节点的右键菜单
   *
   * 使用 CategoryTreeNode 组件提供的 buildVirtualContextMenu 辅助函数构建菜单。
   */
  const getVirtualContextMenu = () => buildVirtualContextMenu({
    setContextMenuInfo,            // 关闭右键菜单的回调函数
    setEditingNode,                // 设置编辑节点状态的函数（新建分类时使用）
    importFileInputRef,            // 统一的隐藏文件输入框 ref（弹出文件选择对话框）
    importTargetCategoryIdRef,     // 导入目标分类 ID 的 ref（虚拟节点场景下设为 null）
  });

  /**
   * 构建用户分类节点的右键菜单
   *
   * 使用 CategoryTreeNode 组件提供的 buildContextMenu 辅助函数构建菜单。
   *
   * @param {object} categoryData - 被右键点击的分类数据
   * @returns {object} Ant Design Menu 菜单配置
   */
  const getContextMenu = (categoryData: any) => buildContextMenu({
    categoryData,                  // 被右键点击的分类数据对象（包含 id, name 等字段）
    setEditingNode,                // 设置编辑节点状态的函数（新建/重命名时使用）
    setContextMenuInfo,            // 关闭右键菜单的回调函数
    setExpandedKeys,               // 设置展开节点的函数（新建子分类时自动展开父节点）
    onSelectDirectOnly,            // "仅显示直属论文"回调（过滤掉后代分类的论文）
    handleDelete,                  // 删除分类的处理函数（含确认对话框）
    expandedKeys,                  // 当前已展开的节点 key 数组
    importFileInputRef,            // 统一的隐藏文件输入框 ref（弹出文件选择对话框）
    importTargetCategoryIdRef,     // 导入目标分类 ID 的 ref（记录当前分类 ID）
  });


  // ========== 渲染树节点标题（支持内联编辑）==========

  // 使用 useInlineEditing hook 的 createEnhancedTitleRenderer 创建增强标题渲染器
  // 替换原有的 renderEnhancedTitle 内联函数实现
  const renderEnhancedTitle = createEnhancedTitleRenderer(renderCategoryTitle, Input); // 传入渲染函数和 Input 组件，返回增强渲染器


  /**
   * 增强树形数据：将 CategoryTreeNode 组件注入到树节点中
   *
   * 遍历树形数据，将每个节点的 title 替换为 CategoryTreeNode 组件。
   * CategoryTreeNode 封装了右键菜单、拖放、内联编辑等功能。
   *
   * @param {Array} nodes - 原始树形数据
   * @returns {Array} 增强后的树形数据
   */
  const enhancedTreeData = useMemo(() => {
    const enhance = (nodes: any) => {
      return nodes.map((node: any) => {
        return {
          ...node,
          title: (
            <CategoryTreeNode
              node={node}
              renderEnhancedTitle={renderEnhancedTitle}
              getContextMenu={getContextMenu}
              getVirtualContextMenu={getVirtualContextMenu}
              loadCategories={loadCategories}
              setExpandedKeys={setExpandedKeys}
              expandedKeys={expandedKeys}
              setEditingNode={setEditingNode}
              onSelectDirectOnly={onSelectDirectOnly}
              handleDelete={handleDelete}
              onPapersUpdate={onPapersUpdate}
              message={message}
            />
          ),
          children: node.children ? enhance(node.children) : [],
        };
      });
    };
    return enhance(treeData);
  }, [treeData, editingNode, renderEnhancedTitle, getContextMenu, getVirtualContextMenu, loadCategories, setExpandedKeys, expandedKeys, setEditingNode, onSelectDirectOnly, handleDelete, onPapersUpdate, message]);


  // ========== 辅助函数 ==========

  /**
   * 递归收集指定节点的所有后代节点 key
   *
   * 收起节点时需要同时移除所有后代的 key，
   * 确保下次展开时只显示一级子文件夹。
   *
   * @param {Array} nodes - 树形数据数组
   * @param {string} targetKey - 目标节点的 key
   * @returns {string[]|null} 所有后代节点的 key 数组，未找到目标则返回 null
   */
  const getAllDescendantKeys = (nodes: any, targetKey: any): string[] | null => {
    for (const node of nodes) {
      if (node.key === targetKey) {
        const keys: string[] = [];
        const collect = (children: any) => {
          for (const child of children) {
            keys.push(child.key);
            if (child.children?.length) collect(child.children);
          }
        };
        if (node.children?.length) collect(node.children);
        return keys;
      }
      if (node.children) {
        const result: string[] | null = getAllDescendantKeys(node.children, targetKey);
        if (result) return result;
      }
    }
    return null;
  };


  // ========== 事件处理 ==========

  /**
   * 处理树节点选中事件
   *
   * 用户点击分类节点时触发。同时执行两个操作：
   * 1. 展开/收起：展开时只展开一级，收起时同时收起所有后代
   * 2. 选中：调用 onSelectCategory 回调，传递选中的分类 ID
   *
   * @param {Array} selectedKeys - 选中的节点 key 数组（单选模式只有一个元素）
   * @param {object} info - 选中事件附加信息 { node, selected, ... }
   */
  const handleSelect = (selectedKeys: any, info: any) => { // 树节点选中处理函数
    // ===== 展开/收起一级 =====
    const clickedKey = info?.node?.key;
    if (clickedKey && clickedKey !== 'all' && clickedKey !== 'uncategorized') {
      if (expandedKeys.includes(clickedKey)) {
        // 当前已展开 → 收起该节点及所有后代
        const descendantKeys = getAllDescendantKeys(treeData, clickedKey);
        const keysToRemove = new Set([clickedKey, ...(descendantKeys || [])]);
        setExpandedKeys(expandedKeys.filter(k => !keysToRemove.has(k)));
      } else {
        // 当前已收起 → 仅展开该节点（显示一级子文件夹）
        setExpandedKeys([...expandedKeys, clickedKey]);
      }
    }

    // ===== 选中逻辑（保持原有行为） =====
    // 如果没有选中任何节点（用户点击了已选中的节点，触发了取消选中）
    if (selectedKeys.length === 0) {
      // 重新调用 onSelectCategory 传入当前分类 ID，这会重置父组件的 directOnly 状态
      // 如果当前是直属模式，左键点击同一分类应该回到普通模式（显示包含子分类的所有论文）
      if (selectedCategoryId !== null && selectedCategoryId !== 'uncategorized') {
        onSelectCategory(selectedCategoryId); // 重新选中同一分类，触发 directOnly 重置
      }
      return;
    }

    // 获取选中的节点 key
    const key = selectedKeys[0]; // 取第一个（也是唯一一个）选中的 key

    // 根据虚拟分类的 key 转换为对应的回调参数
    if (key === 'all') { // 如果选中的是"全部论文"
      onSelectCategory(null); // 传递 null 表示显示全部论文
    } else if (key === 'uncategorized') { // 如果选中的是"未分类"
      onSelectCategory('uncategorized'); // 传递 'uncategorized' 字符串标识
    } else { // 选中的是用户自定义分类
      onSelectCategory(key); // UUID 字符串
    }
  };


  /**
   * 处理树节点展开/折叠事件
   *
   * 收起时同时移除所有后代的 key，确保下次展开只显示一级。
   *
   * @param {Array} keys - Tree 组件返回的当前展开节点 key 数组
   * @param {object} info - 展开事件附加信息 { node, expanded, ... }
   */
  const handleExpand = (keys: any, info: any) => { // 展开/折叠处理函数
    if (!info.expanded) {
      // 收起时：额外移除所有后代的 key（Tree 只移除了被点击节点的 key）
      const descendantKeys = getAllDescendantKeys(treeData, info.node.key);
      if (descendantKeys?.length) {
        const keysToRemove = new Set(descendantKeys);
        setExpandedKeys(keys.filter((k: any) => !keysToRemove.has(k)));
        return;
      }
    }
    setExpandedKeys(keys);
  };


  /**
   * 处理树节点右键点击事件
   *
   * 在用户自定义分类节点上右键时，记录分类信息用于构建右键菜单。
   * 虚拟分类（全部论文、未分类）不触发右键菜单。
   *
   * @param {object} info - 右键事件信息 { node, event }
   */
  const handleRightClick = ({ node }: any) => { // 右键点击处理函数
    // 虚拟节点不显示右键菜单
    if (node.key === 'all' || node.key === 'uncategorized') return; // 跳过虚拟节点
    // 注意：右键菜单的显示由 Dropdown 组件自动管理
    // 这里不需要额外处理，Dropdown 的 trigger=['contextMenu'] 已经处理了
  };

  // ========== 右键菜单"导入文献"处理 ==========
  //
  // 【设计理念】
  // 以文献条目（元数据）为核心，PDF 只是文献的一种来源。
  // 无论用户选择 PDF 还是题录文件，都是"导入文献"的统一入口。
  // 系统根据文件扩展名自动路由到对应的处理逻辑，用户无需关心底层差异。
  //
  // 路由规则：
  //   PDF 文件 → onUploadToCategory 回调（提取元数据创建文献条目，PDF 自动挂为附件）
  //   题录文件 → onImportBibliography 回调（批量导入文献元数据，支持重复检测和去重）

  /**
   * 处理统一文件导入的回调函数
   *
   * 当用户在右键菜单中点击"导入文献"后，浏览器弹出文件选择对话框。
   * 用户选择文件后，根据文件扩展名自动路由到对应的处理逻辑：
   * - PDF 文件（.pdf）→ 调用 onUploadToCategory（提取元数据创建文献条目，PDF 作为附件）
   *   后端会解析 PDF 提取标题、作者等元数据，创建一条文献记录，PDF 文件自动关联为附件
   * - 题录文件（.bib/.ris/.nbib/.enw/.xml/.txt）→ 调用 onImportBibliography（批量导入文献元数据）
   *   后端会解析题录文件，批量创建文献记录，支持重复检测和去重处理
   *
   * 该函数替代了原来的 handleFileSelect（仅处理 PDF）和 handleBibFileSelect（仅处理题录）两个函数，
   * 将它们合并为一个统一的入口，简化了用户界面和代码维护。
   *
   * @param {Event} e - 文件选择事件对象（由隐藏的 <input type="file"> 触发）
   */
  const handleImportFileSelect = async (e: any) => {
    // 从事件对象中获取用户选择的文件（只取第一个，因为 input 没有设置 multiple）
    // 可选链操作符防止 files 为 null（理论上不会发生，但做防御性检查）
    const file = e.target.files?.[0];
    if (!file) return; // 没有选择文件则直接返回（用户取消了文件选择对话框）

    // 将文件名转为小写，用于后续不区分大小写的后缀匹配
    const fileName = file.name.toLowerCase();
    // 从 ref 中获取用户右键点击的目标分类 ID
    // 该值在菜单项的 onClick 回调中设置（buildVirtualContextMenu 设为 null，buildContextMenu 设为 categoryData.id）
    // null 表示导入到"全部论文"（不归入任何分类），数字 ID 表示导入到指定分类
    const categoryId = importTargetCategoryIdRef.current;

    try {
      // ========== 根据文件扩展名路由到不同的处理逻辑 ==========
      if (fileName.endsWith('.pdf')) {
        // ===== PDF 文件路径 =====
        // 调用父组件传入的上传回调，将 PDF 文件和目标分类 ID 传递给父组件处理
        // 父组件（PaperListPage）负责实际的 API 调用、列表更新和 SSE 监听
        // 后端处理流程：保存 PDF → 解析提取元数据（标题、作者等）→ 创建文献记录 → PDF 自动挂为附件
        // 注意：categoryId 为 null 时也需要调用，表示上传但不指定分类（普通上传）
        if (onUploadToCategory) { // 检查回调是否存在（防御性检查）
          await onUploadToCategory(file, categoryId); // 调用父组件的上传处理函数
        }
      } else {
        // ===== 题录文件路径 =====
        // 校验文件扩展名是否在允许的题录格式列表中
        const isAllowed = BIBLIOGRAPHY_EXTENSIONS.some(ext => fileName.endsWith(ext));
        if (!isAllowed) {
          message.error(t('message.unsupportedFormat'));
          e.target.value = '';
          return;
        }
        // 调用父组件传入的题录导入回调，将文件和目标分类 ID 传递给父组件处理
        // 后端处理流程：解析题录文件 → 提取文献元数据 → 检测重复 → 批量创建文献记录
        if (onImportBibliography) { // 检查回调是否存在（防御性检查）
          await onImportBibliography(file, categoryId); // 调用父组件的导入处理函数
        }
      }
      // 导入成功后刷新分类树数据（更新论文计数，如 "机器学习 (5)" → "(6)"）
      await loadCategories(); // 异步刷新分类列表
    } catch (err) { // 捕获导入过程中的错误（网络错误、服务器错误等）
      // 显示统一的错误提示（无论是 PDF 上传还是题录导入的失败）
      message.error(t('message.importFailed', { error: (err as any).message })); // 弹出错误提示信息
    }

    // 重置 file input 的值，确保用户下次还能选择同一个文件
    // 如果不重置，选择相同文件时不会触发 onChange 事件（浏览器行为）
    e.target.value = ''; // 清空 input 值
  };


  // ========== 拖拽排序相关 ==========

  /**
   * 处理拖拽放置完成（onDrop 回调）
   *
   * 当用户完成拖拽操作（松开鼠标）时触发。
   * 根据放置位置计算新的父级关系和排序位置，然后调用后端 API 持久化。
   *
   * 重要：rc-tree 的 onDrop 回调传递的参数与 allowDrop 不同！
   * onDrop 接收：{ node, dragNode, dropPosition, dropToGap }
   * 其中 dropPosition 是"绝对位置"（= 内部相对位置 + 节点在父级中的索引）
   *   计算公式（rc-tree 源码 Tree.js:322）：
   *   dropPosition = internalDropPosition(-1/0/1) + nodeIndex
   * 而 dropToGap 是布尔值（内部位置 !== 0 时为 true）
   *
   * 三种放置情况：
   * 1. dropToGap=true，dropNode 为虚拟节点：
   *    放在虚拟节点间隙 → 移动到根级别
   *    dropPosition 需减去虚拟节点数量才能得到用户分类的 sort_order
   *
   * 2. dropToGap=true，dropNode 为用户分类：
   *    放在节点间隙 → 同级排序
   *    根级别：dropPosition 需减去虚拟节点数量
   *    非根级别：dropPosition 直接映射为 sort_order
   *
   * 3. dropToGap=false：放在节点上面 → 成为子节点
   *
   * @param {object} info - 拖拽放置信息
   * @param {object} info.node - 放置目标节点（dropNode 的别名）
   * @param {object} info.dragNode - 被拖拽的节点
   * @param {number} info.dropPosition - 放置的绝对位置索引
   * @param {boolean} info.dropToGap - 是否放置在间隙中（true=同级，false=成为子节点）
   */
  const handleDrop = useCallback(async (info: any) => { // 拖拽放置处理函数
    // 解构拖拽事件信息
    const { node, dragNode, dropPosition, dropToGap } = info; // 提取关键信息

    // ===== 安全检查：跳过虚拟节点作为拖拽源 =====
    // 如果被拖拽的是虚拟节点（理论上不会发生，因为 draggable 已排除），直接返回
    if (dragNode.key === 'all' || dragNode.key === 'uncategorized') { // 检查拖拽节点
      return; // 跳过虚拟节点
    }

    // ===== 检查放置目标是否为虚拟节点 =====
    const isDropOnVirtual = node.key === 'all' || node.key === 'uncategorized'; // 放置目标是否为虚拟节点

    // 如果放置在虚拟节点内部（dropToGap=false），直接返回
    // 虚拟节点没有"子分类"的概念，不能将分类放入虚拟节点
    if (isDropOnVirtual && !dropToGap) { // 放置在虚拟节点内部（非间隙）
      return; // 跳过，不允许此操作
    }

    // ===== 提取被拖拽分类的 ID =====
    // dragNode.key 是字符串类型的分类 ID，需要转换为数字
    const dragId = dragNode.key; // UUID 字符串

    // ===== 计算新的父分类 ID 和排序位置 =====
    // 这是拖拽排序的核心逻辑，需要根据放置方式确定新的层级关系
    let newParentId; // 新的父分类 ID（null 表示根级别）
    let sortOrder; // 新的排序位置（在同级用户分类节点中的索引）

    // 虚拟节点数量常量（从 constants.ts 导入）

    if (dropToGap && isDropOnVirtual) {
      // 情况A：放置在虚拟节点的间隙中（移动到根级别）
      //
      // 树形数据结构中的根级子节点排列：
      //   [0] all            ← 虚拟节点
      //   [1] uncategorized  ← 虚拟节点
      //   [2] Parent A       ← 第一个用户分类（sort_order=0）
      //   [3] Parent B       ← 第二个用户分类（sort_order=1）
      //
      // dropPosition 是绝对位置，需要减去虚拟节点数量得到用户分类的 sort_order：
      // - 放在 all 前面（绝对位置0）：sort_order = max(0, 0-2) = 0
      // - 放在 all 后面/uncategorized 前面（绝对位置1）：sort_order = max(0, 1-2) = 0
      // - 放在 uncategorized 后面（绝对位置2）：sort_order = max(0, 2-2) = 0
      // 三种情况都是 sort_order=0，即放在用户分类列表最前面
      newParentId = null; // 根级别（与虚拟节点同级，parent_id 为 null）
      // 将绝对位置转换为用户分类的 sort_order（减去虚拟节点占位）
      sortOrder = Math.max(0, dropPosition - VIRTUAL_NODE_COUNT); // 确保非负
    } else if (dropToGap) {
      // 情况B：放置在用户分类节点的间隙中（同级排序）
      // 被拖拽的节点将与 dropNode 处于同一层级

      // dropNode 的父分类 ID
      // node.raw 包含原始分类数据（id, name, parent_id 等）
      newParentId = node.raw?.parent_id ?? null; // 获取 dropNode 的父分类 ID

      if (newParentId === null) {
        // 根级别：dropPosition 包含虚拟节点的偏移，需要减去
        // 例如：放在 Parent A 前面（绝对位置1）：sort_order = max(0, 1-2) = 0
        //       放在 Parent A 后面（绝对位置3）：sort_order = max(0, 3-2) = 1
        sortOrder = Math.max(0, dropPosition - VIRTUAL_NODE_COUNT); // 减去虚拟节点偏移
      } else {
        // 非根级别：没有虚拟节点，dropPosition 直接映射为 sort_order
        sortOrder = Math.max(0, dropPosition); // 直接使用绝对位置（已确保非负）
      }
    } else {
      // 情况C：放置在用户分类节点上面（成为子节点）
      // 被拖拽的节点将成为 dropNode 的子分类

      // 新的父分类 = dropNode 自身
      newParentId = node.key; // UUID 字符串

      // 排序位置设为 0，表示放在子节点列表的最前面
      sortOrder = 0; // 成为第一个子节点
    }

    // ===== 调用后端 API 执行移动操作 =====
    try { // 开始 try-catch 错误处理
      // 调用 moveCategory API，传入分类 ID、新父级 ID 和排序位置
      // 后端会执行以下操作：
      // 1. 校验分类和父分类是否存在
      // 2. 检测循环引用（不能将分类移到自己的后代下）
      // 3. 调整同级节点的 sort_order（腾出位置）
      // 4. 更新分类的 parent_id 和 sort_order
      await moveCategory(dragId, newParentId, sortOrder); // 异步调用移动接口

      // 移动成功后，重新加载分类树数据
      // 后端已经调整了所有相关的 sort_order，刷新即可获得最新顺序
      await loadCategories(); // 异步刷新分类列表
    } catch (err) { // 捕获移动操作错误
      // 显示错误提示（可能是循环引用、分类不存在等）
      message.error(t('message.moveCategoryFailed', { error: (err as any).message })); // 弹出错误提示
      // 移动失败时不需要额外处理，分类树保持原状
    }
  }, [loadCategories, message]);


  // ========== 计算当前选中的 key ==========

  /**
   * 将 selectedCategoryId 转换为 Tree 组件需要的 selectedKeys 格式
   *
   * 转换规则：
   * - null → ['all']（选中"全部论文"）
   * - 'uncategorized' → ['uncategorized']（选中"未分类"）
   * - 数字 → [String(id)]（选中对应的分类节点）
   */
  const selectedKeys = [ // 当前选中的节点 key 数组
    selectedCategoryId === null ? 'all' : // null 表示"全部论文"
    selectedCategoryId === 'uncategorized' ? 'uncategorized' : // 字符串表示"未分类"
    String(selectedCategoryId), // 数字 ID 转为字符串
  ];


  // ========== 渲染新建根分类的内联输入框（已由 hook 提供）==========

  // renderRootCreateInput 由 useInlineEditing hook 返回，需传入 Input 组件
  // 在 JSX 中通过 renderRootCreateInput(Input) 调用


  // ========== 组件渲染 ==========

  return (
    <div style={{
      width: fillParent ? '100%' : sidebarWidth, // 填满父容器或使用拖拽宽度
      ...(fillParent ? {} : { minWidth: 180, maxWidth: 480 }), // 填满模式下不限制宽度范围
      height: fillParent ? '100%' : '100vh', // 填满模式下继承父容器高度，独立模式占满视口
      background: 'var(--bg-primary)', // 主背景色（深色模式适配）
      ...(fillParent ? {} : { borderRight: '1px solid var(--border-color)' }), // 填满模式下不显示右边框（由外部面板控制）
      display: 'flex', // 弹性布局
      flexDirection: 'column', // 纵向排列
      position: 'relative', // 为拖拽手柄提供定位参考
      userSelect: 'none', // 禁止文字选中（避免拖拽时选中文本）
    }}>
      {/* 分类树区域：可滚动容器
       * ref={treeWrapperRef}：绑定到 useEffect 中注册的原生 dragover/dragleave/drop 监听器。
       * 这些监听器在 capture 阶段执行，先于 rc-tree 的处理函数，用于检测外部文件/论文拖拽。 */}
      <div
        ref={treeWrapperRef}
        style={{
          flex: 1, // 占据剩余空间
          overflow: 'auto', // 内容超出时滚动
          padding: '4px 0', // 上下内边距 4px
        }}
        className="auto-scrollbar category-tree-wrapper" // 自动隐藏滚动条 + 隐藏展开箭头
      >
        {loading ? ( // 加载中状态
          // 加载中暂不显示（分类树数据量小，加载很快）
          <div style={{ padding: '12px', color: 'var(--text-tertiary)', fontSize: 12 }}>{t('common:loading')}</div>
        ) : categories.length === 0 ? ( // 没有分类时
          // 空状态：仍需增强节点以支持右键菜单（新建分类）
          <Tree
            treeData={enhancedTreeData} // 增强后的树形数据（包含右键菜单）
            selectedKeys={selectedKeys} // 选中的节点
            onSelect={handleSelect} // 选中事件处理
            blockNode // 节点占满整行，更容易点击
            style={{ padding: '0 4px' }} // 内边距
          />
        ) : ( // 有分类数据时
          // 渲染完整的分类树
          <Tree
            treeData={enhancedTreeData} // 增强后的树形数据（包含右键菜单和内联编辑）
            selectedKeys={selectedKeys} // 当前选中的节点
            expandedKeys={expandedKeys} // 当前展开的节点
            onSelect={handleSelect} // 选中事件处理（同时处理展开/折叠）
            onExpand={handleExpand} // 展开/折叠事件处理（保持 expandedKeys 受控）
            onRightClick={handleRightClick} // 右键点击事件处理
            // showLine 已移除：第一级竖线占空间且无实际层级指示作用，缩进+箭头已足够表达层级
            blockNode
            style={{ padding: '0 4px' }}
            draggable={draggableConfig}
            allowDrop={allowDrop}
            onDrop={handleDrop}
          />
        )}
        {/* 渲染根级别的新建输入框（在树形目录下方） */}
        {renderRootCreateInput(Input)} {/* 调用 hook 的根级别创建输入框渲染，传入 Input 组件 */}
      </div>

      {/* 拖拽调整宽度的手柄（填满模式下不渲染，由外部面板控制宽度） */}
      {!fillParent && (
        <div
          onMouseDown={handleResizeStart}
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
      )}

      {/* 统一的隐藏文件选择输入框，用于"导入文献"功能 */}
      <input
        type="file"
        accept={FILE_INPUT_ACCEPT}
        ref={importFileInputRef}
        style={{ display: 'none' }}
        onChange={handleImportFileSelect}
      />
    </div>
  );
});

export default CategorySidebar;
