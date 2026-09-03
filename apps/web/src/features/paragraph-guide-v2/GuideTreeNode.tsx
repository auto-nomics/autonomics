/**
 * JayRead V2 层次化导览树节点组件
 *
 * 单个导览树节点的渲染组件，支持：
 * 1. 节点展开/折叠控制
 * 2. 星级标注重要程度
 * 3. 角色图标标注逻辑角色
 * 4. 点击节点触发坐标导航回调
 * 5. 节点样式根据层级和类型自动调整
 *
 * @module GuideTreeNode
 */

import React, { useState, useRef, useEffect } from 'react'; // 导入 React 核心库
import PropTypes from 'prop-types'; // 导入 PropTypes 类型检查库
import { Tooltip, Collapse } from 'antd'; // 导入 Ant Design 组件
import styles from './GuideTreeNode.module.css'; // 导入样式模块
import {
  FileTextOutlined, // 文档图标（默认）
  BookOutlined, // 书籍图标（根节点）
  StarFilled, // 星星图标（重要程度）
  StarOutlined, // 空心星星图标
  CaretRightOutlined, // 右箭头（折叠状态）
  CaretDownOutlined, // 下箭头（展开状态）
  InfoCircleOutlined, // 信息图标
  QuestionCircleOutlined, // 问题图标（批判性思考）
  ForwardOutlined, // 跳过图标
  ThunderboltOutlined, // 锚点图标
} from '@ant-design/icons'; // 导入 Ant Design 图标

const { Panel } = Collapse; // 折叠面板组件

/**
 * V3 结构化导览：逻辑角色图标映射表
 *
 * 根据节点的 logical_role 字段显示对应的图标。
 * V3 导览主要使用 heading 和 keypoint 两种角色。
 * 保留旧版角色（root/section/subsection/paragraph）以兼容老数据。
 */
const ROLE_ICONS = {
  // V3 结构化导览角色
  heading: <BookOutlined />, // heading 节点：书籍图标（主要章节）
  keypoint: null, // keypoint 节点：不显示图标（使用圆点标记）
  // 兼容 V2 老角色
  root: <BookOutlined />, // 根节点：书籍图标
  section: <FileTextOutlined />, // 章节：文档图标
  subsection: <FileTextOutlined />, // 小节：文档图标
  paragraph: <FileTextOutlined />, // 段落：文档图标
  default: <FileTextOutlined />, // 默认：文档图标
};

/**
 * 重要程度星级颜色映射表
 *
 * 根据节点的重要程度（1-5）显示不同颜色的星星。
 */
const IMPORTANCE_COLORS = {
  1: 'var(--importance-1)',
  2: 'var(--importance-2)',
  3: 'var(--importance-3)',
  4: 'var(--importance-4)',
  5: 'var(--importance-5)',
};

/**
 * 导览树节点组件
 *
 * @param {object} props - 组件属性
 * @param {object} props.node - 节点数据对象
 * @param {string} props.node.id - 节点唯一标识
 * @param {string} props.node.title - 节点标题
 * @param {string} [props.node.summary] - 节点摘要（可选）
 * @param {number} [props.node.pageIdx] - 节点所在页码索引（可选）
 * @param {Array<object>} [props.node.children] - 子节点数组（可选）
 * @param {string} [props.node.logicalRole] - 逻辑角色（可选）
 * @param {number} [props.node.importance] - 重要程度（1-5，可选）
 * @param {Array<Array<number>>} [props.node.bboxes] - 边界框数组（可选）
 * @param {number} props.depth - 节点深度（用于缩进）
 * @param {Function} props.onNodeClick - 节点点击回调：(node) => void
 * @param {Function} props.onToggle - 展开/折叠切换回调：(nodeId) => void
 * @param {boolean} props.isExpanded - 节点是否展开
 * @param {Function} [props.onRegisterElement] - 注册 DOM 元素回调：(nodeId, element) => void
 */
function GuideTreeNode({
  node,
  depth,
  onNodeClick,
  onToggle,
  isExpanded,
  onRegisterElement, // 新增：注册 DOM 元素回调
}: {
  node: any;
  depth: any;
  onNodeClick: any;
  onToggle: any;
  isExpanded: any;
  onRegisterElement: any;
}) {
  // ========== 状态管理 ==========

  /**
   * 鼠标悬停状态
   *
   * 用于控制悬停时的高亮效果。
   */
  const [isHovered, setIsHovered] = useState(false); // 状态：鼠标是否悬停

  /**
   * 阅读引导展开状态
   *
   * V4 导览阅读体验：控制阅读引导详情面板的展开/折叠。
   */
  const [guideExpanded, setGuideExpanded] = useState(false); // 状态：阅读引导是否展开

  // ========== 数据解构 ==========

  /**
   * 解构节点数据，设置默认值
   */
  const {
    id, // 节点唯一标识（必需）
    title, // 节点标题（必需）
    summary = '', // 节点摘要（默认空字符串）
    pageIdx = null, // 页码索引（默认 null）
    children = [], // 子节点数组（默认空数组）
    logicalRole = 'default', // 逻辑角色（V3: heading/keypoint，V2: root/section/paragraph）
    importance = 0, // 重要程度（默认 0，表示未设置）
    bboxes = [], // 边界框数组（默认空数组）
    imageUrl = null, // 关联图片 URL（默认 null，可能为 null）
    // V3 结构化导览新增属性
    originalTitle = null, // heading 节点：原始英文标题（用于 Tooltip）
    originalText = null, // keypoint 节点：英文原文摘录
    translationZh = null, // keypoint 节点：中文翻译（用于 Tooltip）
    sourceBlockId = null, // keypoint 节点：来源文本块 ID（用于后续悬浮高亮）
    // V4 导览阅读体验新增属性
    sectionRole = null, // 章节角色描述（自然语言）
    readingGuide = null, // 阅读引导 JSON 字符串
    attentionAnchors = null, // 注意力锚点 JSON 字符串
    criticalHints = null, // 批判性思考提示 JSON 字符串
    skipHint = null, // 可跳过提示
    argumentContext = null, // 在论证链中的位置（一句话）
    whyExists = null, // 这个章节为什么存在
  } = node; // 解构节点数据

  // ========== V4 导览阅读体验：解析 JSON 字段 ==========

  /**
   * 解析阅读引导数据
   *
   * readingGuide 字段是 JSON 字符串，需要解析为对象。
   */
  let parsedReadingGuide = null;
  try {
    if (readingGuide) {
      parsedReadingGuide = typeof readingGuide === 'string'
        ? JSON.parse(readingGuide)
        : readingGuide;
    }
  } catch {
    parsedReadingGuide = null;
  }

  /**
   * 解析注意力锚点数据
   *
   * attentionAnchors 字段是 JSON 字符串，需要解析为数组。
   */
  let parsedAnchors = [];
  try {
    if (attentionAnchors) {
      parsedAnchors = typeof attentionAnchors === 'string'
        ? JSON.parse(attentionAnchors)
        : attentionAnchors || [];
    }
  } catch {
    parsedAnchors = [];
  }

  /**
   * 解析批判性思考提示数据
   *
   * criticalHints 字段是 JSON 字符串，需要解析为数组。
   */
  let parsedCriticalHints = [];
  try {
    if (criticalHints) {
      parsedCriticalHints = typeof criticalHints === 'string'
        ? JSON.parse(criticalHints)
        : criticalHints || [];
    }
  } catch {
    parsedCriticalHints = [];
  }

  /**
   * 节点 DOM 元素引用
   *
   * 用于向父组件注册当前节点的 DOM 元素，
   * 支持反向导航时快速定位节点。
   */
  const nodeElementRef = useRef(null); // ref：指向当前节点的 DOM 元素

  // ========== V3 导览：判断节点角色 ==========
  // isHeading: 是否为标题节点（可展开/折叠，有子节点）
  // isKeypoint: 是否为要点节点（叶子节点，圆点标记）
  const isHeading = logicalRole === 'heading'; // V3 heading 节点
  const isKeypoint = logicalRole === 'keypoint'; // V3 keypoint 节点

  // ========== V4 导览阅读体验：判断是否有阅读引导数据 ==========
  const hasReadingGuide = parsedReadingGuide
    || parsedAnchors.length > 0
    || parsedCriticalHints.length > 0
    || skipHint
    || argumentContext
    || sectionRole
    || whyExists;

  /**
   * 注册/注销 DOM 元素
   *
   * 当节点挂载或卸载时，通知父组件注册或注销当前节点的 DOM 元素。
   */
  useEffect(() => {
    if (onRegisterElement) {
      onRegisterElement(id, nodeElementRef.current);
    }
    return () => {
      if (onRegisterElement) {
        onRegisterElement(id, null);
      }
    };
  }, [id, onRegisterElement]); // 依赖于节点 ID 和注册回调

  // ========== 计算属性 ==========

  /**
   * 是否有子节点
   *
   * 用于判断是否显示展开/折叠按钮。
   */
  const hasChildren = children && children.length > 0; // 是否有子节点

  /**
   * 获取逻辑角色图标
   *
   * 根据 logicalRole 字段从映射表中获取对应图标，
   * 未找到时使用默认图标。
   */
  const roleIcon = (ROLE_ICONS as any)[logicalRole] || ROLE_ICONS.default; // 角色图标

  /**
   * 获取重要程度星星数组
   *
   * 根据 importance 字段生成对应数量的星星。
   * 如果 importance 为 0，不显示星星。
   */
  const stars = importance > 0
    ? Array.from({ length: importance }, (_, i) => (
        <StarFilled
          key={i}
          className={styles.starIcon}
          style={{ color: (IMPORTANCE_COLORS as any)[importance] }}
        />
      ))
    : null; // 无重要程度时不显示星星

  /**
   * 计算节点缩进
   *
   * 根据节点深度计算左侧内边距，每层缩进 16px。
   */
  const indentStyle = {
    paddingLeft: `${depth * 16}px`, // 每层缩进 16px
  };

  // ========== 事件处理函数 ==========

  /**
   * 处理节点点击事件
   *
   * 触发导航回调，传递节点数据。
   * 如果有子节点且未展开，同时展开节点。
   */
  const handleClick = () => {
    // 触发导航回调
    onNodeClick?.(node);

    // 如果有子节点且未展开，自动展开
    if (hasChildren && !isExpanded) {
      onToggle?.(id);
    }
  };

  /**
   * 处理展开/折叠按钮点击事件
   *
   * 阻止事件冒泡，避免触发节点点击。
   */
  const handleToggleClick = (e: any) => {
    e.stopPropagation(); // 阻止事件冒泡
    onToggle?.(id); // 触发展开/折叠切换
  };

  // ========== 渲染 ==========

  return (
    <div>
      {/**
       * 节点内容行
       *
       * V3 结构化导览：根据 logicalRole 区分 heading 和 keypoint 的渲染样式
       * - heading 节点：可展开/折叠，较大字体，显示角色图标
       * - keypoint 节点：叶子节点，圆点标记，较小字体，次要颜色
       *
       * V4 导览阅读体验：heading 节点增加阅读引导显示
       */}
      <div
        ref={nodeElementRef}
        className={styles.nodeContainer}
        style={{ paddingLeft: `${depth * 16}px` }}
        onClick={handleClick}
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
      >
        {/**
         * 节点主行：标题 + 图标 + 展开按钮
         */}
        <div className={isKeypoint ? styles.nodeMainRowKeypoint : styles.nodeMainRow}>
        {/**
         * V3 结构化导览：keypoint 节点的圆点标记
         *
         * keypoint 节点使用圆点（•）代替角色图标，作为视觉标记。
         * 圆点使用更小的字体和淡色，不干扰主要内容。
         */}
        {isKeypoint && (
          <span className={styles.keypointBullet}>
            •
          </span>
        )}

        {/**
         * 展开/折叠按钮
         *
         * V3 结构化导览：仅 heading 节点且有子节点时显示展开/折叠按钮。
         * keypoint 节点是叶子节点，不显示展开/折叠按钮。
         */}
        {!isKeypoint && hasChildren && (
          <span onClick={handleToggleClick} className={styles.toggleButton}>
            {isExpanded ? <CaretDownOutlined /> : <CaretRightOutlined />}
          </span>
        )}

        {/**
         * V3 结构化导览：heading 节点的角色图标
         *
         * 仅 heading 节点显示角色图标（BookOutlined/FileTextOutlined）。
         * keypoint 节点不显示图标（已用圆点标记）。
         */}
        {!isKeypoint && roleIcon && (
          <span className={styles.roleIcon}>
            {roleIcon}
          </span>
        )}

        {/**
         * V3 结构化导览：节点标题
         *
         * heading 节点：较大字体（13px），主色显示，Tooltip 显示原始英文标题
         * keypoint 节点：较小字体（12px），次要色显示，Tooltip 显示中文翻译
         */}
        <Tooltip
          title={isKeypoint ? (translationZh || title) : (originalTitle || title)}
          placement="top"
        >
          <span className={`${styles.nodeTitle} ${isKeypoint ? styles.nodeTitleKeypoint : styles.nodeTitleHeading}`}>
            {title}
          </span>
        </Tooltip>

        </div>

        {/**
         * V4 导览阅读体验：heading 节点的阅读引导显示
         *
         * 显示：
         * - 章节角色描述
         * - 论证位置（argument_context）
         * - 注意力锚点标签（最多 3 个）
         */}
        {isHeading && hasReadingGuide && (
          <div className={styles.readingGuideContainer}>
            {/**
             * 论证位置
             *
             * 使用 📍 前缀，次要颜色
             */}
            {argumentContext && (
              <div className={styles.argumentContext}>
                <span className={styles.argumentContextIcon}>📍</span>
                {typeof argumentContext === 'string' ? (
                  <span className={styles.argumentContextText}>
                    {argumentContext}
                  </span>
                ) : (
                  <div style={{ flex: 1 }}>
                    {argumentContext.role && (
                      <span className={styles.argumentContextRole}>
                        {argumentContext.role}
                      </span>
                    )}
                    <span className={styles.argumentContextText}>
                      {argumentContext.description || argumentContext.claim || ''}
                    </span>
                    {(argumentContext.depends_on?.length > 0 || argumentContext.depended_by?.length > 0) && (
                      <div className={styles.argumentContextDeps}>
                        {argumentContext.depends_on?.length > 0 && (
                          <span>← 依赖: {Array.isArray(argumentContext.depends_on) ? argumentContext.depends_on.join(', ') : argumentContext.depends_on}</span>
                        )}
                        {argumentContext.depended_by?.length > 0 && (
                          <span> → 被依赖: {Array.isArray(argumentContext.depended_by) ? argumentContext.depended_by.join(', ') : argumentContext.depended_by}</span>
                        )}
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}

            {/**
             * 注意力锚点标签
             *
             * 使用 ⚡ 前缀，小标签样式，最多显示 3 个
             */}
            {parsedAnchors.length > 0 && (
              <div className={`${styles.anchorTags} ${guideExpanded ? styles.dividerSmallMargin : styles.dividerNoMargin}`}>
                {parsedAnchors.slice(0, guideExpanded ? undefined : 3).map((anchor: any, index: any) => (
                  <Tooltip
                    key={index}
                    title={
                      <div style={{ fontSize: 11 }}>
                        <div style={{ fontWeight: 500, marginBottom: 2 }}>
                          {anchor.guidance || '查看详情'}
                        </div>
                        {anchor.connects_to && (
                          <div style={{ opacity: 0.8 }}>
                            → {anchor.connects_to}
                          </div>
                        )}
                        {anchor.reference_count > 0 && (
                          <div style={{ opacity: 0.6, marginTop: 2 }}>
                            被引用 {anchor.reference_count} 次
                          </div>
                        )}
                      </div>
                    }
                  >
                    <span className={styles.anchorTag}>
                      <ThunderboltOutlined className={styles.anchorTagIcon} />
                      {anchor.location_hint || `锚点 ${index + 1}`}
                      {anchor.reference_count > 0 && (
                        <span className={styles.anchorTagCount}>
                          {anchor.reference_count}
                        </span>
                      )}
                    </span>
                  </Tooltip>
                ))}
                {!guideExpanded && parsedAnchors.length > 3 && (
                  <span className={styles.anchorMore}>
                    +{parsedAnchors.length - 3} 个锚点
                  </span>
                )}
              </div>
            )}

            {/**
             * 可跳过提示
             *
             * 使用 ⏭ 图标前缀
             */}
            {skipHint && (
              <div className={styles.skipHint}>
                <ForwardOutlined className={styles.skipHintIcon} />
                <span>可跳过：{skipHint}</span>
              </div>
            )}
          </div>
        )}

        {/**
         * V4 导览阅读体验：阅读引导详情面板（可折叠）
         *
         * 点击展开后显示完整的阅读引导信息
         */}
        {isHeading && guideExpanded && hasReadingGuide && (
          <div className={styles.guideDetailPanel}>
            {/**
             * 章节角色描述
             */}
            {sectionRole && (
              <div className={styles.sectionInfo}>
                <span className={styles.sectionInfoLabel}>
                  章节：
                </span>
                <span className={styles.sectionInfoText}>
                  {typeof sectionRole === 'string'
                    ? sectionRole
                    : (sectionRole.description || sectionRole.role || JSON.stringify(sectionRole))}
                </span>
              </div>
            )}

            {/**
             * 章节存在原因（whyExists）
             *
             * 告诉读者这个章节为什么存在，在论证中的作用。
             */}
            {whyExists && (
              <div className={styles.whyExistsSection}>
                <div className={styles.whyExistsHeader}>
                  <span className={styles.whyExistsIcon}>💡</span>
                  <span className={styles.whyExistsLabel}>为什么有这一节</span>
                </div>
                <div className={styles.whyExistsContent}>
                  {whyExists}
                </div>
              </div>
            )}

            {/**
             * 阅读策略建议
             */}
            {parsedReadingGuide?.reading_strategy && (
              <div className={styles.readingStrategySection}>
                <div className={styles.readingStrategyHeader}>
                  <InfoCircleOutlined className={styles.readingStrategyIcon} />
                  <span className={styles.readingStrategyLabel}>阅读策略</span>
                </div>
                <div className={styles.readingStrategyContent}>
                  {parsedReadingGuide.reading_strategy}
                </div>
              </div>
            )}

            {/**
             * 注意力锚点详情
             */}
            {parsedAnchors.length > 0 && (
              <div className={styles.anchorsDetailSection}>
                <div className={styles.anchorsDetailLabel}>
                  注意力锚点
                </div>
                {parsedAnchors.map((anchor: any, index: any) => (
                  <div key={index} className={styles.anchorDetailItem}>
                    <div className={styles.anchorDetailHeader}>
                      <ThunderboltOutlined className={styles.anchorDetailIcon} />
                      {anchor.location_hint || `锚点 ${index + 1}`}
                      {anchor.reference_count > 0 && (
                        <span className={styles.anchorDetailCountBadge}>
                          {anchor.reference_count} 次引用
                        </span>
                      )}
                    </div>
                    <div className={styles.anchorDetailGuidance}>
                      {anchor.guidance}
                    </div>
                    {anchor.connects_to && (
                      <div className={styles.anchorDetailConnects}>
                        → {anchor.connects_to}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}

            {/**
             * 批判性思考提示
             */}
            {parsedCriticalHints.length > 0 && (
              <div className={styles.criticalHintsSection}>
                <div className={styles.criticalHintsLabel}>
                  批判性思考
                </div>
                {parsedCriticalHints.map((hint: any, index: any) => (
                  <div key={index} className={styles.criticalHintItem}>
                    <QuestionCircleOutlined className={styles.criticalHintIcon} />
                    <span>{hint}</span>
                  </div>
                ))}
              </div>
            )}

            {/**
             * 点击折叠按钮
             */}
            <div
              onClick={(e) => {
                e.stopPropagation();
                setGuideExpanded(false);
              }}
              className={styles.collapseButton}
            >
              收起引导 ▲
            </div>
          </div>
        )}

        {/**
         * V4 导览阅读体验：展开阅读引导按钮（仅在有引导数据且未展开时显示）
         */}
        {isHeading && hasReadingGuide && !guideExpanded && (
          <div
            onClick={(e) => {
              e.stopPropagation();
              setGuideExpanded(true);
            }}
            className={styles.expandButton}
          >
            展开阅读引导 ↓
          </div>
        )}

      </div>

      {/**
       * 子节点列表
       *
       * 仅在节点展开且有子节点时渲染。
       * 递归渲染 GuideTreeNode 组件。
       */}
      {isExpanded && hasChildren && (
        <div>
          {children.map((child: any) => (
            <GuideTreeNode
              key={child.id} // React key，使用节点 ID
              node={child} // 子节点数据
              depth={depth + 1} // 深度加一
              onNodeClick={onNodeClick} // 导航回调
              onToggle={onToggle} // 展开/折叠回调
              isExpanded={child.expanded || false} // 子节点展开状态（从节点数据中读取）
              onRegisterElement={onRegisterElement} // 传递 DOM 元素注册回调
            />
          ))}
        </div>
      )}

    </div>
  );
}

// ========== PropTypes 类型检查 ==========

/**
 * 组件属性类型检查
 *
 * V3 结构化导览：新增 originalTitle、originalText、translationZh、sourceBlockId 属性
 * V4 导览阅读体验：新增 sectionRole、readingGuide、attentionAnchors、criticalHints、skipHint、argumentContext、whyExists 属性
 */
GuideTreeNode.propTypes = {
  /**
   * 节点数据对象
   */
  node: PropTypes.shape({
    id: PropTypes.string.isRequired, // 节点唯一标识（必需）
    title: PropTypes.string.isRequired, // 节点标题（必需）
    summary: PropTypes.string, // 节点摘要（可选）
    pageIdx: PropTypes.number, // 页码索引（可选）
    children: PropTypes.array, // 子节点数组（可选）
    logicalRole: PropTypes.string, // 逻辑角色（V3: heading/keypoint，V2: root/section/paragraph）
    importance: PropTypes.number, // 重要程度（可选）
    bboxes: PropTypes.array, // 边界框数组（可选）
    imageUrl: PropTypes.string, // 关联图片 URL（可选）
    // V3 结构化导览新增属性
    originalTitle: PropTypes.string, // heading 节点：原始英文标题（用于 Tooltip）
    originalText: PropTypes.string, // keypoint 节点：英文原文摘录
    translationZh: PropTypes.string, // keypoint 节点：中文翻译（用于 Tooltip）
    sourceBlockId: PropTypes.string, // keypoint 节点：来源文本块 ID
    // V4 导览阅读体验新增属性
    sectionRole: PropTypes.string, // 章节角色描述（自然语言）
    readingGuide: PropTypes.string, // 阅读引导 JSON 字符串
    attentionAnchors: PropTypes.string, // 注意力锚点 JSON 字符串
    criticalHints: PropTypes.string, // 批判性思考提示 JSON 字符串
    skipHint: PropTypes.string, // 可跳过提示
    argumentContext: PropTypes.string, // 在论证链中的位置（一句话）
    whyExists: PropTypes.string, // 这个章节为什么存在
  }).isRequired,

  /**
   * 节点深度
   */
  depth: PropTypes.number.isRequired, // 深度（必需）

  /**
   * 节点点击回调
   */
  onNodeClick: PropTypes.func, // 点击回调（可选）

  /**
   * 展开/折叠切换回调
   */
  onToggle: PropTypes.func, // 切换回调（可选）

  /**
   * 节点是否展开
   */
  isExpanded: PropTypes.bool, // 展开状态（可选）

  /**
   * 注册 DOM 元素回调（用于反向导航）
   */
  onRegisterElement: PropTypes.func, // 注册回调（可选）
};

/**
 * 默认导出：GuideTreeNode 组件
 */
export default React.memo(GuideTreeNode);
