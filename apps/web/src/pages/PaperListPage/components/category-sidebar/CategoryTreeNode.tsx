/**
 * CategoryTreeNode.jsx
 * ============================================================
 * JayRead 分类侧边栏树节点组件
 * 从 CategorySidebar 中提取的独立组件
 *
 * 核心功能：
 * - 渲染单个分类树节点（虚拟节点或用户分类节点）
 * - 右键菜单（新建子分类、重命名、删除、导入文献）
 * - 通过 data-category-key 属性供 CategorySidebar 的拖拽系统定位目标分类
 * - 内联编辑输入框
 * - 论文计数显示
 *
 * 【拖拽高亮说明】
 * 拖拽高亮不在此组件内处理，而是由 CategorySidebar 通过原生 DOM capture 事件
 * 直接在 .ant-tree-treenode 元素上添加/移除 CSS 类（类 Zotero 做法）。
 * 本组件通过 data-category-key 属性提供"分类 ID → DOM 元素"的映射关系，
 * 供 CategorySidebar 的 getCategoryKeyFromEvent() 函数查找目标分类。
 *
 * 技术栈：
 * - Ant Design：Dropdown
 * ============================================================
 */

import React from 'react';
import { Dropdown } from 'antd';
import { ImportOutlined, FolderAddOutlined, EditOutlined, DeleteOutlined } from '@ant-design/icons';

/**
 * CategoryTreeNode 组件
 * 渲染单个分类树节点，支持右键菜单、内联编辑等功能
 *
 * @param {Object} props - 组件属性
 * @param {Object} props.node - 树节点对象 { key, title, raw, children }
 * @param {Function} props.renderEnhancedTitle - 增强标题渲染函数（支持内联编辑）
 * @param {Function} props.getContextMenu - 获取用户分类右键菜单的函数
 * @param {Function} props.getVirtualContextMenu - 获取虚拟节点右键菜单的函数
 * @param {Function} props.loadCategories - 刷新分类树的回调
 * @param {Function} props.setExpandedKeys - 设置展开节点的方法
 * @param {Array} props.expandedKeys - 当前展开的节点 key 数组
 * @param {Function} props.setEditingNode - 设置编辑节点的方法
 * @param {Function} props.onSelectDirectOnly - "仅显示直属论文"回调
 * @param {Function} props.handleDelete - 删除分类的回调
 * @param {Function} props.onPapersUpdate - 论文列表更新回调
 * @param {object} props.message - Ant Design message 对象
 */
const CategoryTreeNode = React.memo(({
    node,
    renderEnhancedTitle,
    getContextMenu,
    getVirtualContextMenu,
    loadCategories,
    setExpandedKeys,
    expandedKeys,
    setEditingNode,
    onSelectDirectOnly,
    handleDelete,
    onPapersUpdate,
    message,
}: any) => {
    // 从 node.raw 中获取原始分类数据对象（包含 id, name, parent_id, paper_count 等）
    const raw = node.raw;

    // 判断当前节点是否为虚拟节点（系统内置的特殊分类，不在后端数据库中）
    // 'all' = 全部论文，'uncategorized' = 未分类
    const isVirtual = node.key === 'all' || node.key === 'uncategorized';

    // 获取右键菜单配置
    // 虚拟节点和用户分类节点有不同的菜单项
    const contextMenu = isVirtual ? getVirtualContextMenu() : getContextMenu(raw);

    return (
        // Ant Design Dropdown 组件，用于显示右键菜单
        // trigger={['contextMenu']} 表示只有右键点击时才弹出菜单
        <Dropdown
            menu={contextMenu} // 菜单配置（items 数组）
            trigger={['contextMenu']} // 触发方式：仅右键
        >
            {/* 树节点标题容器 */}
            {/* data-category-key：关键属性！供 CategorySidebar 的拖拽系统通过 DOM 查找目标分类。
                拖拽 drop 事件处理函数通过 closest('.ant-tree-treenode').querySelector('[data-category-key]')
                找到此元素，读取 dataset.categoryKey 得到分类 ID。 */}
            <span data-category-key={node.key}>
                {/* 虚拟节点使用静态标题，用户分类节点使用增强渲染（支持内联编辑） */}
                {isVirtual ? node.title : (renderEnhancedTitle ? renderEnhancedTitle(raw) : node.title)}
            </span>
        </Dropdown>
    );
});

/**
 * 构建虚拟节点的右键菜单
 *
 * 虚拟节点（全部论文、未分类）提供：
 * - 导入文献（统一入口，接受 PDF 和题录文件）
 * - 新建分类
 *
 * @param {Object} options - 菜单配置选项
 * @param {Function} options.setContextMenuInfo - 设置右键菜单状态的函数
 * @param {Function} options.setEditingNode - 设置编辑节点的函数
 * @param {React.RefObject} options.importFileInputRef - 统一导入文件输入框引用（同时接受 PDF 和题录文件）
 * @param {React.RefObject} options.importTargetCategoryIdRef - 导入目标分类 ID 引用（null 表示不指定分类）
 * @returns {Object} Ant Design Menu 菜单配置
 */
export const buildVirtualContextMenu = ({
    setContextMenuInfo,           // 关闭右键菜单的回调函数
    setEditingNode,               // 设置编辑节点状态的函数（用于触发新建/重命名内联编辑）
    importFileInputRef,           // 统一的隐藏文件输入框 ref（HTMLInputElement）
    importTargetCategoryIdRef,    // 导入目标分类 ID 的 ref（用于记录用户右键点击的分类）
}: any) => ({
    items: [
        {
            key: 'import',                    // 菜单项的唯一标识符
            icon: <ImportOutlined />,         // 使用 ImportOutlined 图标，体现"导入"语义
            label: '导入文献',                 // 菜单项显示文本（合并了原来的"上传文件"和"导入题录"）
            onClick: () => {
                // 虚拟节点（全部论文/未分类）没有分类 ID，设为 null
                // 后续 handleImportFileSelect 会读取此 ref 来决定导入到哪个分类
                importTargetCategoryIdRef.current = null; // null = 不归入任何分类
                // 通过编程方式触发隐藏的 file input 的 click 事件，弹出系统文件选择对话框
                // 可选链操作符防止 ref 未绑定时报错
                importFileInputRef.current?.click(); // 触发文件选择
                // 关闭右键菜单（将状态重置为 null）
                setContextMenuInfo?.(null); // 清除右键菜单状态
            },
        },
        { type: 'divider' }, // 分隔线：将"导入文献"和"新建分类"视觉分隔开
        {
            key: 'create',                       // 菜单项的唯一标识符
            icon: <FolderAddOutlined />,         // 新建文件夹图标
            label: '新建分类',                    // 菜单项显示文本
            onClick: () => {
                // 设置编辑节点状态为"新建根分类"模式
                // type: 'create' 表示新建操作
                // parentId: null 表示根级别分类（没有父分类）
                // value: '' 表示初始输入值为空
                setEditingNode?.({
                    type: 'create',              // 操作类型：新建分类
                    parentId: null,              // 父分类 ID：null 表示根级别
                    value: '',                   // 初始输入值：空字符串
                });
                // 关闭右键菜单
                setContextMenuInfo?.(null); // 清除右键菜单状态
            },
        },
    ],
});

/**
 * 构建用户分类节点的右键菜单
 *
 * 用户分类节点提供：
 * - 导入文献（统一入口，接受 PDF 和题录文件）
 * - 新建子分类
 * - 重命名
 * - 仅显示直属论文
 * - 删除分类
 *
 * @param {Object} options - 菜单配置选项
 * @param {Object} options.categoryData - 分类数据对象（包含 id, name, parent_id 等字段）
 * @param {Function} options.setEditingNode - 设置编辑节点的函数（触发内联编辑模式）
 * @param {Function} options.setContextMenuInfo - 设置右键菜单状态的函数（用于关闭菜单）
 * @param {Function} options.setExpandedKeys - 设置展开节点的函数（用于新建子分类时自动展开父节点）
 * @param {Function} options.onSelectDirectOnly - "仅显示直属论文"回调（过滤掉后代分类的论文）
 * @param {Function} options.handleDelete - 删除分类的回调（弹出确认对话框后调用 API）
 * @param {Array} options.expandedKeys - 当前展开的节点 key 数组
 * @param {React.RefObject} options.importFileInputRef - 统一导入文件输入框引用（同时接受 PDF 和题录文件）
 * @param {React.RefObject} options.importTargetCategoryIdRef - 导入目标分类 ID 引用（记录用户右键点击的分类 ID）
 * @returns {Object} Ant Design Menu 菜单配置
 */
export const buildContextMenu = ({
    categoryData,                 // 被右键点击的分类数据对象 { id, name, parent_id, paper_count, ... }
    setEditingNode,               // 设置编辑节点状态的函数（用于触发新建/重命名内联编辑）
    setContextMenuInfo,           // 关闭右键菜单的回调函数
    setExpandedKeys,              // 设置展开节点的函数（新建子分类时自动展开父节点）
    onSelectDirectOnly,           // "仅显示直属论文"回调（仅显示直接归属该分类的论文，不含子分类的）
    handleDelete,                 // 删除分类的处理函数（含确认对话框）
    expandedKeys,                 // 当前已展开的节点 key 数组（用于判断是否需要展开父节点）
    importFileInputRef,           // 统一的隐藏文件输入框 ref（HTMLInputElement）
    importTargetCategoryIdRef,    // 导入目标分类 ID 的 ref（用于记录用户右键点击的分类）
}: any) => ({
    items: [
        {
            key: 'import',                    // 菜单项的唯一标识符
            icon: <ImportOutlined />,         // 使用 ImportOutlined 图标，体现"导入"语义
            label: '导入文献',                 // 菜单项显示文本（合并了原来的"上传文件"和"导入题录"）
            onClick: () => {
                // 用户分类节点有真实的分类 ID，记录到 ref 中
                // 后续 handleImportFileSelect 会读取此 ref 来决定导入到哪个分类
                importTargetCategoryIdRef.current = categoryData.id; // 记录目标分类 ID
                // 通过编程方式触发隐藏的 file input 的 click 事件，弹出系统文件选择对话框
                // 可选链操作符防止 ref 未绑定时报错
                importFileInputRef.current?.click(); // 触发文件选择
                // 关闭右键菜单（将状态重置为 null）
                setContextMenuInfo?.(null); // 清除右键菜单状态
            },
        },
        { type: 'divider' }, // 分隔线：将"导入文献"和分类管理操作视觉分隔开
        {
            key: 'create',                       // 菜单项的唯一标识符
            icon: <FolderAddOutlined />,         // 新建文件夹图标
            label: '新建子分类',                  // 菜单项显示文本
            onClick: () => {
                // 设置编辑节点状态为"新建子分类"模式
                // type: 'create' 表示新建操作
                // parentId: categoryData.id 表示父分类为当前右键点击的分类
                // value: '' 表示初始输入值为空
                setEditingNode?.({
                    type: 'create',              // 操作类型：新建分类
                    parentId: categoryData.id,   // 父分类 ID：当前右键点击的分类
                    value: '',                   // 初始输入值：空字符串
                });
                // 自动展开父节点，让用户能立即看到新建的子分类
                // 将分类 ID 转为字符串（Tree 组件的 key 是字符串类型）
                const parentKey = String(categoryData.id); // 将数字 ID 转为字符串 key
                // 仅在父节点当前未展开时才展开（避免重复设置导致不必要的 re-render）
                if (setExpandedKeys && expandedKeys && !expandedKeys.includes(parentKey)) {
                    setExpandedKeys([...expandedKeys, parentKey]); // 展开父节点
                }
                // 关闭右键菜单
                setContextMenuInfo?.(null); // 清除右键菜单状态
            },
        },
        {
            key: 'rename',                       // 菜单项的唯一标识符
            icon: <EditOutlined />,              // 编辑图标
            label: '重命名',                      // 菜单项显示文本
            onClick: () => {
                // 设置编辑节点状态为"重命名"模式
                // type: 'rename' 表示重命名操作
                // categoryId: categoryData.id 表示要重命名的分类 ID
                // value: categoryData.name 表示初始值为当前分类名称（方便用户修改）
                setEditingNode?.({
                    type: 'rename',                // 操作类型：重命名分类
                    categoryId: categoryData.id,   // 要重命名的分类 ID
                    value: categoryData.name,      // 初始输入值：当前分类名称
                });
                // 关闭右键菜单
                setContextMenuInfo?.(null); // 清除右键菜单状态
            },
        },
        {
            key: 'direct-only',                  // 菜单项的唯一标识符
            label: '仅显示直属论文',              // 菜单项显示文本（无图标）
            onClick: () => {
                // 关闭右键菜单
                setContextMenuInfo?.(null); // 清除右键菜单状态
                // 调用"仅显示直属论文"回调，传入分类 ID
                // 此功能会过滤掉后代分类中的论文，只显示直接归属该分类的论文
                if (onSelectDirectOnly) {
                    onSelectDirectOnly(categoryData.id); // 触发直属论文筛选
                }
            },
        },
        { type: 'divider' }, // 分隔线：将常规操作和危险操作（删除）视觉分隔开
        {
            key: 'delete',                        // 菜单项的唯一标识符
            icon: <DeleteOutlined />,             // 删除图标
            label: '删除分类',                     // 菜单项显示文本
            danger: true,                         // Ant Design 危险样式（红色文字）
            onClick: () => {
                // 关闭右键菜单
                setContextMenuInfo?.(null); // 清除右键菜单状态
                // 调用删除处理函数，传入分类数据对象
                // handleDelete 内部会弹出确认对话框，用户确认后才真正删除
                handleDelete?.(categoryData); // 触发删除确认流程
            },
        },
    ],
});

export default CategoryTreeNode;
