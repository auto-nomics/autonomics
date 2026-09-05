/**
 * PaperListPage 论文列表页常量定义
 *
 * 本文件集中管理论文列表页面的所有常量配置，包括：
 * - 解析状态配置（STATUS_CONFIG）：定义论文解析各个阶段的显示样式
 * - 解析引擎配置（ENGINE_CONFIG）：区分不同解析引擎的显示标签
 * - 开放获取状态配置（OA_STATUS_CONFIG）：OA 论文的类型标识
 * - 表格列定义（ALL_COLUMNS）：Zotero/EndNote 风格表格的列配置
 * - 默认可见列（DEFAULT_VISIBLE_COLUMNS）：首次使用时默认显示的列
 * - 排序字段映射（SORT_FIELD_MAP）：前端列 key 到后端字段名的映射
 *
 * 为什么将常量提取为独立文件？
 * - 集中管理配置，便于统一调整样式和行为
 * - 避免在主组件文件中写大量的常量定义
 * - 扩展新状态或列时只需添加一个配置项
 * - 便于其他组件复用这些配置
 */

// 从 Ant Design Icons 导入图标组件，用于状态配置
import {
  CheckCircleOutlined, // 对勾圆圈图标：表示成功/完成
  CloseCircleOutlined, // 叉号圆圈图标：表示失败/错误
  ClockCircleOutlined, // 时钟图标：表示等待中
  SyncOutlined,        // 同步旋转图标：表示解析中
} from '@ant-design/icons';
// 导入 Ant Design 组件，供 ALL_COLUMNS 中的 render 函数使用
import { Tooltip, Tag } from 'antd';
// 导入文献类型显示工具函数：类型标签颜色、中文名称、可筛选类型列表
import { getTypeLabel, getTypeColor, FILTERABLE_TYPES } from "../../utils/literatureTypes";  // 类型显示工具函数和筛选配置
// 导入样式模块
import styles from './paperListConstants.module.css';
// 导入 i18next 用于模块级常量国际化
import i18next from 'i18next';

/**
 * 解析状态配置映射表
 *
 * 为什么使用配置对象而不是 switch/if？
 * - 集中管理状态样式，便于统一调整
 * - 避免在 JSX 中写大量的条件判断
 * - 扩展新状态时只需添加一个配置项
 *
 * 颜色说明：
 * - default: 灰色，表示等待/默认状态
 * - processing: 蓝色动画，表示进行中
 * - success: 绿色，表示成功完成
 * - error: 红色，表示失败
 */
export const STATUS_CONFIG = { // 状态配置常量，key 为后端返回的状态字符串
  // 等待解析：论文已上传，排队等待后端处理
  pending: {
    color: 'default',    // Ant Design Tag 颜色：灰色（默认）
    icon: <ClockCircleOutlined />, // 时钟图标：表示等待
    text: i18next.t('paperList:status.pending'),    // 显示文本
  },
  // 解析中：后端正在处理，spin 属性让图标旋转
  parsing: {
    color: 'processing', // Ant Design Tag 颜色：蓝色动画（进行中）
    icon: <SyncOutlined spin />,   // 旋转同步图标：表示正在处理
    text: i18next.t('paperList:status.parsing'),    // 显示文本
  },
  // 后端实际存储的解析中状态（前端 SSE 收到 progress 后会覆盖为 parsing）
  processing: {
    color: 'processing',
    icon: <SyncOutlined spin />,
    text: i18next.t('paperList:status.parsing'),
  },
  // 已就绪：不再单独显示，直接由引擎标签（MinerU）表示
  done: {
    color: 'success',    // Ant Design Tag 颜色：绿色（成功）
    icon: <CheckCircleOutlined />, // 对勾图标：表示完成
    text: i18next.t('paperList:status.done'),      // 显示文本
    // 注意：此状态在渲染时由引擎标签替代，不直接显示"已就绪"文本
  },
  // 解析失败：无法处理该 PDF（如损坏、加密等）
  failed: {
    color: 'error',      // Ant Design Tag 颜色：红色（错误）
    icon: <CloseCircleOutlined />, // 叉号图标：表示失败
    text: i18next.t('paperList:status.failed'),    // 显示文本
  },
};

/**
 * 解析引擎配置映射表
 *
 * 为什么需要区分引擎？
 * - 不同引擎对同一 PDF 的解析效果可能不同
 * - 用户可以根据效果选择重解析时使用哪个引擎
 * - MinerU: 更适合中文论文和复杂排版
 */
export const ENGINE_CONFIG = { // 引擎配置常量
  mineru: {
    color: 'blue',   // Ant Design Tag 颜色：蓝色
    text: 'MinerU',  // 显示文本
  },
};

/**
 * 开放获取（OA）状态配置映射表
 *
 * 用于显示论文的开放获取状态标签
 * 颜色说明：
 * - gold: 金色 OA（出版商版本直接开放获取）
 * - green: 绿色 OA（预印本或自存档版本开放获取）
 * - bronze: 铜色 OA（在出版商网站上免费阅读，但可能不是正式 OA 许可）
 * - hybrid: 混合型 OA（付费期刊中选择性开放获取的论文）
 * - closed: 非开放获取（需要付费或订阅）
 */
export const OA_STATUS_CONFIG = {
  gold: {
    color: 'gold',   // 金色标签
    text: i18next.t('paperList:oa.gold'), // 显示文本
  },
  green: {
    color: 'green',  // 绿色标签
    text: i18next.t('paperList:oa.green'), // 显示文本
  },
  bronze: {
    color: 'orange', // 橙色标签（Ant Design 无 bronze 预设，使用 orange 代替）
    text: i18next.t('paperList:oa.bronze'), // 显示文本
  },
  hybrid: {
    color: 'cyan',   // 青色标签
    text: i18next.t('paperList:oa.hybrid'), // 显示文本
  },
  closed: {
    color: 'default', // 灰色标签
    text: i18next.t('paperList:oa.closed'),   // 显示文本
  },
};

/**
 * 表格视图的所有可用列定义
 *
 * 类似 Zotero/EndNote 的表格视图，每列对应论文的一个字段。
 * 用户可以通过列设置弹窗（Checkbox.Group）自定义显示哪些列。
 *
 * 列定义包含：
 * - key: 列的唯一标识，用于 localStorage 持久化和列过滤
 * - label: 列标题，显示在表头
 * - width: 列宽度（像素），用于固定布局
 * - sortable: 是否支持排序（点击表头排序），只有数值和日期类字段适合排序
 * - hideable: 是否允许隐藏（标题列始终显示，不可隐藏）
 * - ellipsis: 超出宽度时是否显示省略号
 * - render: 自定义渲染函数，接收 paper 对象，返回 JSX
 *
 * 为什么使用配置数组而不是硬编码列？
 * - 集中管理列属性，便于统一调整样式和行为
 * - 动态过滤和排序列，实现用户自定义列显示
 * - 扩展新列时只需添加一个配置项
 */
export const ALL_COLUMNS = [
  // ===== 标题列 =====
  // 标题是核心列，始终显示、不可隐藏、固定在左侧
  {
    key: 'title', // 列标识：标题
    label: i18next.t('paperList:column.title'), // 列标题文本
    width: 300, // 列宽 300px，为标题和标签预留足够空间
    sortable: false, // 标题不支持排序（字符串排序意义不大）
    hideable: false, // 标题列不可隐藏（始终显示）
    ellipsis: true, // 超出宽度时省略号截断（与期刊列行为一致）
    // render 函数在组件内部定义，因为它依赖 navigate、categoriesCache 等
    // 这里使用 null 占位，实际的 render 在 buildColumnRenderMap 中覆盖
    render: null,
  },
  // ===== 作者列 =====
  // 显示论文作者列表，从题录导入或 PDF 元数据中获取
  {
    key: 'authors', // 列标识：作者
    label: i18next.t('paperList:column.authors'), // 列标题文本
    width: 160, // 列宽 160px，足够显示前几位作者
    sortable: false, // 作者不支持排序
    hideable: true, // 允许用户隐藏
    ellipsis: true, // 超出宽度时省略号截断
    // 作者渲染：显示为数组，用逗号分隔
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.authors || paper.authors.length === 0) {
        return <span className={styles.textTertiary}>-</span>; // 无作者时显示灰色短横线
      }
      // authors 现在是数组，直接用逗号连接
      const text = Array.isArray(paper.authors) ? paper.authors.join(', ') : String(paper.authors);
      return (
        <span title={text}>
          {text.length > 30 ? text.slice(0, 30) + '...' : text}
        </span>
      ); // 超过 30 字符时截断并显示省略号
    },
  },
  // ===== 期刊列 =====
  // 显示期刊名称（如 Nature, Science 等）
  {
    key: 'journal', // 列标识：期刊
    label: i18next.t('paperList:column.journal'), // 列标题文本
    width: 160, // 列宽 160px
    sortable: false, // 期刊名不支持排序
    hideable: true, // 允许用户隐藏
    ellipsis: true, // 超出宽度时省略号
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.journal_name) {
        return <span className={styles.textTertiary}>-</span>; // 无期刊信息时显示灰色短横线
      }
      // 使用 Tooltip 显示完整期刊名，单元格内截断显示
      return (
        <Tooltip title={paper.journal_name}>
          <span className={styles.journalLink}>
            {paper.journal_name}
          </span>
        </Tooltip>
      ); // 期刊名用主题色加粗显示
    },
  },
  // ===== 发表年份列 =====
  // 显示论文发表年份，支持数值排序
  {
    key: 'year', // 列标识：年份
    label: i18next.t('paperList:column.year'), // 列标题文本
    width: 70, // 列宽 70px（4 位数字足够）
    sortable: true, // 支持按年份排序（数值排序）
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 年份不会超出宽度
    render: (paper: any) => { // 自定义渲染函数
      return <span>{paper.publication_year || '-'}</span>; // 无年份时显示短横线
    },
  },
  // ===== 被引次数列 =====
  // 显示论文被引用次数（来自 OpenAlex 数据），支持数值排序
  {
    key: 'citations', // 列标识：被引次数
    label: i18next.t('paperList:column.citations'), // 列标题文本
    width: 70, // 列宽 70px
    sortable: true, // 支持按被引次数排序
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 数字不会超出宽度
    render: (paper: any) => { // 自定义渲染函数
      const className = paper.citation_count > 0 ? styles.textPrimary : styles.textTertiary;
      return (
        <span className={className}>
          {paper.citation_count || '-'}
        </span>
      ); // 有引用时用主文字色，无引用时灰色
    },
  },
  // ===== DOI 列 =====
  // 显示论文的 DOI 标识符
  {
    key: 'doi', // 列标识：DOI
    label: 'DOI', // 列标题文本
    width: 140, // 列宽 140px（DOI 通常较长）
    sortable: false, // DOI 不支持排序
    hideable: true, // 允许用户隐藏
    ellipsis: true, // 超出宽度时省略号（DOI 很长）
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.doi) {
        return <span className={styles.textTertiary}>-</span>; // 无 DOI 时显示灰色短横线
      }
      // 使用 Tooltip 显示完整 DOI，单元格内截断
      return (
        <Tooltip title={paper.doi}>
          <span className={styles.doiText}>
            {paper.doi}
          </span>
        </Tooltip>
      ); // 灰色小字显示 DOI
    },
  },
  // ===== 出版商列 =====
  // 显示论文的出版商信息
  {
    key: 'publisher', // 列标识：出版商
    label: i18next.t('paperList:column.publisher'), // 列标题文本
    width: 120, // 列宽 120px
    sortable: false, // 出版商不支持排序
    hideable: true, // 允许用户隐藏
    ellipsis: true, // 超出宽度时省略号
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.publisher) {
        return <span className={styles.textTertiary}>-</span>; // 无出版商时显示灰色短横线
      }
      return (
        <Tooltip title={paper.publisher}>
          <span className={styles.smallText}>
            {paper.publisher}
          </span>
        </Tooltip>
      ); // Tooltip 显示完整名称
    },
  },
  // ===== 开放获取状态列 =====
  // 显示论文的 OA 状态标签（Gold/Green/Bronze/Hybrid/Closed）
  {
    key: 'oaStatus', // 列标识：OA 状态
    label: 'OA', // 列标题文本
    width: 90, // 列宽 90px
    sortable: false, // OA 状态不支持排序
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 标签不会超出宽度
    render: (paper: any) => { // 自定义渲染函数
      const oaStatus = (OA_STATUS_CONFIG as any)[paper.open_access_status]; // 从配置映射表获取 OA 状态配置
      if (!oaStatus) {
        return <span className={styles.textTertiary}>-</span>; // 无 OA 信息时显示灰色短横线
      }
      // 渲染彩色 OA 状态标签
      const badgeClass = oaStatus.color === 'default' ? styles.badgeOaDefault : styles.badge;
      return (
        <span className={badgeClass}>
          {oaStatus.text}
        </span>
      ); // 小字号标签
    },
  },
  // ===== 论文类型列 =====
  // 显示论文类型（如 article, review, conference_paper 等）
  // 支持列头筛选：点击列表头弹出筛选菜单，可按类型多选过滤（客户端筛选，无需请求后端）
  {
    key: 'paperType', // 列标识：论文类型
    label: i18next.t('paperList:column.type'), // 列标题文本
    width: 90, // 列宽 90px
    sortable: true, // 启用排序功能
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 标签不会超出宽度
    filters: FILTERABLE_TYPES.map(t => ({ text: t.label, value: t.value })), // 列头筛选选项：将 FILTERABLE_TYPES 的 {value, label} 格式转为 Ant Design 要求的 {value, text} 格式
    onFilter: (value: any, paper: any) => paper.paper_type === value, // 筛选函数：Ant Design 对每个已选筛选值调用此函数，记录通过任一值匹配即显示（OR 逻辑）
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.paper_type) return <span className={styles.textTertiary}>-</span>;  // 无类型时显示短横线
      const color = getTypeColor(paper.paper_type);  // 获取类型对应的颜色
      const label = getTypeLabel(paper.paper_type);  // 获取类型的中文显示名称
      return (
        <span
          className={styles.badge}
          style={{
            backgroundColor: color === 'default' ? 'var(--border-color)' : `var(--ant-color-${color})`,
            color: color === 'default' ? 'var(--text-secondary)' : 'white',
          }}
        >
          {label}  {/* 显示中文名称而非原始键名 */}
        </span>
      );
    },
  },
  // ===== 影响因子列 =====
  // 显示 JCR 年度影响因子（如 5.2），支持数值排序
  {
    key: 'if', // 列标识：影响因子
    label: 'IF', // 列标题文本（Impact Factor 缩写）
    width: 65, // 列宽 65px（数字如 "5.2" 足够）
    sortable: true, // 支持按影响因子数值排序
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 数字不会超出宽度
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.impact_factor) {
        return <span className={styles.textTertiary}>-</span>; // 无 IF 时显示灰色短横线
      }
      return (
        <span className={`${styles.badgeIf} ${styles.badge}`}>
          {paper.impact_factor}
        </span>
      ); // 绿色标签显示 IF 值
    },
  },
  // ===== 五年影响因子列 =====
  // 显示 JCR 五年平均影响因子
  {
    key: 'if5', // 列标识：五年影响因子
    label: 'IF5', // 列标题文本
    width: 65, // 列宽 65px
    sortable: true, // 支持排序
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 数字不会超出宽度
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.impact_factor_5) {
        return <span className={styles.textTertiary}>-</span>; // 无 IF5 时显示灰色短横线
      }
      return (
        <span className={`${styles.badgeIf5} ${styles.badge}`}>
          {paper.impact_factor_5}
        </span>
      ); // 浅绿色标签
    },
  },
  // ===== JCR 分区列 =====
  // 显示 JCR 四分位分区（Q1/Q2/Q3/Q4），支持排序
  {
    key: 'jcr', // 列标识：JCR 分区（对应 SORT_FIELD_MAP 中的 jcr → jcr_quartile 映射）
    label: 'JCR', // 列标题文本
    width: 70, // 列宽 70px，足够显示 "Q1" 等短文本
    sortable: true, // 支持按 JCR 分区排序（Q1 > Q2 > Q3 > Q4）
    hideable: true, // 允许用户在列设置中隐藏该列
    ellipsis: false, // 短文本不会超出宽度
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.jcr_quartile) {
        return <span className={styles.textTertiary}>-</span>; // 无 JCR 分区时显示灰色短横线
      }
      // 根据分区等级选择标签类名
      const badgeClass =
        paper.jcr_quartile === 'Q1' ? styles.badgeQ1 :
        paper.jcr_quartile === 'Q2' ? styles.badgeQ2 :
        paper.jcr_quartile === 'Q3' ? styles.badgeQ3 :
        styles.badgeQ4;
      return (
        <span className={badgeClass}>
          {paper.jcr_quartile} {/* 显示 JCR 分区值，如 "Q1"、"Q2" */}
        </span>
      );
    },
  },
  // ===== 分区综合列 =====
  // 将原来分散的 JCR、中科院、中科院小类、Top、预警五列合并为一个「分区」列
  // 采用紧凑的彩色标签组形式展示，各标签水平排列、自动换行
  {
    key: 'partition', // 列唯一标识，用于列选择器和 localStorage 持久化
    label: i18next.t('paperList:column.partition'), // 列标题，显示在表头
    width: 200, // 列宽 200px，需要容纳多个标签并排
    sortable: false, // 合并列不支持排序（内含多个维度，无法确定统一排序规则）
    hideable: true, // 允许用户在列设置中隐藏该列
    ellipsis: false, // 不启用文本省略（标签组需要完整显示）
    render: (paper: any) => { // 自定义渲染函数，paper 为当前行的论文数据对象
      const tags = []; // 收集所有分区标签的数组，最后统一渲染

      // ---- JCR 分区 ----
      // 判断论文是否有 JCR 四分位分区数据（Q1/Q2/Q3/Q4）
      if (paper.jcr_quartile) {
        // 根据分区等级选择标签类名
        const badgeClass =
          paper.jcr_quartile === 'Q1' ? styles.badgeQ1 :
          paper.jcr_quartile === 'Q2' ? styles.badgeQ2 :
          paper.jcr_quartile === 'Q3' ? styles.badgeQ3 :
          styles.badgeQ4;
        tags.push(
          <span key="jcr" className={`${badgeClass} ${styles.badge}`}>
            {paper.jcr_quartile}
          </span>
        );
      }

      // ---- 中科院分区 ----
      // 判断论文是否有中科院升级版分区数据（1区/2区/3区/4区）
      if (paper.cas_quartile) {
        // 根据分区等级选择标签类名
        // 使用 includes() 而非 === 匹配，因为 cas_quartile 值可能为 "1区"、"2区TOP" 等复合格式
        const badgeClass =
          paper.cas_quartile.includes('1') ? styles.badgeCas1 :
          paper.cas_quartile.includes('2') ? styles.badgeCas2 :
          paper.cas_quartile.includes('3') ? styles.badgeCas3 :
          styles.badgeCas4;
        tags.push(
          <span key="cas" className={`${badgeClass} ${styles.badge}`}>
            {paper.cas_quartile}
          </span>
        );
      }

      // ---- 中科院小类分区 ----
      // 判断论文是否有中科院升级版小类分区数据
      // 小类分区是细分学科方向的分区信息，如 "计算机科学 1区"、"数学 2区" 等
      if (paper.cas_small) {
        tags.push(
          // 使用 Tooltip 包裹，鼠标悬停时显示完整的小类分区文本
          // 因为小类名称可能较长（如 "计算机科学 1区"），列宽不足以完整显示时可通过 Tooltip 查看
          <Tooltip key="casSmall" title={paper.cas_small}>
            <span className={`${styles.badgeCasSmall} ${styles.badge}`}>
              {paper.cas_small}
            </span>
          </Tooltip>
        );
      }

      // ---- Top 期刊标识 ----
      // 判断论文所在期刊是否为中科院 Top 期刊（顶级期刊标识）
      if (paper.cas_top) {
        tags.push(
          <span key="top" className={`${styles.badgeTop} ${styles.badge}`}>
            Top
          </span>
        );
      }

      // ---- 中科院预警标识 ----
      // 判断论文所在期刊是否被中科院列入预警名单（学术风险提示）
      if (paper.cas_warning) {
        tags.push(
          <span key="warning" className={`${styles.badgeWarning} ${styles.badge}`}>
            {i18next.t('paperList:column.warning')}{paper.cas_warning}
          </span>
        );
      }

      // 如果该论文没有任何分区数据，显示灰色短横线占位符
      if (tags.length === 0) {
        return <span className={styles.textTertiary}>-</span>; // 使用 CSS 变量保持与全局主题一致
      }

      // 将所有标签渲染到一个 flex 容器中
      return (
        <div className={styles.flexContainer}>
          {tags}
        </div>
      );
    },
  },
  // ===== SSCI 分区列 =====
  // 显示 SSCI 期刊的 JCR 四分位分区
  {
    key: 'ssci', // 列标识：SSCI 分区
    label: 'SSCI', // 列标题文本
    width: 70, // 列宽 70px
    sortable: false, // SSCI 分区不支持排序
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 短文本不会超出宽度
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.ssci_quartile) {
        return <span className={styles.textTertiary}>-</span>; // 无 SSCI 分区时显示灰色短横线
      }
      // 根据分区等级选择标签类名
      const badgeClass =
        paper.ssci_quartile === 'Q1' ? styles.badgeSsciQ1 :
        paper.ssci_quartile === 'Q2' ? styles.badgeSsciQ2 :
        paper.ssci_quartile === 'Q3' ? styles.badgeSsciQ3 :
        styles.badgeQ4;
      return (
        <span className={`${badgeClass} ${styles.badge}`}>
          SSCI {paper.ssci_quartile}
        </span>
      ); // 渲染彩色标签
    },
  },
  // ===== 解析状态列（已移除） =====
  // 原先在表格中有一列"解析状态"，显示等待解析/解析中（含进度条）/已就绪（引擎标签）/失败
  // 但解析状态本质上只与主 PDF 有关，放在展开行的附件列表中更合理
  // 现在附件列表的主 PDF 行已直接显示引擎名（MinerU），一个标签同时传达状态和方式
  // 因此表格中的"解析状态"列已不再需要，在此处彻底移除
  // ===== 文件名列 =====
  // 显示原始 PDF 文件名
  {
    key: 'filename', // 列标识：文件名
    label: i18next.t('paperList:column.filename'), // 列标题文本
    width: 150, // 列宽 150px
    sortable: false, // 文件名不支持排序
    hideable: true, // 允许用户隐藏
    ellipsis: true, // 文件名可能很长，超出时省略号
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.filename) {
        return <span className={styles.textTertiary}>-</span>; // 无文件名时显示灰色短横线
      }
      return (
        <Tooltip title={paper.filename}>
          <span className={`${styles.smallText} ${styles.textTertiary}`}>
            {paper.filename}
          </span>
        </Tooltip>
      ); // Tooltip 显示完整文件名
    },
  },
  // ===== 字符数列 =====
  // 显示 Markdown 内容的字符数（如 "12.5k"），用于评估论文内容量
  {
    key: 'charCount', // 列标识：字符数
    label: i18next.t('paperList:column.charCount'), // 列标题文本
    width: 70, // 列宽 70px
    sortable: true, // 支持按字符数排序
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 数字不会超出宽度
    render: (paper: any) => { // 自定义渲染函数
      if (!paper.markdown_length || paper.markdown_length <= 0) {
        return <span className={styles.textTertiary}>-</span>; // 无内容时显示灰色短横线
      }
      // 将字符数转换为 "X.Xk" 格式显示
      return (
        <span className={styles.smallText}>
          {(paper.markdown_length / 1000).toFixed(1)}k
        </span>
      ); // 除以 1000 并保留 1 位小数
    },
  },
  // ===== 添加时间列 =====
  // 显示论文上传/添加时间，支持按时间排序
  {
    key: 'added', // 列标识：添加时间
    label: i18next.t('paperList:column.added'), // 列标题文本
    width: 120, // 列宽 120px
    sortable: true, // 支持按时间排序
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 日期不会超出宽度
    render: (paper: any) => {
      const formatted = formatDate(paper.created_at);
      return <span>{formatted || paper.created_at}</span>;
    },
  },
  // ===== 更新时间列 =====
  // 显示论文最后更新时间
  {
    key: 'updated', // 列标识：更新时间
    label: i18next.t('paperList:column.updated'), // 列标题文本
    width: 120, // 列宽 120px
    sortable: true, // 支持按时间排序
    hideable: true, // 允许用户隐藏
    ellipsis: false, // 日期不会超出宽度
    render: (paper: any) => {
      const formatted = formatDate(paper.updated_at);
      return <span>{formatted || paper.updated_at}</span>;
    },
  },
];

/**
 * 默认可见列配置
 *
 * 首次使用时（localStorage 无记录）显示这些列：
 * - title: 标题（核心信息，始终显示）
 * - authors: 作者（重要学术信息）
 * - journal: 期刊（重要学术信息）
 * - year: 年份（筛选和排序常用）
 * - if: 影响因子（学术评估常用）
 * - partition: 分区（JCR、中科院、Top 等综合分区信息）
 * - added: 添加时间（管理论文库）
 *
 * 注意：parseStatus（解析状态）和 filename（文件名）已移到展开行的附件列表中显示，
 * 不再作为默认可见列。用户可通过右键表头重新启用这些列。
 */
export const DEFAULT_VISIBLE_COLUMNS = [
  'title',
  'authors',
  'journal',
  'year',
  'if',
  'partition',
  'added',
];

/**
 * 表格列 key 到后端排序字段名的映射
 *
 * 为什么需要映射？
 * - 表格列使用简短的英文 key（如 'year', 'if'）
 * - 后端排序接口使用 camelCase 字段名（如 'publicationYear', 'impactFactor'）
 * - 排序时需要将前端列 key 转换为后端可识别的字段名
 *
 * 未在映射表中的列 key 将直接传递给后端（如 'title', 'createdAt'）
 */
export const SORT_FIELD_MAP = {
  year: 'publicationYear',        // 发表年份 → 后端字段 publicationYear
  citations: 'citationCount',      // 被引次数 → 后端字段 citationCount
  if: 'impactFactor',              // 影响因子 → 后端字段 impactFactor
  if5: 'impactFactor5',            // 五年影响因子 → 后端字段 impactFactor5
  jcr: 'jcrQuartile',              // JCR 分区 → 后端字段 jcrQuartile
  added: 'createdAt',               // 添加时间 → 后端字段 createdAt
  updated: 'updatedAt',             // 更新时间 → 后端字段 updatedAt
  charCount: 'markdownLength',     // 字符数 → 后端字段 markdownLength
  paperType: 'paperType',          // 按文献类型排序
};

/**
 * 格式化 Unix 时间戳为中文日期字符串
 *
 * 为什么使用 Unix 时间戳而不是 ISO 字符串？
 * - 后端存储的是 Unix 时间戳（整数），节省存储空间
 * - 前端根据用户语言环境格式化显示
 * - 避免时区转换问题
 *
 * @param {number} timestamp - Unix 时间戳（秒）
 * @returns {string} 格式化后的日期，如 "4月11日 14:30"
 */
export function formatDate(timestamp: any) { // 日期格式化工具函数，供表格列和卡片视图共用
  // 防御性检查：如果时间戳为 null/undefined/0，返回空字符串
  if (!timestamp) return ''; // 时间戳为空时返回空字符串，避免显示 "Invalid Date"
  // autonomics 的 created_at/updated_at 是 RFC3339 字符串；jayread 旧路径是 Unix 秒数字。
  // 小数秒截到毫秒：后端可能回吐纳秒精度（9 位），WebKit 的 Date 解析器会拒收
  const normalized = typeof timestamp === 'string' ? timestamp.replace(/(\.\d{3})\d+/, '$1') : timestamp;
  const d = typeof normalized === 'number' ? new Date(normalized * 1000) : new Date(normalized);
  if (Number.isNaN(d.getTime())) return ''; // 解析失败回落空串，不再显示 Invalid Date
  // 使用中文格式：月/日/时:分，不显示年份节省空间
  return d.toLocaleDateString(i18next.language === 'zh' ? 'zh-CN' : 'en-US', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }); // 使用 toLocaleDateString 按 i18n 语言格式化
}
