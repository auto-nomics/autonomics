/**
 * JayRead V2 段落导览入口组件
 *
 * 集成 useGuideStream hook 和 GuideTree 组件，
 * 实现基于 MinerU Precision Extract API 的层次化导览。
 *
 * 核心功能：
 * 1. SSE 消费：连接后端 SSE 端点，实时接收导览生成进度
 * 2. 完整导览渲染：guide_ready 事件后渲染完整导览树
 * 3. Phase B 局部更新：section_refined 事件后局部更新节点
 * 4. 生成触发：提供按钮触发导览生成
 * 5. 进度显示：显示生成进度（阶段、百分比、剩余时间）
 *
 * 与 V1 的主要区别：
 * - V1：轮询获取导览数据，扁平化段落列表
 * - V2：SSE 流式接收，层次化树形结构，坐标导航
 *
 * @module ParagraphGuideV2
 */

import React, { useState, useCallback, useEffect, forwardRef, useImperativeHandle, useRef } from 'react'; // 导入 React 核心钩子
import PropTypes from 'prop-types'; // 导入 PropTypes 类型检查库
import { Button, App, Progress, Alert } from 'antd'; // 导入 Ant Design 组件
import {
  ReloadOutlined, // 刷新图标（重新生成/右键菜单）
} from '@ant-design/icons'; // 导入 Ant Design 图标
import useGuideStream from '../../hooks/useGuideStream'; // 导入 SSE 消费 Hook
import GuideTree from './GuideTree'; // 导入导览树组件
import CoreOverview from './CoreOverview'; // 导入核心概览组件
import { generateGuideV2, getGuideTree } from '../../services/guideApi'; // 导入 V2 导览 API

/**
 * 将后端 guide tree API 返回的字段映射为前端组件期望的格式
 *
 * 后端返回数据库原始列名（role, pageRange, primaryBbox），
 * 前端使用语义化名称（logicalRole, pageIdx, bboxes）。
 * 此函数统一转换，确保前后端数据契约一致。
 *
 * @param {object} node - 后端返回的节点对象
 * @returns {object} 前端期望的节点对象
 */
const mapNode = (node: any): any => {
  // ========== V3 结构化导览：解析 tags JSON 字段 ==========
  // tags 字段存储 JSON 字符串，包含 heading/keypoint 节点的额外信息
  // heading: {original_title, heading_level}
  // keypoint: {original_text, translation_zh, source_block_id}
  let parsedTags: any = {};
  try {
    if (node.tags) {
      // tags 可能是 JSON 字符串，需要解析
      parsedTags = typeof node.tags === 'string' ? JSON.parse(node.tags) : node.tags;
    }
  } catch {
    // tags 格式异常时使用空对象，不影响其他功能
    parsedTags = {};
  }

  // ========== 解析页码信息 ==========
  // pageRange 是 JSON 字符串，如 '{"start_page_idx": 1, "end_page_idx": 3}'
  // 前端只需要起始页码用于页面跳转
  let pageIdx = null;
  try {
    if (node.pageRange) {
      const range = JSON.parse(node.pageRange);
      pageIdx = range?.start_page_idx ?? null;
    }
  } catch {
    // pageRange 格式异常时保持 null，不影响其他功能
  }

  // ========== 解析边界框信息 ==========
  // primaryBbox 是单个 bbox 的 JSON 字符串，前端期望数组格式
  let bboxes: any[] = [];
  try {
    if (node.primaryBbox) {
      bboxes = [JSON.parse(node.primaryBbox)];
    }
  } catch {
    // primaryBbox 格式异常时保持空数组
  }

  // ========== V3 映射：根据 role 类型提取不同属性 ==========
  const role = node.role || 'default'; // 后端角色：heading 或 keypoint
  let mappedNode = {
    id: node.id,
    title: node.title,
    summary: node.summary || '',
    pageIdx,
    logicalRole: role, // V3: 直接使用 role 作为 logicalRole（heading/keypoint）
    importance: node.importance || 0,
    bboxes,
    imageUrl: node.imageUrl || null,
    children: (node.children || []).map(mapNode),
    // V4 导览阅读体验重新设计：新增字段映射
    sectionRole: node.sectionRole || null, // 章节角色描述（自然语言）
    readingGuide: node.readingGuide || null, // 阅读引导 JSON 字符串，需解析
    attentionAnchors: node.attentionAnchors || null, // 注意力锚点 JSON 字符串，需解析
    criticalHints: node.criticalHints || null, // 批判性思考提示 JSON 字符串，需解析
    skipHint: node.skipHint || null, // 可跳过提示
    argumentContext: node.argumentContext || null, // 在论证链中的位置（一句话）
    whyExists: node.whyExists || null, // 这个章节为什么存在
  };

  // V3 结构化导览：heading 节点添加原始英文标题
  if (role === 'heading' && parsedTags.original_title) {
    (mappedNode as any).originalTitle = parsedTags.original_title; // 原始英文标题，用于 Tooltip 显示
  }

  // V3 结构化导览：keypoint 节点添加原文摘录和中文翻译
  if (role === 'keypoint') {
    // originalText: 英文原文摘录（优先从 tags.original_text，回退到 summary 字段）
    (mappedNode as any).originalText = parsedTags.original_text || node.summary || '';
    // translationZh: 中文翻译（从 tags.translation_zh 提取）
    (mappedNode as any).translationZh = parsedTags.translation_zh || '';
    // sourceBlockId: 来源文本块 ID（用于后续悬浮高亮定位）
    (mappedNode as any).sourceBlockId = parsedTags.source_block_id || null;
  }

  return mappedNode;
};

/**
 * V2 段落导览入口组件
 *
 * @param {object} props - 组件属性
 * @param {string|number} props.paperId - 论文 ID（必需）
 * @param {Function} props.onNavigate - 坐标导航回调：(navigation) => void
 * @param {React.Ref} ref - 转发的 ref，用于暴露命令式 API（scrollToNode 方法）
 */
const ParagraphGuideV2 = forwardRef<any, any>(function ParagraphGuideV2({
  paperId,
  onNavigate,
  guideVersion,
}: any, ref) {
  const { message } = App.useApp();
  // ========== 状态管理 ==========

  /**
   * 是否启用 SSE 连接
   *
   * 用户点击"生成导览"后设为 true，开始接收 SSE 事件。
   */
  const [streamEnabled, setStreamEnabled] = useState(false); // 状态：SSE 是否启用

  /**
   * 导览节点数据
   *
   * 存储从 SSE 接收的节点数据，用于渲染 GuideTree。
   */
  const [guideNodes, setGuideNodes] = useState<any[]>([]); // 状态：导览节点数组

  /**
   * 论证地图数据
   *
   * 存储核心主张和核心概览，用于渲染 CoreOverview 组件。
   */
  const [argumentMap, setArgumentMap] = useState({
    coreClaim: null, // 核心主张（一句话）
    coreOverview: null, // 核心概览（200-300 字）
  }); // 状态：论证地图数据

  /**
   * 是否正在加载数据
   *
   * 初始加载或重新生成时为 true。
   */
  const [loading, setLoading] = useState(false); // 状态：是否正在加载

  /**
   * 是否存在已有导览数据
   *
   * 组件挂载时尝试加载已有导览，如果成功加载到数据则为 true。
   * 用于区分"旧版导览需要升级"和"新论文暂无导览"两种空状态。
   */
  const [hasExistingGuide, setHasExistingGuide] = useState(false); // 状态：是否已有导览数据

  /**
   * 生成方式标识
   *
   * 记录导览的生成方式，用于显示不同的提示横幅。
   * 取值：'ai'=AI生成, 'rule_no_key'=未配置API Key, 'error'=生成失败, ''=老数据或未知
   */
  const [generationMethod, setGenerationMethod] = useState(''); // 状态：生成方式标识

  /**
   * 生成进度信息
   *
   * 存储从 progress 事件获取的进度数据。
   */
  const [progressInfo, setProgressInfo] = useState({
    phase: null, // 当前阶段（'phase_a' | 'phase_b'）
    percent: 0, // 进度百分比（0-100）
    etaMs: null, // 预估剩余时间（毫秒）
    message: '', // 进度消息文本
  }); // 状态：进度信息对象

  /**
   * GuideTree 组件的 ref
   *
   * 用于调用 GuideTree 暴露的 scrollToNode 方法，实现反向导航。
   */
  const guideTreeRef = useRef<any>(null); // ref：指向 GuideTree 组件实例

  // ========== 使用 SSE Hook ==========

  /**
   * 使用 useGuideStream Hook 消费 SSE 事件
   *
   * 仅在 streamEnabled 为 true 时建立连接。
   */
  const {
    nodes: streamNodes, // 从 SSE 接收的节点
    status: streamStatus, // SSE 连接状态
    error: streamError, // SSE 错误对象
    progress: streamProgress, // SSE 进度信息
    retry: retryStream, // 手动重连函数
  } = useGuideStream(paperId, {
    enabled: streamEnabled, // 启用条件
    onNodeReady: (nodes: any) => {
      // 节点就绪回调（保留兼容）
      setGuideNodes(nodes);
      message.success('导览生成完成');
    },
    onProgress: (progress: any) => {
      // 进度更新回调
      setProgressInfo({
        phase: progress.phase,
        percent: progress.percent,
        etaMs: progress.eta_ms,
        message: progress.message,
      });
    },
    onError: (error: any) => {
      // 错误回调
      message.error(error.message || '导览生成失败'); // 显示错误提示
    },
    onDone: async () => {
      // 生成完成回调：后端 done 事件不携带节点数据，通过 REST API 获取
      try {
        const data = await getGuideTree(paperId) as any;
        const rawNodes = data.nodes || [];
        if (rawNodes.length > 0) {
          const nodes = rawNodes.map(mapNode);
          setGuideNodes(nodes);
          // 保存生成方式标识
          if (data.generationMethod) {
            setGenerationMethod(data.generationMethod);
          }
          // V4 导览阅读体验：保存论证地图数据（核心主张 + 核心概览）
          if (data.argumentMap) {
            setArgumentMap({
              coreClaim: data.argumentMap.coreClaim || null,
              coreOverview: data.argumentMap.coreOverview || null,
            });
          }
          message.success('导览生成完成');
        }
      } catch (err: any) {
        console.error('[ParagraphGuideV2] 完成后获取导览失败:', err);
      }
    },
  });

  // ========== 副作用（Effects）==========

  /**
   * Effect: 组件挂载时加载已有导览
   *
   * 检查是否已有生成的导览数据，如果有则直接加载。
   */
  useEffect(() => {
    const loadExistingGuide = async () => {
      if (!paperId) return; // 无 paperId 时跳过

      try {
        setLoading(true); // 设置加载状态

        // 调用 API 获取已有导览
        const data = await getGuideTree(paperId) as any;

        // 后端返回 data.nodes
        const rawNodes = data.nodes || [];
        if (rawNodes.length > 0) {
          const nodes = rawNodes.map(mapNode);
          setGuideNodes(nodes);
          setHasExistingGuide(true); // 标记已有导览数据
        }
        // 保存生成方式标识（用于显示提示横幅）
        if (data.generationMethod) {
          setGenerationMethod(data.generationMethod);
        }
        // V4 导览阅读体验：保存论证地图数据（核心主张 + 核心概览）
        if (data.argumentMap) {
          setArgumentMap({
            coreClaim: data.argumentMap.coreClaim || null,
            coreOverview: data.argumentMap.coreOverview || null,
          });
        }
        // 页面刷新恢复：后端返回 generating=true 表示正在生成中，
        // 需要启用 SSE 连接以恢复进度条和实时更新
        if (data.generating) {
          setStreamEnabled(true);
        }
      } catch (err: any) {
        // 忽略 404 错误（表示没有导览数据）
        if (err.message && !err.message.includes('404')) {
          console.error('[ParagraphGuideV2] 加载导览失败:', err);
        }
      } finally {
        setLoading(false); // 关闭加载状态
      }
    };

    loadExistingGuide(); // 执行加载
  }, [paperId]); // 依赖于 paperId

  /**
   * Effect: 同步 SSE 接收的节点到本地状态
   *
   * 当 streamNodes 变化时，同步到 guideNodes 状态。
   */
  useEffect(() => {
    if (streamNodes && streamNodes.length > 0) {
      setGuideNodes(streamNodes as any[]);
    }
  }, [streamNodes]); // 依赖于 streamNodes

  // ========== 事件处理函数 ==========

  /**
   * 触发导览生成
   *
   * 调用后端 API 触发 AI 生成导览，同时启用 SSE 连接接收进度。
   */
  const handleGenerate = useCallback(async () => {
    if (!paperId) {
      message.warning('请先选择论文'); // 无 paperId 时提示
      return;
    }

    try {
      setLoading(true); // 设置加载状态

      // 调用 API 触发生成
      await generateGuideV2(paperId);

      // 启用 SSE 连接，接收生成进度
      setStreamEnabled(true);

      message.info('开始生成导览，请稍候...'); // 显示提示
    } catch (err: any) {
      console.error('[ParagraphGuideV2] 生成导览失败:', err);
      message.error(err.message || '生成导览失败'); // 显示错误提示
    } finally {
      setLoading(false); // 关闭加载状态
    }
  }, [paperId]); // 依赖于 paperId

  /**
   * 处理节点点击事件
   *
   * 触发坐标导航，跳转到 PDF 对应页面并高亮。
   *
   * @param {object} node - 被点击的节点数据
   */
  const handleNodeClick = useCallback(
    (node: any) => {
      // 检查节点是否有导航数据
      if (!node.pageIdx && (!node.bboxes || node.bboxes.length === 0)) {
        message.warning('该节点没有关联的页面位置'); // 无导航数据时提示
        return;
      }

      // 触发导航回调
      onNavigate?.({
        pageIdx: node.pageIdx, // 页码索引
        bboxes: node.bboxes || [], // 边界框数组
        animationMs: 4000, // 动画持续时间
        imageUrl: node.imageUrl || null, // 关联图片 URL（可能为 null）
      });
    },
    [onNavigate] // 依赖于 onNavigate
  );

  /**
   * 重新生成导览
   *
   * 清空已有数据，重新触发生成。
   */
  const handleRegenerate = useCallback(async () => {
    setGuideNodes([]); // 清空导览节点
    setProgressInfo({ phase: null, percent: 0, etaMs: null, message: '' }); // 重置进度
    await handleGenerate(); // 重新生成
  }, [handleGenerate]); // 依赖于 handleGenerate

  // ========== 右键菜单处理 ==========

  /**
   * 右键菜单位置（null 表示隐藏，{x, y} 表示显示在对应屏幕坐标）
   */
  const [contextMenuPos, setContextMenuPos] = useState<any>(null); // 状态：右键菜单位置

  /**
   * 右键菜单处理函数
   *
   * 在导览栏空白区域右键时显示自定义上下文菜单，提供"重新生成导览"选项。
   * 使用 e.preventDefault() 阻止浏览器默认右键菜单。
   *
   * @param {MouseEvent} e - 右键事件对象
   */
  const handleContextMenu = useCallback((e: any) => {
    // 不在按钮、输入框等交互元素上时，显示"重新生成导览"菜单
    if (e.target.closest('.ant-btn') || e.target.closest('.ant-input')) {
      return; // 点击在交互元素上，不显示菜单
    }
    e.preventDefault(); // 阻止浏览器默认右键菜单
    setContextMenuPos({ x: (e as any).clientX, y: (e as any).clientY }); // 记录右键点击的屏幕坐标
  }, []);

  /**
   * 右键菜单项点击处理
   *
   * 关闭右键菜单并执行重新生成操作。
   */
  const handleContextMenuClick = useCallback(() => {
    setContextMenuPos(null); // 关闭右键菜单
    handleRegenerate(); // 调用重新生成函数
  }, [handleRegenerate]);

  /**
   * 关闭右键菜单
   *
   * 点击菜单外部区域时调用，隐藏右键菜单。
   */
  const closeContextMenu = useCallback(() => {
    setContextMenuPos(null); // 将位置设为 null，菜单消失
  }, []);

  /**
   * 命令式 API（暴露给父组件）
   *
   * 提供 scrollToNode 方法，用于反向导航（PDF 点击 → 导览滚动）。
   */
  useImperativeHandle(ref, () => ({
    /**
     * 滚动到指定节点
     *
     * 委托给 GuideTree 的 scrollToNode 方法实现。
     *
     * @param {string} nodeId - 目标节点 ID
     */
    scrollToNode: (nodeId: any) => {
      if (guideTreeRef.current?.scrollToNode) {
        guideTreeRef.current.scrollToNode(nodeId);
      }
    },
  }), []); // 无外部依赖，guideTreeRef 是稳定引用

  // ========== 渲染 ==========

  /**
   * 生成中的进度显示
   *
   * 当 SSE 状态为 streaming 时显示进度条。
   */
  // 判断是否正在生成中：包括 SSE 已连接的 streaming 状态，以及 API 返回后等待 SSE 建立的 connecting 状态
  // 必须同时检查 streamEnabled，避免组件初始挂载时（streamEnabled=false, streamStatus='connecting'）误判为生成中
  const isStreaming = streamEnabled && (streamStatus === 'connecting' || streamStatus.includes('streaming'));

  /**
   * 格式化剩余时间
   *
   * 将毫秒数转换为人类可读的时间格式。
   *
   * @param {number} etaMs - 剩余时间（毫秒）
   * @returns {string} 格式化后的时间字符串
   */
  const formatEta = (etaMs: any) => {
    if (!etaMs) return ''; // 无剩余时间时返回空字符串

    const seconds = Math.ceil(etaMs / 1000); // 转换为秒

    if (seconds < 60) {
      return `约 ${seconds} 秒`; // 小于 1 分钟
    } else {
      const minutes = Math.ceil(seconds / 60); // 转换为分钟
      return `约 ${minutes} 分钟`; // 大于 1 分钟
    }
  };

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        height: '100%',
        background: 'var(--bg-primary)',
      }}
      onContextMenu={handleContextMenu}
    >
      {/**
       * 进度条区域
       *
       * 生成中时显示进度条和进度信息。
       */}
      {isStreaming && (
        <div style={{ padding: '12px 16px 8px' }}>
          <Progress
            percent={Math.round(progressInfo.percent || 0)} // 进度百分比
            status="active" // 激活状态（动画效果）
            size="small" // 小号尺寸
          />
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              marginTop: 4,
              fontSize: 11,
              color: 'var(--text-secondary)',
            }}
          >
            {/* 阶段标签 */}
            <span>
              {progressInfo.phase === 'phase_a' ? '全文导览' : '精细化导览'}
            </span>
            {/* 剩余时间 */}
            <span>{formatEta(progressInfo.etaMs)}</span>
          </div>
          {/* 进度消息 */}
          {progressInfo.message && (
            <div
              style={{
                marginTop: 4,
                fontSize: 11,
                color: 'var(--text-tertiary)',
              }}
            >
              {progressInfo.message}
            </div>
          )}
        </div>
      )}

      {/**
       * 错误提示
       *
       * SSE 错误时显示错误提示和重试按钮。
       */}
      {streamError && (
        <div style={{ padding: '8px 12px' }}>
          <Alert
            type="error"
            message={streamError.message || '导览生成失败'}
            closable
            onClose={() => {
              /* 关闭提示 */
            }}
            action={
              <Button size="small" onClick={retryStream}>
                重试
              </Button>
            }
          />
        </div>
      )}

      {/**
       * 生成方式提示横幅
       *
       * 当导览非 AI 生成时显示提示横幅，引导用户配置 API Key 或重新生成。
       */}
      {generationMethod === 'rule_no_key' && (
        <div style={{ padding: '8px 12px' }}>
          <Alert
            type="info"
            message="当前导览由基础提取生成。请在模型配置中设置 API Key 后重新生成"
            closable
            style={{ marginBottom: 8 }}
            onClose={() => {
              /* 关闭提示 */
            }}
          />
        </div>
      )}
      {generationMethod === 'error' && (
        <div style={{ padding: '8px 12px' }}>
          <Alert
            type="warning"
            message="导览可能不完整，建议重新生成"
            closable
            style={{ marginBottom: 8 }}
            onClose={() => {
              /* 关闭提示 */
            }}
          />
        </div>
      )}

      {/**
       * 空状态：无导览数据
       *
       * 全局面板可点击，与摘要面板风格一致。
       * 直接作为 flex 子元素渲染（不嵌套在 auto-scrollbar 中），
       * 使 flex:1 能正确撑满整个面板高度。
       */}
      {guideNodes.length === 0 && !loading && !isStreaming && (
        <div
          style={{
            flex: 1,
            display: 'flex',
            flexDirection: 'column',
            cursor: 'pointer',
          }}
          onClick={handleGenerate}
        >
          <div style={{
            flex: 1,
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            gap: 12,
            opacity: 0.6,
            transition: 'opacity 0.2s',
          }}>
            {hasExistingGuide && guideVersion !== 'v2' ? (
              <>
                <span style={{ fontSize: 13, color: 'var(--text-secondary)' }}>此论文使用旧版导览格式</span>
                <span style={{ fontSize: 12, color: 'var(--text-tertiary)' }}>点击重新生成，以使用新版层次化导览</span>
              </>
            ) : (
              <span style={{ fontSize: 13, color: 'var(--text-secondary)' }}>点击生成导览</span>
            )}
          </div>
        </div>
      )}

      {/**
       * 导览树内容区
       *
       * 有导览数据时渲染，包裹在可滚动容器中。
       */}
      {guideNodes.length > 0 && (
        <div
          className="auto-scrollbar"
          style={{
            flex: 1,
            overflow: 'auto',
            padding: '0 8px 8px',
          }}
        >
          {/**
           * V4 导览阅读体验：核心主张 + 核心概览卡片
           *
           * 在导览树顶部显示，帮助读者快速判断论文价值。
           */}
          <CoreOverview
            coreClaim={argumentMap.coreClaim}
            coreOverview={argumentMap.coreOverview}
          />

          <GuideTree
            ref={guideTreeRef} // 暴露 ref，用于父组件调用 scrollToNode 方法
            nodes={guideNodes} // 导览节点数据
            loading={loading} // 加载状态
            onNodeClick={handleNodeClick} // 节点点击回调
            expandLevel="first" // 默认展开第一层
          />
        </div>
      )}

      {/* 右键菜单：重新生成导览 */}
      {contextMenuPos && (
        <div
          style={{ position: 'fixed', top: 0, left: 0, right: 0, bottom: 0, zIndex: 1000 }}
          onClick={closeContextMenu}
          onContextMenu={(e) => { e.preventDefault(); closeContextMenu(); }}
        >
          <div
            style={{
              position: 'fixed',
              top: contextMenuPos.y,
              left: contextMenuPos.x,
              background: 'var(--bg-primary, #fff)',
              border: '1px solid var(--border-color, #e8e8e8)',
              borderRadius: 6,
              boxShadow: '0 2px 8px rgba(0,0,0,0.12)',
              padding: '4px 0',
              minWidth: 140,
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <div
              onClick={handleContextMenuClick}
              style={{
                padding: '6px 12px',
                cursor: 'pointer',
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                fontSize: 13,
                color: isStreaming ? 'var(--text-secondary, #999)' : 'var(--text-primary, #333)',
              }}
              onMouseEnter={(e) => { if (!isStreaming) e.currentTarget.style.background = 'var(--overlay-hover)'; }}
              onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
            >
              <ReloadOutlined spin={isStreaming} />
              {isStreaming ? '生成中...' : '重新生成导览'}
            </div>
          </div>
        </div>
      )}
    </div>
  );
});

// ========== PropTypes 类型检查 ==========

/**
 * 组件属性类型检查
 */
ParagraphGuideV2.propTypes = {
  /**
   * 论文 ID
   */
  paperId: PropTypes.oneOfType([PropTypes.string, PropTypes.number]).isRequired,

  /**
   * 坐标导航回调
   *
   * 点击节点时触发，传递导航数据（页码和边界框）。
   */
  onNavigate: PropTypes.func, // 导航回调（可选）

  /**
   * 导览版本（如 'v2'），用于显示兼容性提示
   */
  guideVersion: PropTypes.string,
};

/**
 * 默认导出：ParagraphGuideV2 组件
 */
export default ParagraphGuideV2;
