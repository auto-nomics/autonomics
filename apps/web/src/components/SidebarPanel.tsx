/**
 * JayRead 侧边栏面板容器组件
 *
 * 这是阅读器页面左侧的面板容器，整合了导览功能：
 * - ParagraphGuideV2：论文段落级阅读引导（V4 导览系统）
 * - CoreOverview：核心主张 + 核心概览卡片（预读阶段）
 *
 * 设计考虑：
 * - 支持折叠/展开状态，由外层 PaperReaderPage 控制
 * - 折叠时不渲染任何内容
 * - 使用 useImperativeHandle 代理 ParagraphGuide 的方法
 */
import React from 'react';
import ParagraphGuideV2 from '../features/paragraph-guide-v2/'; // 导入段落导览组件（V4，阅读引导）
import './SidebarPanel.css'; // 导入侧边栏面板样式

/**
 * 侧边栏面板组件
 *
 * @param {object} props - 组件属性
 * @param {number} props.paperId - 论文 ID，用于获取导览数据
 * @param {boolean} props.collapsed - 侧边栏是否折叠，true 时只显示展开按钮
 * @param {function} props.onToggleCollapse - 折叠/展开切换回调函数
 * @param {object} props.paperInfo - 论文基础信息（标题、作者、摘要等）
 * @param {function} props.onCoordinateNavigate - V2 坐标导航回调函数，接收参数 ({pageIdx, bboxes, animationMs})
 * @param {React.Ref} props.guideV2Ref - V2 导览组件的 ref，用于调用 scrollToNode 方法（反向导航）
 */
const SidebarPanel = React.memo(function SidebarPanel({
  paperId, // 论文 ID，用于 API 调用
  collapsed, // 侧边栏是否折叠
  onToggleCollapse, // 折叠/展开切换回调函数
  paperInfo = null, // 论文基础信息，默认 null
  onCoordinateNavigate, // V2 坐标导航回调函数（签名 ({pageIdx, bboxes, animationMs})）
  guideV2Ref, // V2 导览组件的 ref（用于反向导航调用 scrollToNode）
}: {
  paperId: number;
  collapsed: boolean;
  onToggleCollapse: () => void;
  paperInfo?: { title?: string; guide_version?: string } | null;
  onCoordinateNavigate: (coords: { pageIdx: number; bboxes: unknown[]; animationMs?: number }) => void;
  guideV2Ref: React.Ref<unknown>;
}) {

  // ========== 折叠状态：不渲染内容，通过 Ctrl+Left 展开 ==========
  if (collapsed) {
    return null;
  }

  // ========== 展开状态：直接渲染导览面板（不再有 Tab 切换） ==========
  return (
    <div className="sidebar-panel-wrapper">
      <div className="sidebar-tab-content">
        <div
          className="sidebar-tab-pane"
          style={{
            display: 'flex',
            flexDirection: 'column',
          }}
        >
          {/* V4 段落导览组件（阅读引导 + 坐标导航） */}
          {React.createElement(ParagraphGuideV2 as any, {
            ref: guideV2Ref,
            paperId,
            onNavigate: onCoordinateNavigate,
            guideVersion: paperInfo?.guide_version,
          })}
        </div>
      </div>
    </div>
  );
});

// 导出 SidebarPanel 组件
export default SidebarPanel;
