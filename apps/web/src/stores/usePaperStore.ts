/**
 * 论文列表状态管理 Store
 *
 * 管理论文列表数据、加载状态、分页、排序、筛选、搜索等。
 */
import { create } from 'zustand';
import type { StateCreator } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Paper } from '@/types';

/** 分页信息 */
interface Pagination {
  current: number;
  pageSize: number;
  total: number;
}

/** Paper Store 状态接口 */
interface PaperState {
  papers: Paper[];
  loading: boolean;
  pagination: Pagination;
  sortField: string;
  sortOrder: 'asc' | 'desc';
  selectedRowKeys: string[];
  screeningMode: boolean;
  searchText: string;
  selectedCategoryId: string | null;
  directOnly: boolean;
  setPapers: (papersOrUpdater: Paper[] | ((prev: Paper[]) => Paper[])) => void;
  setLoading: (loading: boolean) => void;
  setPagination: (pagination: Pagination) => void;
  setSort: (field: string, order: 'asc' | 'desc') => void;
  setSortField: (field: string) => void;
  setSortOrder: (order: 'asc' | 'desc' | ((prev: 'asc' | 'desc') => 'asc' | 'desc')) => void;
  setSelectedRowKeys: (keys: string[]) => void;
  setScreeningMode: (mode: boolean) => void;
  setSearchText: (text: string) => void;
  setSelectedCategoryId: (categoryId: string | null) => void;
  setDirectOnly: (direct: boolean) => void;
  reset: () => void;
}

type PaperStoreCreator = StateCreator<PaperState>;

const usePaperStore = create<PaperState>()(
  persist(
    ((set) => ({
      // 论文列表数据
      papers: [],
      // 加载状态
      loading: true,
      // 分页信息：当前页、每页数量、总数
      pagination: { current: 1, pageSize: 20, total: 0 },
      // 排序字段
      sortField: 'createdAt',
      // 排序顺序：'asc' | 'desc'
      sortOrder: 'desc',
      // 选中的行 key 列表（用于批量操作）
      selectedRowKeys: [],
      // 筛选模式标志
      screeningMode: false,
      // 搜索关键词
      searchText: '',
      // 选中的分类 ID
      selectedCategoryId: null,
      // 仅显示直接添加的论文（排除引用的论文）
      directOnly: false,

      /** 设置论文列表
       * @param papersOrUpdater - 论文数组或函数式更新器
       * - 如果是数组：直接替换论文列表
       * - 如果是函数：接收当前论文列表，返回新的论文列表（支持 React 的 setState 函数式更新模式）
       */
      setPapers: (papersOrUpdater: Paper[] | ((prev: Paper[]) => Paper[])) => set((state) => ({
        papers: typeof papersOrUpdater === 'function' ? papersOrUpdater(state.papers) : papersOrUpdater,
      })),
      // 设置加载状态 - 控制论文列表加载指示器的显示
      setLoading: (loading: boolean) => set({ loading }),
      // 设置分页信息 - 更新当前页、每页数量、总数
      setPagination: (pagination: Pagination) => set({ pagination }),
      // 同时设置排序字段和顺序 - 一次性更新排序条件
      setSort: (field: string, order: 'asc' | 'desc') => set({ sortField: field, sortOrder: order }),
      // 设置排序字段 - 指定按哪个字段排序（如 createdAt、title 等）
      setSortField: (field: string) => set({ sortField: field }),
      // 设置排序顺序（支持函数式更新）- asc 升序或 desc 降序
      setSortOrder: (order: 'asc' | 'desc' | ((prev: 'asc' | 'desc') => 'asc' | 'desc')) => set((state) => ({
        sortOrder: typeof order === 'function' ? order(state.sortOrder) : order
      })),
      // 设置选中的行（用于批量操作）- 存储选中论文的 key 列表
      setSelectedRowKeys: (keys: string[]) => set({ selectedRowKeys: keys }),
      // 设置筛选模式 - 开启时进入文献筛选流程
      setScreeningMode: (mode: boolean) => set({ screeningMode: mode }),
      // 设置搜索关键词 - 用于论文列表的本地过滤
      setSearchText: (text: string) => set({ searchText: text }),
      // 设置选中的分类 ID - 按分类筛选论文列表
      setSelectedCategoryId: (categoryId: string | null) => set({ selectedCategoryId: categoryId }),
      // 设置是否仅显示直接添加的论文 - 排除引用/推荐的论文
      setDirectOnly: (direct: boolean) => set({ directOnly: direct }),
      // 重置所有状态为默认值 - 清除筛选、排序、选择等状态
      reset: () => set({
        papers: [],
        loading: true,
        pagination: { current: 1, pageSize: 20, total: 0 },
        sortField: 'createdAt',
        sortOrder: 'desc',
        selectedRowKeys: [],
        screeningMode: false,
        searchText: '',
        selectedCategoryId: null,
        directOnly: false,
      }),
    })) as PaperStoreCreator,
    {
      name: 'autonomics-paper-storage',
      version: 2,
      migrate: (persistedState: unknown, version: number) => {
        // 数据版本迁移：从 v0 升级到 v2
        // v0 使用 snake_case 字段名（created_at），v2 改为 camelCase（createdAt）
        // 需要将旧字段名映射到新字段名，确保历史数据兼容
        const persisted = persistedState as Record<string, unknown>;
        if (version === 0) {
          const state = persisted as Record<string, unknown>;
          if (state.sortField === 'created_at') state.sortField = 'createdAt';
          if (state.sortField === 'updated_at') state.sortField = 'updatedAt';
          if (state.sortField === 'publication_year') state.sortField = 'publicationYear';
        }
        return persisted;
      },
      partialize: (state: PaperState) => ({
        sortField: state.sortField,
        sortOrder: state.sortOrder,
        screeningMode: state.screeningMode,
        searchText: state.searchText,
        selectedCategoryId: state.selectedCategoryId,
      }),
    }
  )
);

export default usePaperStore;
