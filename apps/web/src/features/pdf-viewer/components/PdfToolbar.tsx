/**
 * PDF 工具栏组件
 *
 * 从 PdfViewer 中提取以改善代码组织。
 *
 * 职责：
 * - 带目标选择器的统一搜索栏（导览/聊天/PDF）
 * - PDF 搜索导航控制（上一个/下一个匹配）
 * - 高亮注释按钮
 *
 * 页码指示器已移除（用户用 Alt+Left 切换缩略图查看页码）。
 * Props 从 PdfViewer 传递下来以保持关注点分离。
 */

import React from 'react';
import { Input, Button, Spin, Tooltip, Dropdown } from 'antd';
import {
  LeftOutlined,
  RightOutlined,
  HighlightOutlined,
} from '@ant-design/icons';
import i18next from 'i18next';
import styles from '../PdfViewer.module.css';

/** 搜索目标类型 */
export interface SearchTarget {
  key: 'outline' | 'chat' | 'pdf';
  label: string;
}

/** 搜索目标列表 */
const SEARCH_TARGETS: SearchTarget[] = [
  { key: 'outline', label: i18next.t('paperReader:searchTarget.outline') },
  { key: 'chat', label: i18next.t('paperReader:searchTarget.chat') },
  { key: 'pdf', label: i18next.t('paperReader:searchTarget.pdf') },
];

export interface PdfToolbarProps {
  // 搜索状态
  /** 当前搜索目标 */
  searchTarget: 'outline' | 'chat' | 'pdf';
  /** 搜索关键词 */
  searchKeyword: string;
  /** 是否正在搜索 PDF */
  pdfSearching: boolean;
  /** PDF 搜索匹配结果列表 */
  pdfSearchMatches: unknown[];
  /** 当前匹配索引 */
  currentMatchIndex: number;

  // 搜索回调
  /** 搜索目标变化回调 */
  onSearchTargetChange: (target: 'outline' | 'chat' | 'pdf') => void;
  /** 搜索输入变化回调 */
  onSearchChange: (e: React.ChangeEvent<HTMLInputElement>) => void;
  /** 搜索按键回调 */
  onSearchKeyDown: (e: React.KeyboardEvent<HTMLInputElement>) => void;
  /** 上一个匹配回调 */
  onPrevMatch: () => void;
  /** 下一个匹配回调 */
  onNextMatch: () => void;

  // 高亮
  /** 创建高亮注释回调 */
  onCreateHighlight: () => void;
}

/**
 * PDF 工具栏组件
 *
 * 布局（水平 flex）：
 * - 左侧：统一搜索栏及 PDF 导航控制
 * - 右侧：高亮按钮 + 缩放百分比
 */
export const PdfToolbar: React.FC<PdfToolbarProps> = React.memo(({
  searchTarget,
  searchKeyword,
  pdfSearching,
  pdfSearchMatches,
  currentMatchIndex,
  onSearchTargetChange,
  onSearchChange,
  onSearchKeyDown,
  onPrevMatch,
  onNextMatch,
  onCreateHighlight,
}) => {
  const getSearchPlaceholder = React.useCallback((): string => {
    switch (searchTarget) {
      case 'outline':
        return '搜索导览...';
      case 'chat':
        return '搜索聊天记录...';
      case 'pdf':
        return '搜索 PDF 全文...';
      default:
        return '搜索...';
    }
  }, [searchTarget]);

  const getSearchTargetLabel = React.useCallback((): string => {
    const target = SEARCH_TARGETS.find(t => t.key === searchTarget);
    return target?.label || 'PDF';
  }, [searchTarget]);

  return (
    <div className={styles.toolbar}>
      <Input
        size="small"
        prefix={
          <Dropdown
            menu={{
              items: SEARCH_TARGETS.map(t => ({ key: t.key, label: t.label })),
              onClick: ({ key }) => onSearchTargetChange(key as 'outline' | 'chat' | 'pdf'),
              selectedKeys: [searchTarget],
            }}
            trigger={['click']}
          >
            <span className={styles.searchTargetSelector}>
              {getSearchTargetLabel()}
            </span>
          </Dropdown>
        }
        placeholder={getSearchPlaceholder()}
        allowClear
        value={searchKeyword}
        onChange={onSearchChange}
        onKeyDown={onSearchKeyDown}
        suffix={pdfSearching ? <Spin size="small" /> : null}
        className={styles.searchInput}
      />

      {searchTarget === 'pdf' && searchKeyword && pdfSearchMatches.length > 0 && (
        <div className={styles.pdfSearchNav}>
          <span className={styles.matchCounter}>
            {currentMatchIndex + 1} / {pdfSearchMatches.length}
          </span>
          <Tooltip title="上一个匹配 (Shift+Enter)">
            <Button
              size="small"
              type="text"
              icon={<LeftOutlined />}
              onClick={onPrevMatch}
              className={styles.iconButton}
            />
          </Tooltip>
          <Tooltip title="下一个匹配 (Enter)">
            <Button
              size="small"
              type="text"
              icon={<RightOutlined />}
              onClick={onNextMatch}
              className={styles.iconButton}
            />
          </Tooltip>
        </div>
      )}

      <Button
        size="small"
        type="text"
        icon={<HighlightOutlined />}
        onClick={onCreateHighlight}
        className={styles.iconButton}
      />
    </div>
  );
});
