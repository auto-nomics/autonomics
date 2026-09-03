/**
 * 元数据视图组件
 *
 * 当论文为题录导入记录（无 PDF 文件）时显示此视图，替代 PDF 阅读器。
 * 功能特性：
 * - 显示论文的完整元数据字段（标题、作者、摘要、DOI、期刊等）
 * - 根据文献类型（journal/conference/thesis 等）动态显示对应字段
 * - 上传 PDF 按钮：为题录记录手动关联 PDF 文件
 * - DOI 下载按钮：通过 DOI 自动下载开放获取 PDF
 * - 支持深色模式（通过 CSS 变量）
 *
 * @module PaperReaderPage/components/MetadataView
 */
import React from 'react';
import { useTranslation } from 'react-i18next';
import { Button, Tag, Space } from 'antd';
import { UploadOutlined, CloudDownloadOutlined, CopyOutlined } from '@ant-design/icons';
import { attachPdf, downloadPdf } from '../../../services/bibliographyApi';
import { getTypeLabel, getTypeColor, getTypeFields, METADATA_FIELD_LABELS } from '../../../utils/literatureTypes';
import { useCopyCitation } from '../../../hooks/useCopyCitation';
import './MetadataView.css';

/**
 * 元数据视图组件
 *
 * @param {Object} props - 组件属性
 * @param {Object} props.paper - 论文对象
 * @param {number} props.paperId - 论文 ID
 * @param {Function} props.reloadPaper - 重新加载论文数据的回调
 * @param {Function} props.message - Ant Design message 对象（用于显示提示）
 */
export default function MetadataView({ paper, paperId, reloadPaper, message }: any) {
  const { t } = useTranslation('paperReader');
  // 复制引用（共享 hook，注意 message 实例由本组件接收，但 hook 内部会从 App.useApp 取自己的实例）
  const copyCitation = useCopyCitation();
  // 处理上传附件
  const handleUploadAttachment = () => {
    const input = document.createElement('input');
    input.type = 'file';
    input.onchange = async (ev: any) => {
      const file = ev.target?.files?.[0];
      if (file) {
        try {
          const result = await attachPdf(paperId, file);
          message.success(result.message || t('metadata.uploadSuccess'));
          await reloadPaper();
        } catch (err) {
          message.error(t('metadata.uploadFailed', { error: (err as any).message }));
        }
      }
    };
    input.click();
  };

  // 处理自动下载 PDF
  const handleDownloadPdf = async () => {
    try {
      const result = await downloadPdf(paperId);
      if (result.success) {
        message.success(result.message);
        await reloadPaper();
      } else {
        message.warning(result.message);
      }
    } catch (err) {
      message.error(t('metadata.downloadFailed', { error: (err as any).message }));
    }
  };

  // 渲染额外元数据字段
  const renderExtraMetadata = () => {
    if (!paper.extra_metadata || Object.keys(paper.extra_metadata).length === 0) {
      return null;
    }

    const fields = getTypeFields(paper.paper_type);
    const items = [];

    // 遍历类型特有字段
    fields.forEach((field) => {
      const value = paper.extra_metadata[field];
      if (value) {
        const label = (METADATA_FIELD_LABELS as any)[field] || field;
        items.push(`${label}: ${value}`);
      }
    });

    // 通用字段：keywords
    if (paper.extra_metadata.keywords) {
      items.push(`${(METADATA_FIELD_LABELS as any).keywords}: ${paper.extra_metadata.keywords}`);
    }

    // 通用字段：url
    if (paper.extra_metadata.url) {
      items.push(
        <span key="url">
          {(METADATA_FIELD_LABELS as any).url}:{' '}
          <a href={paper.extra_metadata.url} target="_blank" rel="noopener noreferrer">
            {paper.extra_metadata.url}
          </a>
        </span>
      );
    }

    if (items.length === 0) {
      return null;
    }

    return (
      <div className="metadata-view__extra-metadata">
        <div className="metadata-view__metadata-items">
          {items.map((item: any, index: any) => (
            <span key={index}>{item}</span>
          ))}
        </div>
      </div>
    );
  };

  // 渲染作者列表
  const renderAuthors = () => {
    if (!paper.authors || paper.authors.length === 0) return null;

    const list = Array.isArray(paper.authors) ? paper.authors : [paper.authors];
    const authors = list.map((a: any, i: any) => (
      <span key={i}>
        {i > 0 && ', '}
        <span className="metadata-view__author">{typeof a === 'string' ? a.trim() : a}</span>
      </span>
    ));

    return <p className="metadata-view__authors">{authors}</p>;
  };

  // 渲染期刊信息
  const renderJournalInfo = () => {
    if (!paper.journal_name && !paper.publication_year) return null;

    return (
      <p className="metadata-view__journal">
        {paper.journal_name && (
          <span className="metadata-view__journal-name">{paper.journal_name}</span>
        )}
        {paper.journal_name && paper.publication_year && ' · '}
        {paper.publication_year && <span>{paper.publication_year}</span>}
        {paper.doi && (
          <span className="metadata-view__doi">DOI: {paper.doi}</span>
        )}
      </p>
    );
  };

  // 渲染类型标签
  const renderTypeTag = () => {
    if (!paper.paper_type) return null;

    return (
      <div className="metadata-view__type-tag">
        <Tag color={getTypeColor(paper.paper_type)} style={{ marginRight: 8 }}>
          {getTypeLabel(paper.paper_type)}
        </Tag>
      </div>
    );
  };

  return (
    <div className="metadata-view">
      <div className="metadata-view__card">
        {/* 论文标题 */}
        <h2 className="metadata-view__title">{paper.title || t('metadata.unnamed')}</h2>

        {/* 作者列表 */}
        {renderAuthors()}

        {/* 期刊信息 */}
        {renderJournalInfo()}

        {/* 类型标签 */}
        {renderTypeTag()}

        {/* 额外元数据 */}
        {renderExtraMetadata()}

        {/* 摘要 */}
        {paper.abstract && (
          <div className="metadata-view__abstract">
            <strong className="metadata-view__abstract-label">{t('metadata.abstract')}</strong>
            {paper.abstract}
          </div>
        )}

        {/* 操作按钮 */}
        <div className="metadata-view__actions">
          <Space direction="vertical" style={{ width: '100%' }}>
            <Button
              type="primary"
              size="large"
              icon={<UploadOutlined />}
              onClick={handleUploadAttachment}
              block
            >
              {t('metadata.uploadAttachment')}
            </Button>
            <Button
              size="large"
              icon={<CopyOutlined />}
              onClick={() => copyCitation(paperId)}
              block
            >
              {t('metadata.copyCitation', { defaultValue: '复制引用' })}
            </Button>
            {paper.doi && (
              <Button
                size="large"
                icon={<CloudDownloadOutlined />}
                onClick={handleDownloadPdf}
                block
              >
                {t('metadata.autoDownloadPdf')}
              </Button>
            )}
          </Space>
        </div>
      </div>
    </div>
  );
}
