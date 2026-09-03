/**
 * 重复文献解决对话框组件
 *
 * 使用 NiceModal 实现命令式调用，支持 await 获取解决结果。
 * 数据来自 /bibliography/import 响应的 duplicate_details，
 * 确认后产出 DuplicateResolution[] 交给 hook 提交 /bibliography/resolve-duplicates。
 */

import { useState } from 'react'
import { Modal, Table, Select, Tag, Space, Button } from 'antd'
import { FileTextOutlined, SwapOutlined, CopyOutlined } from '@ant-design/icons'
import { useTranslation } from 'react-i18next'
import NiceModal, { useModal } from '@ebay/nice-modal-react'
import type { DuplicateDetail, DuplicateResolution, ResolutionAction } from '@/types';

const DuplicateResolutionModal = NiceModal.create(({
  duplicates,
}: {
  duplicates: DuplicateDetail[];
}) => {
  // 每条重复的选择，按 DuplicateDetail.id 索引
  const [resolutions, setResolutions] = useState<Record<string, ResolutionAction>>({})
  const { t } = useTranslation('paperList')
  const modal = useModal();

  const handleChange = (id: string, action: ResolutionAction) => {
    setResolutions(prev => ({ ...prev, [id]: action }))
  }

  const handleSetAll = (action: ResolutionAction) => {
    const newResolutions: Record<string, ResolutionAction> = {}
    duplicates.forEach(dup => { newResolutions[dup.id] = action })
    setResolutions(newResolutions)
  }

  const handleConfirm = () => {
    const result: DuplicateResolution[] = duplicates.map(dup => ({
      id: dup.id,
      action: resolutions[dup.id] || 'skip',
      existing_item_id: dup.existing_item_id,
      record: dup.record,
    }))
    modal.resolve(result);
    modal.hide();
    setResolutions({})
  }

  const handleCancel = () => {
    modal.resolve(null);
    modal.hide();
    setResolutions({})
  }

  const columns = [
    {
      title: t('duplicate.index'),
      key: 'index',
      width: 60,
      render: (_: unknown, _record: DuplicateDetail, idx: number) => `#${idx + 1}`,
    },
    {
      title: t('duplicate.importTitle'),
      dataIndex: 'title',
      key: 'importTitle',
      ellipsis: true,
      render: (text: string) => text || t('duplicate.noTitle'),
    },
    {
      title: t('duplicate.existingTitle'),
      dataIndex: 'existing_title',
      key: 'existingTitle',
      ellipsis: true,
      render: (text: string) => text || t('duplicate.noTitle'),
    },
    {
      title: t('duplicate.matchReason'),
      dataIndex: 'reason',
      key: 'reason',
      width: 100,
      render: (reason: string) => {
        // 后端 reason 形如 "DOI match: 10.xxxx" / "Title match: xxx"
        if (reason.startsWith('DOI')) return <Tag color="blue">{t('duplicate.doiMatch')}</Tag>
        return <Tag color="orange">{t('duplicate.titleMatch')}</Tag>
      },
    },
    {
      title: t('common:operation'),
      key: 'action',
      width: 140,
      render: (_: unknown, record: DuplicateDetail) => (
        <Select
          value={resolutions[record.id] || 'skip'}
          onChange={(value) => handleChange(record.id, value)}
          size="small"
          style={{ width: '100%' }}
          options={[
            { value: 'skip', label: <><FileTextOutlined /> {t('duplicate.skip')}</> },
            { value: 'replace', label: <><SwapOutlined /> {t('duplicate.overwrite')}</> },
            { value: 'keep_both', label: <><CopyOutlined /> {t('duplicate.keepBoth')}</> },
          ]}
        />
      ),
    },
  ]

  return (
    <Modal
      title={t('duplicate.title')}
      open={modal.visible}
      onCancel={handleCancel}
      width={900}
      footer={
        <Space>
          <Button size="small" onClick={() => handleSetAll('skip')}>{t('duplicate.skipAll')}</Button>
          <Button size="small" onClick={() => handleSetAll('replace')}>{t('duplicate.overwriteAll')}</Button>
          <Button size="small" onClick={() => handleSetAll('keep_both')}>{t('duplicate.keepAll')}</Button>
          <div style={{ flex: 1 }} />
          <Button onClick={handleCancel}>{t('duplicate.cancelImport')}</Button>
          <Button type="primary" onClick={handleConfirm}>{t('common:confirmAction')}</Button>
        </Space>
      }
    >
      <p style={{ marginBottom: 16, color: 'var(--text-secondary)' }}>
        {t('duplicate.description', { count: duplicates.length })}
      </p>
      <Table
        dataSource={duplicates}
        columns={columns}
        rowKey="id"
        pagination={false}
        size="small"
        scroll={{ y: 400 }}
      />
    </Modal>
  )
})

export default DuplicateResolutionModal;
