/**
 * PaperTable 组件
 *
 * 渲染论文列表表格，支持：
 * - 列宽度拖拽调整
 * - 行右键菜单
 * - 附件展开
 * - 表头右键菜单（列可见性切换）
 *
 * 正常模式和筛选模式共用同一个表格，区别在于：
 * - 正常模式：expandable 启用，点击标题展开附件
 * - 筛选模式：expandable 禁用，点击标题选中论文
 */
import React, { useState, useLayoutEffect } from 'react';
import { Table, Checkbox, Space } from 'antd';
import { ALL_COLUMNS } from '../paperListConstants';
import ResizableTitle from '../ResizableTitle';
import AttachmentList from './AttachmentList';

/**
 * 论文表格组件
 *
 * @param {Object} props - 组件属性
 * @param {Array} props.papers - 论文列表数据
 * @param {Array} props.activeColumns - 活跃列配置
 * @param {number} props.tableScrollX - 表格总宽度
 * @param {boolean} props.screeningMode - 筛选模式标志
 * @param {Array<number>} props.expandedRowKeys - 已展开行 key 列表
 * @param {Function} props.handleExpand - 展开/折叠回调
 * @param {Object} props.attachmentCache - 附件缓存
 * @param {Object} props.attachmentLoading - 附件加载状态
 * @param {Function} props.handleDeleteAttachment - 删除附件回调
 * @param {Function} props.handleOpenAttachment - 打开附件回调
 * @param {Function} props.handleSetPrimary - 设置主 PDF 回调
 * @param {Function} props.TableRowWithDragAndContextMenu - 自定义表格行组件（virtual table 下返回 <div>）
 * @param {Object|null} props.headerContextMenu - 表头右键菜单位置
 * @param {Function} props.setHeaderContextMenu - 设置表头右键菜单位置
 * @param {Array<string>} props.visibleColumns - 可见列 key 数组
 * @param {Function} props.toggleColumnVisibility - 切换列可见性回调
 * @param {Object} props.selectedPaperForScreening - 当前选中的筛选论文
 * @param {Function} props.handleContextMenu - 右键菜单回调
 * @param {Function} props.setSelectedPaperForScreening - 选中论文回调
 * @param {Function} props.setLeftPanelView - 切换左面板视图回调
 */
function PaperTable({
  papers,
  activeColumns,
  tableScrollX,
  screeningMode,
  expandedRowKeys,
  handleExpand,
  attachmentCache,
  attachmentLoading,
  handleDeleteAttachment,
  handleOpenAttachment,
  handleSetPrimary,
  TableRowWithDragAndContextMenu,
  headerContextMenu,
  setHeaderContextMenu,
  visibleColumns,
  toggleColumnVisibility,
  selectedPaperForScreening,
  handleContextMenu,
  setSelectedPaperForScreening,
  setLeftPanelView,
  containerRef,
}: any) {
  // antd virtual table 要求 scroll.y 必须是数字。用 ResizeObserver 测父容器真实高度，
  // 别再用 window.innerHeight - 180 那种魔法数——桌面版 decorations:false 没有应用 chrome，
  // 而且分屏/抽屉开合/different DPI 都会让 innerHeight 跟实际可用高度脱节，
  // 表格底部就会空出一块露 antd Layout 的灰色 colorBgLayout（"灰色挡板" bug）。
  //
  // containerRef 由调用方传入（PaperListPage/ScreeningMode 的 `flex:1, overflow:auto` 容器），
  // 高度由父级 flex 决定，跟 scrollY 完全解耦——直接观察本组件里的元素会形成反馈循环
  // （wrapper→Table→scrollY 互相耦合，挡板会循环上下浮动）。
  //
  // useLayoutEffect 而非 useEffect：在 paint 前同步定好 scrollY，避免首次渲染先闪一下
  // 未虚拟化的全表（scrollY=0 时 antd 会渲染全部行）再切到虚拟列表。
  const [scrollY, setScrollY] = useState(0);
  useLayoutEffect(() => {
    // 初次挂载时 containerRef 挂在祖先 wrapper div 上，React 先跑后代的 layout
    // effect、后挂祖先 ref，此刻 .current 还是 null——不能就此放弃，否则 observer
    // 永远建不起来、scrollY 恒 0，virtual 表会退化成普通 table 且丢掉自定义行
    // （右键菜单/拖拽全部失效）。rAF 到下一帧重试，ref 必已挂上。
    let ro: ResizeObserver | null = null;
    let cancelled = false;
    const attach = (): boolean => {
      const el = containerRef?.current;
      if (!el || cancelled) return false;
      const update = () => setScrollY(Math.max(0, el.clientHeight - 40));
      update();
      ro = new ResizeObserver(update);
      ro.observe(el);
      return true;
    };
    let raf = 0;
    if (!attach()) {
      raf = requestAnimationFrame(() => { if (attach()) raf = 0; });
    }
    return () => {
      cancelled = true;
      if (raf) cancelAnimationFrame(raf);
      ro?.disconnect();
    };
  }, [containerRef]);

  // 无论文时不渲染表格（必须放在所有 hook 之后，避免 React 报
  // "Rendered fewer hooks than expected"）
  if (papers.length === 0) return null;

  return (
    <>
      <Table
        className="paper-list-table"
        dataSource={papers}
        rowKey="id"
        size="small"
        scroll={{ x: tableScrollX, y: scrollY || undefined }}
        pagination={false}
        columns={activeColumns}
        showSorterTooltip={false}
        virtual
        // 筛选模式下禁止展开附件，正常模式下启用
        expandable={screeningMode ? undefined : {
          expandedRowKeys: expandedRowKeys,
          onExpand: handleExpand,
          expandedRowRender: (paper: any) => (
            <AttachmentList
              paper={paper}
              attachments={attachmentCache[paper.id] || []}
              loading={!!attachmentLoading[paper.id]}
              onDelete={(attId: any) => handleDeleteAttachment(paper.id, attId)}
              onOpenPdf={(attId: any) => handleOpenAttachment(paper.id, attId)}
              onSetPrimary={(attId: any) => handleSetPrimary(paper.id, attId)}
            />
          ),
          rowExpandable: () => true,
          expandRowByClick: false,
          columnWidth: 0,
          showExpandColumn: false,
        }}
        onHeaderRow={() => ({
          onContextMenu: (e) => {
            e.preventDefault();
            setHeaderContextMenu({ x: e.clientX, y: e.clientY });
          },
        })}
        components={{
          header: { cell: ResizableTitle },
          body: {
            row: (props: any) => (
              <TableRowWithDragAndContextMenu
                {...props}
                screeningMode={screeningMode}
                selectedPaperForScreening={selectedPaperForScreening}
                onContextMenu={handleContextMenu}
                papers={papers}
                onSelectPaper={setSelectedPaperForScreening}
                onChangeLeftPanelView={setLeftPanelView}
              />
            ),
          },
        }}
        style={{ background: 'var(--bg-primary)', width: '100%' }}
      />
      {/* 表头右键菜单 */}
      {headerContextMenu && (
        <div
          onClick={(e) => e.stopPropagation()}
          style={{
            position: 'fixed',
            left: headerContextMenu.x,
            top: headerContextMenu.y,
            zIndex: 1050,
            background: 'var(--bg-elevated)',
            borderRadius: 8,
            boxShadow: '0 6px 16px 0 rgba(0,0,0,0.08), 0 3px 6px -4px rgba(0,0,0,0.12), 0 9px 28px 8px rgba(0,0,0,0.05)',
            padding: '8px 12px',
            maxHeight: 400,
            overflow: 'auto',
            width: 240,
          }}
        >
          <div style={{ fontWeight: 500, marginBottom: 8, fontSize: 14, color: 'var(--text-primary)' }}>
            选择显示列
          </div>
          {/* 列可见性切换 */}
          <Checkbox.Group
            value={visibleColumns}
            onChange={toggleColumnVisibility}
          >
            <Space direction="vertical" size={4}>
              {ALL_COLUMNS.map(col => (
                <Checkbox
                  key={col.key}
                  value={col.key}
                  disabled={col.key === 'title'}
                  style={{ fontSize: 13 }}
                >
                  {col.label}
                </Checkbox>
              ))}
            </Space>
          </Checkbox.Group>
        </div>
      )}
    </>
  );
}

export default React.memo(PaperTable);
