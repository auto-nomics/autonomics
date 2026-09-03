/**
 * useActiveColumns Hook
 *
 * 构建表格视图的活跃列配置
 *
 * 根据 visibleColumns 状态过滤 ALL_COLUMNS，生成 Ant Design Table 的 columns 属性。
 * 使用 useMemo 缓存计算结果，只在 visibleColumns 或依赖数据变化时重新计算。
 *
 * 为什么标题列的 render 需要在组件内部定义？
 * - 标题列的渲染依赖 navigate（路由导航）、categoriesCache（分类数据）等组件级变量
 * - 这些变量在 ALL_COLUMNS 常量定义时还不可用
 * - 所以 ALL_COLUMNS 中标题列的 render 为 null，在此处动态覆盖
 */
import { useMemo } from 'react';
import { Space, Tooltip } from 'antd';
import { FolderOutlined, CloseOutlined } from '@ant-design/icons';
import { useTranslation } from 'react-i18next';
import type { Paper, Category } from '@/types';
import { ALL_COLUMNS, SORT_FIELD_MAP } from '../paperListConstants';

/** 列定义 */
interface ColumnDef {
  key: string;
  label: string;
  width: number;
  fixed?: string;
  ellipsis?: boolean;
  sortable?: boolean;
  render?: (paper: Paper) => React.ReactNode;
}

/** useActiveColumns 参数 */
interface UseActiveColumnsParams {
  visibleColumns: string[];
  columnWidths: Record<string, number>;
  paperCategoryMap: Record<string, string[]>;
  categoriesCache: Category[];
  navigate: (path: string) => void;
  handleUnassignFromCategory: (e: React.MouseEvent, paperId: string, categoryId: string) => void;
  getCategoryPath: (catId: string) => string;
  handleResize: (colKey: string, width: number) => void;
  sortField: string;
  sortOrder: 'asc' | 'desc';
  screeningMode: boolean;
  handleExpand: (expanded: boolean, paper: Paper) => void;
  expandedRowKeys: string[];
  setLeftPanelView: (view: string) => void;
  setSelectedPaperForScreening: (paper: Paper | null) => void;
  setSortField: (field: string) => void;
  setSortOrder: React.Dispatch<React.SetStateAction<'asc' | 'desc'>> | ((order: 'asc' | 'desc' | ((prev: 'asc' | 'desc') => 'asc' | 'desc')) => void);
}

/**
 * 自定义 Hook：生成活跃列配置
 *
 * @param {Object} config - 配置对象
 * @param {Array<string>} config.visibleColumns - 当前可见的列 key 数组
 * @param {Object} config.columnWidths - 列宽配置对象 { columnKey: width }
 * @param {Object} config.paperCategoryMap - 论文-分类映射表 { paperId: [categoryId1, categoryId2, ...] }
 * @param {Array} config.categoriesCache - 分类树数据缓存 [{ id, name, parent_id }, ...]
 * @param {Function} config.navigate - 路由导航函数
 * @param {Function} config.handleUnassignFromCategory - 取消分类关联回调
 * @param {Function} config.getCategoryPath - 获取分类路径函数
 * @param {Function} config.handleResize - 列宽拖拽回调
 * @param {string} config.sortField - 当前排序字段（后端字段名）
 * @param {string} config.sortOrder - 当前排序方向（'asc'/'desc'）
 * @param {boolean} config.screeningMode - 筛选模式状态
 * @param {Function} config.handleExpand - 展开/折叠回调
 * @param {Array<number>} config.expandedRowKeys - 已展开行 key 列表
 * @param {Function} config.setLeftPanelView - 设置左面板视图回调
 * @param {Function} config.setSelectedPaperForScreening - 设置选中论文回调
 * @param {Function} config.setSortField - 设置排序字段回调
 * @param {Function} config.setSortOrder - 设置排序方向回调
 * @returns {Array} activeColumns - Ant Design Table 的列配置数组
 */
export default function useActiveColumns({
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
}: UseActiveColumnsParams) {
  const { t } = useTranslation('paperList');
  return useMemo(() => {
    // 将 ALL_COLUMNS 转为 key → column 的映射表，方便快速查找
    const columnMap: Record<string, any> = {};
    ALL_COLUMNS.forEach((c: any) => { columnMap[c.key] = c; });

    // 按 visibleColumns 数组的顺序构建活跃列列表
    // 先过滤出有效的列 key（防御性检查：确保 key 在 ALL_COLUMNS 中存在）
    return visibleColumns
      .filter(key => columnMap[key])
      .map(key => {
        const col = columnMap[key];

        // 构建渲染函数：如果列有自定义 render，使用它；否则直接显示字段值
        let renderFn: ((text: unknown, paper: Paper, index: number) => React.ReactNode) | undefined = undefined;
        if (col.render) {
          // Ant Design Table 的 render 签名是 (text, record, index)
          // 我们的列定义 render 签名是 (paper)
          // 需要适配：第二个参数 record 就是 paper 对象
          renderFn = (_: unknown, paper: Paper) => col.render!(paper);
        } else if (key === 'title') {
          // 标题列的特殊渲染（依赖组件级变量）
          renderFn = (_: unknown, paper: Paper) => {
            // 构建论文所属文件夹路径列表
            const paperCats = (paperCategoryMap[paper.id] || [])
              .map((catId: string) => categoriesCache.find(c => c.id === catId))
              .filter(Boolean) as Category[];

            return (
              <Space size={4}>
                {/* 论文标题：可点击跳转到阅读器页面，悬浮显示所属文件夹路径 */}
                <Tooltip
                  title={
                    <div style={{ display: 'flex', flexDirection: 'column', maxWidth: 400 }}>
                      {/* 论文标题：方便用户复制 */}
                      <div style={{ wordBreak: 'break-word' }}>
                        {paper.title || t('screening.unnamed')}
                      </div>
                      {/* 所属文件夹路径列表：方便查看和控制论文的分类归属 */}
                      {paperCats.length > 0 && (
                        <div style={{ borderTop: '1px solid var(--border-color-secondary)', paddingTop: 4, marginTop: 4 }}>
                          {paperCats.map(cat => (
                            <div key={cat.id} style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 12 }}>
                              <FolderOutlined style={{ fontSize: 10, opacity: 0.7 }} />
                              <span>{getCategoryPath(cat.id)}</span>
                              <CloseOutlined
                                style={{ fontSize: 10, cursor: 'pointer', opacity: 0.6 }}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  handleUnassignFromCategory(e, paper.id, cat.id);
                                }}
                              />
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  }
                  mouseEnterDelay={0.4}
                  placement="topLeft"
                >
                  <span
                    style={{ cursor: 'pointer', fontWeight: 500, display: 'inline-block', maxWidth: '100%', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
                    onClick={(e) => {
                      e.stopPropagation();
                      if (screeningMode) {
                        // 筛选模式：选中论文，更新左面板和 AI 对话面板
                        // 详情由 useSelectedPaperDetail 监听 selectedPaperForScreening 自动拉取
                        setSelectedPaperForScreening(paper);
                        setLeftPanelView('paper-detail');
                      } else {
                        // 正常模式：展开/折叠附件列表
                        const isExpanded = expandedRowKeys.includes(paper.id);
                        handleExpand(!isExpanded, paper);
                      }
                    }}
                  >
                    {paper.title || t('screening.unnamed')}
                  </span>
                </Tooltip>
              </Space>
            );
          };
        }

        // 使用用户拖拽调整后的宽度，如果没有则使用默认宽度
        const colWidth = columnWidths[col.key] || col.width;

        /**
         * 将列 key 映射为后端 API 的排序字段名
         */
        const apiField = (SORT_FIELD_MAP as any)[col.key] || col.key;
        /**
         * 判断当前列是否为正在排序的列
         */
        const isSorted = sortField === apiField;

        // 返回 Ant Design Table 的列配置对象
        return {
          title: col.label,
          dataIndex: col.key,
          key: col.key,
          width: colWidth,
          fixed: col.fixed || undefined,
          ellipsis: col.ellipsis !== false,
          sorter: col.sortable ? true : undefined,
          sortOrder: col.sortable ? (isSorted ? (sortOrder === 'asc' ? 'ascend' : 'descend') : null) : undefined,
          render: renderFn,
          onHeaderCell: () => {
            const cellProps: any = {
              width: colWidth,
              onResize: (w: number) => handleResize(col.key, w),
            };

            if (col.sortable) {
              cellProps.onClick = () => {
                if (sortField === apiField) {
                  setSortOrder((prev: 'asc' | 'desc') => prev === 'asc' ? 'desc' : 'asc');
                } else {
                  setSortField(apiField);
                  setSortOrder('asc');
                }
              };
            }

            // 筛选模式下：点击标题列表头时，左面板切换回分类树视图
            if (screeningMode && key === 'title') {
              const existingOnClick = cellProps.onClick;
              cellProps.onClick = () => {
                existingOnClick?.();
                setLeftPanelView('category');
                setSelectedPaperForScreening(null);
              };
            }

            return cellProps;
          },
        };
      });
  }, [
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
  ]);
}
