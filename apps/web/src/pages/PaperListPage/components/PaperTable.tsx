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
import React, { useState, useLayoutEffect, useCallback, useMemo } from 'react';
import { Table, Checkbox, Space, App } from 'antd';
import { ALL_COLUMNS } from '../paperListConstants';
import ResizableTitle from '../ResizableTitle';
import AttachmentList from './AttachmentList';
import { usePreTranslation } from '../hooks/usePreTranslation';
import usePaperStore from '../../../stores/usePaperStore';
import { updatePaper } from '../../../services/papersApi';

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
 * @param {Function} props.handleReparse - 重解析回调
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
  handleReparse,
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
  const setPapers = usePaperStore((s: any) => s.setPapers);
  const { message } = App.useApp();

  const patchPaperInStore = useCallback((paperId: string, patch: Record<string, unknown>) => {
    setPapers((prev: any[]) => prev.map((p: any) => p.id === paperId ? { ...p, ...patch } : p));
  }, [setPapers]);

  // 从 store 派生正在生成翻译的论文 id，交给 usePreTranslation 自动轮询。
  // 上传 PDF 后后端自动触发的预翻译不走手动 trigger，之前没有轮询，
  // tag 会永远停在"翻译中 0/0"直到某次列表刷新。
  const generatingIds = useMemo(
    () => (papers as any[])
      .filter((p: any) => p?.paragraphTranslationStatus === 'generating')
      .map((p: any) => String(p.id)),
    [papers],
  );

  const { isPreTranslating, progress: preTranslateProgressMap, triggerPreTranslate } = usePreTranslation({
    autoWatchIds: generatingIds,
    onStart: (paperId) => {
      patchPaperInStore(paperId, { paragraphTranslationStatus: 'generating' });
    },
    onComplete: (paperId, failedCount) => {
      patchPaperInStore(paperId, {
        paragraphTranslationStatus: failedCount > 0 ? 'failed' : 'done',
        // 失败时显式写 false, 而非 undefined。下游 AttachmentList/PaperReaderPage
        // 都用严格 === true 判断, undefined 会被视为"未启用"——但 paper 对象上
        // undefined 也可能是"字段未加载", 二者混淆。显式 false 让"预翻译失败"
        // 的语义清晰。
        hoverTranslationEnabled: failedCount > 0 ? false : true,
      });
    },
  });

  /**
   * 切换悬浮翻译开关（乐观更新）
   *
   * 调 API 前先本地切 Tag 状态，失败回滚。详见 memory: feedback_tauri_caching
   */
  const handleToggleHoverTranslation = useCallback(async (paperId: string, next: boolean) => {
    patchPaperInStore(paperId, { hoverTranslationEnabled: next });
    try {
      await updatePaper(paperId, { hoverTranslationEnabled: next });
    } catch (err: any) {
      // 回滚
      patchPaperInStore(paperId, { hoverTranslationEnabled: !next });
      message.error(`切换悬浮翻译失败: ${err?.message || err}`);
    }
  }, [patchPaperInStore, message]);

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
    const el = containerRef?.current;
    if (!el) return;
    const update = () => setScrollY(Math.max(0, el.clientHeight - 40));
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
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
              onReparse={(engine: any) => handleReparse(paper.id, engine)}
              onSetPrimary={(attId: any) => handleSetPrimary(paper.id, attId)}
              onPreTranslate={triggerPreTranslate}
              preTranslating={isPreTranslating(paper.id)}
              preTranslateProgress={preTranslateProgressMap[paper.id] ?? null}
              onToggleHoverTranslation={handleToggleHoverTranslation}
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
