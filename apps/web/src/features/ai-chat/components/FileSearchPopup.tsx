/**
 * FileSearchPopup 组件
 *
 * AI 聊天中的文件搜索弹窗组件
 * 功能：
 * - 显示文件搜索结果列表
 * - 支持键盘导航（上下箭头选择）
 * - 自动滚动当前选中项到可视区域
 * - 按文件类型上色（文件夹深色加粗，文件按扩展名映射色板）
 */

import React, { useEffect, useRef, useCallback } from 'react';
import type { FileSearchResult } from '../../../services/fileSearchApi';

// 文件扩展名 → CSS 颜色变量 (复用 global.css 中 --file-icon-* 色板)
const FILE_COLOR_BY_EXT: Record<string, string> = {
  pdf: 'var(--file-icon-pdf)',
  doc: 'var(--file-icon-word)', docx: 'var(--file-icon-word)',
  xls: 'var(--file-icon-excel)', xlsx: 'var(--file-icon-excel)', csv: 'var(--file-icon-excel)',
  ppt: 'var(--file-icon-ppt)', pptx: 'var(--file-icon-ppt)',
  png: 'var(--file-icon-image)', jpg: 'var(--file-icon-image)', jpeg: 'var(--file-icon-image)',
  gif: 'var(--file-icon-image)', webp: 'var(--file-icon-image)', svg: 'var(--file-icon-image)',
  bmp: 'var(--file-icon-image)', ico: 'var(--file-icon-image)',
  mp3: 'var(--file-icon-audio)', wav: 'var(--file-icon-audio)', flac: 'var(--file-icon-audio)',
  aac: 'var(--file-icon-audio)', ogg: 'var(--file-icon-audio)', m4a: 'var(--file-icon-audio)',
  mp4: 'var(--file-icon-video)', mkv: 'var(--file-icon-video)', mov: 'var(--file-icon-video)',
  avi: 'var(--file-icon-video)', webm: 'var(--file-icon-video)',
  zip: 'var(--file-icon-archive)', tar: 'var(--file-icon-archive)', gz: 'var(--file-icon-archive)',
  rar: 'var(--file-icon-archive)', '7z': 'var(--file-icon-archive)',
  js: 'var(--file-icon-code)', ts: 'var(--file-icon-code)', tsx: 'var(--file-icon-code)',
  jsx: 'var(--file-icon-code)', py: 'var(--file-icon-code)', rs: 'var(--file-icon-code)',
  go: 'var(--file-icon-code)', java: 'var(--file-icon-code)', c: 'var(--file-icon-code)',
  cpp: 'var(--file-icon-code)', h: 'var(--file-icon-code)', rb: 'var(--file-icon-code)',
  php: 'var(--file-icon-code)', sh: 'var(--file-icon-code)', bash: 'var(--file-icon-code)',
  zsh: 'var(--file-icon-code)',
  epub: 'var(--file-icon-ebook)', mobi: 'var(--file-icon-ebook)', azw3: 'var(--file-icon-ebook)',
  json: 'var(--file-icon-data)', yaml: 'var(--file-icon-data)', yml: 'var(--file-icon-data)',
  toml: 'var(--file-icon-data)', xml: 'var(--file-icon-data)',
  md: 'var(--file-icon-text)', markdown: 'var(--file-icon-text)',
  txt: 'var(--file-icon-text)', rtf: 'var(--file-icon-text)',
  html: 'var(--file-icon-web)', htm: 'var(--file-icon-web)', css: 'var(--file-icon-web)',
};

// 文件夹/文件 → name 渲染样式
function getFileColorStyle(name: string, isDirectory: boolean): React.CSSProperties {
  if (isDirectory) {
    return { color: 'var(--text-primary)', fontWeight: 600 };
  }
  const dotIdx = name.lastIndexOf('.');
  const ext = dotIdx > 0 ? name.slice(dotIdx + 1).toLowerCase() : '';
  return { color: FILE_COLOR_BY_EXT[ext] ?? 'var(--file-icon-default)' };
}

// 组件属性接口
interface FileSearchPopupProps {
  visible: boolean;                          // 是否显示弹窗
  results: FileSearchResult[];               // 搜索结果列表
  selectedIndex: number;                     // 当前选中的索引
  loading: boolean;                          // 是否正在加载
  onSelect: (result: FileSearchResult) => void;  // 选中项的回调
  onHover: (index: number) => void;          // 鼠标悬停的回调
}

const FileSearchPopup: React.FC<FileSearchPopupProps> = ({
  visible,
  results,
  selectedIndex,
  loading,
  onSelect,
  onHover,
}) => {
  // 列表容器引用，用于滚动控制
  const listRef = useRef<HTMLDivElement>(null);

  // 当选中项变化时，自动滚动该项到可视区域
  useEffect(() => {
    if (!listRef.current || results.length === 0) return;
    const activeItem = listRef.current.children[selectedIndex] as HTMLElement;
    if (activeItem) {
      // block: 'nearest' 表示仅在项不可见时才滚动
      activeItem.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex, results.length]);

  // 处理列表项点击
  const handleClick = useCallback(
    (result: FileSearchResult) => {
      onSelect(result);
    },
    [onSelect]
  );

  // 不可见时不渲染
  if (!visible) return null;

  return (
    <div className="file-search-popup" ref={listRef}>
      {/* 加载状态 */}
      {loading && (
        <div className="file-search-loading">搜索中...</div>
      )}
      {/* 无结果状态 */}
      {!loading && results.length === 0 && (
        <div className="file-search-empty">未找到匹配文件</div>
      )}
      {/* 结果列表 */}
      {!loading &&
        results.map((result, index) => (
          <div
            key={result.path}
            className={`file-search-item${index === selectedIndex ? ' active' : ''}`}
            onMouseEnter={() => onHover(index)}
            onMouseDown={(e) => {
              // 防止 mousedown 事件导致输入框失焦
              e.preventDefault();
              handleClick(result);
            }}
          >
            {/* 文件名, 按类型上色 (文件夹深色加粗, 文件按扩展名映射色板) */}
            <span className="file-search-name" style={getFileColorStyle(result.name, result.is_directory)}>
              {result.name}
            </span>
            {/* 完整路径显示 */}
            <span className="file-search-path" title={result.path}>
              {result.path}
            </span>
          </div>
        ))}
    </div>
  );
};

export default FileSearchPopup;
