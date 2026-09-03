/**
 * FileDropzoneOverlay - 全局文件拖拽覆盖层
 *
 * 当用户从外部（文件管理器、Chrome 下载气泡等）拖入文件时，
 * 显示半透明遮罩层和居中的放置提示卡片。
 *
 * 设计要点：
 * - 始终渲染（pointerEvents: none），通过 opacity 实现平滑过渡
 * - 使用主题强调色保持视觉一致性
 * - 不干扰页面内部的表格行拖拽排序
 */

import React from 'react';
import { InboxOutlined } from '@ant-design/icons';

export default function FileDropzoneOverlay({ visible }: { visible: boolean }) {
  return (
    <div style={{
      position: 'fixed',
      inset: 0,
      backgroundColor: visible ? 'rgba(0, 0, 0, 0.45)' : 'rgba(0, 0, 0, 0)',
      display: 'flex',
      justifyContent: 'center',
      alignItems: 'center',
      zIndex: 9999,
      pointerEvents: 'none',
      transition: 'background-color 0.2s ease',
    }}>
      <div style={{
        border: `2px dashed ${visible ? 'var(--color-primary)' : 'transparent'}`,
        borderRadius: '12px',
        padding: '48px 64px',
        backgroundColor: visible ? 'var(--bg-elevated)' : 'transparent',
        textAlign: 'center',
        opacity: visible ? 1 : 0,
        transform: visible ? 'scale(1)' : 'scale(0.95)',
        transition: 'opacity 0.2s ease, transform 0.2s ease, background-color 0.2s ease, border-color 0.2s ease',
      }}>
        <InboxOutlined style={{ fontSize: '48px', color: 'var(--color-primary)' }} />
        <div style={{
          marginTop: '16px',
          fontSize: '16px',
          color: 'var(--text-primary)',
          fontWeight: 500,
        }}>
          释放文件以导入到 JayRead
        </div>
        <div style={{
          marginTop: '8px',
          fontSize: '13px',
          color: 'var(--text-tertiary)',
        }}>
          支持 PDF、BibTeX (.bib)、RIS (.ris)、CSL-JSON (.json) 格式
        </div>
      </div>
    </div>
  );
}
