/**
 * JayRead 模板管理弹窗组件（NiceModal 版）
 *
 * 使用 NiceModal 实现命令式调用。
 * 提供 Tab 1: 动作模板, Tab 2: 系统提示词, Tab 3: 上下文预览
 */

import React, { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { Modal, Button, Form, Input, Checkbox, Tooltip, App, Flex, Tabs, Tag } from 'antd';
import { PlusOutlined, DeleteOutlined, EditOutlined, UndoOutlined } from '@ant-design/icons';
import { useTranslation } from 'react-i18next';
import NiceModal, { useModal } from '@ebay/nice-modal-react';
import { getDefaultTemplates } from './ContextBridge';
import type { AgentType } from '../features/ai-chat/agentTypes';
import { DEFAULT_PERSONAS } from '../features/ai-chat/agentTypes';

interface TemplateManagerModalProps {
  templateData: { templates: any[] };
  onSave?: (data: { templates: any[] }) => void;
  agentType: AgentType;
  customSystemPrompt?: string;
  onCustomSystemPromptSave?: (prompt: string) => void;
  contextPreviewData?: Array<{ key: string; label: string; active: boolean; details: string[] }>;
}

const TemplateManagerModal = NiceModal.create(({
  templateData,
  onSave,
  agentType,
  customSystemPrompt = '',
  onCustomSystemPromptSave,
  contextPreviewData = [],
}: TemplateManagerModalProps) => {
  const { message } = App.useApp();
  const { t } = useTranslation('template');
  const modal = useModal();

  const CONTEXT_TYPE_OPTIONS = useMemo(() => [
    { value: 'pdf_selection', label: t('scenes.pdfSelection') },
  ], [t]);

  const getContextTypeShortLabel = useCallback((type: string) => {
    if (type === 'pdf_selection') return 'PDF';
    return type;
  }, [t]);

  const [internalData, setInternalData] = useState<{ templates: any[] }>({ templates: [] });
  const [editingItem, setEditingItem] = useState<{ templateId: string } | { isNew: true } | null>(null);
  const [selectedContextTypes, setSelectedContextTypes] = useState<string[]>([]);
  const [form] = Form.useForm();
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; templateId: string } | null>(null);
  const contextMenuRef = useRef<HTMLDivElement>(null);
  const [editingSystemPrompt, setEditingSystemPrompt] = useState('');
  const [activeTab, setActiveTab] = useState('templates');

  useEffect(() => {
    if (modal.visible) {
      const cloned = {
        templates: (templateData?.templates || []).map((t: any) => ({ ...t })),
      };
      setInternalData(cloned);
      setEditingItem(null);
      form.resetFields();
      setEditingSystemPrompt(customSystemPrompt || '');
      setActiveTab('templates');
    }
  }, [modal.visible, templateData, form, customSystemPrompt]);

  const handleSelectTemplate = useCallback((templateId: string) => {
    setEditingItem({ templateId });
  }, []);

  const handleAddNew = useCallback(() => {
    setEditingItem({ isNew: true });
  }, []);

  useEffect(() => {
    if (!editingItem) {
      setSelectedContextTypes([]);
      return;
    }
    if ((editingItem as any).templateId) {
      const tpl = internalData.templates.find((t: any) => t.id === (editingItem as any).templateId);
      if (tpl) {
        form.setFieldsValue({ name: tpl.label, templateText: tpl.template });
        setSelectedContextTypes(tpl.contextTypes || []);
      }
    } else if ((editingItem as any).isNew) {
      form.resetFields();
      setSelectedContextTypes(['pdf_selection']);
    }
  }, [editingItem]);

  const handleContextMenu = useCallback((templateId: string, e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setContextMenu({ x: e.clientX, y: e.clientY, templateId });
  }, []);

  const handleCloseContextMenu = useCallback(() => setContextMenu(null), []);

  const handleContextMenuDelete = useCallback(() => {
    if (contextMenu?.templateId) {
      const newTemplates = internalData.templates.filter((t: any) => t.id !== contextMenu.templateId);
      setInternalData(prev => ({ ...prev, templates: newTemplates }));
      if ((editingItem as any)?.templateId === contextMenu.templateId) {
        setEditingItem(null);
        form.resetFields();
      }
      if (onSave) onSave({ templates: newTemplates });
    }
    setContextMenu(null);
  }, [contextMenu, internalData.templates, editingItem, form, onSave]);

  const handleResetBuiltin = useCallback(() => {
    if (!(editingItem as any)?.templateId) return;
    const templateId = (editingItem as any).templateId;
    const defaults = getDefaultTemplates();
    const defaultTpl = defaults.find((t: any) => t.id === templateId);
    if (!defaultTpl) return;
    const newTemplates = internalData.templates.map((t: any) =>
      t.id === templateId ? { ...t, template: defaultTpl.template } : t
    );
    setInternalData(prev => ({ ...prev, templates: newTemplates }));
    form.setFieldsValue({ templateText: defaultTpl.template });
  }, [editingItem, internalData.templates, form]);

  const handleSave = useCallback(() => {
    if (!selectedContextTypes || selectedContextTypes.length === 0) {
      message.warning(t('selectScene'));
      return;
    }
    form.validateFields().then((values: any) => {
      const { name, templateText } = values;
      const contextTypes = selectedContextTypes;
      let newTemplates = [...internalData.templates];
      if ((editingItem as any)?.templateId) {
        newTemplates = internalData.templates.map((t: any) =>
          t.id === (editingItem as any).templateId ? { ...t, label: name, template: templateText, contextTypes } : t
        );
      } else if ((editingItem as any)?.isNew) {
        newTemplates = [...internalData.templates, {
          id: `custom_${Date.now()}`, label: name, template: templateText, contextTypes, isBuiltin: false,
        }];
      }
      const newData = { templates: newTemplates };
      setInternalData(newData);
      if (onSave) onSave(newData);
      message.success(t('templateSaved'));
    }).catch(() => {});
  }, [form, editingItem, internalData, onSave, selectedContextTypes]);

  const handleSystemPromptSave = useCallback(() => {
    if (onCustomSystemPromptSave) {
      onCustomSystemPromptSave(editingSystemPrompt);
      message.success(t('systemPromptSaved'));
    }
  }, [editingSystemPrompt, onCustomSystemPromptSave]);

  const handleSystemPromptReset = useCallback(() => setEditingSystemPrompt(''), []);

  const isBuiltinModified = useCallback((templateId: string) => {
    const tpl = internalData.templates.find((t: any) => t.id === templateId);
    if (!tpl || !tpl.isBuiltin) return false;
    const defaults = getDefaultTemplates();
    const defaultTpl = defaults.find((t: any) => t.id === templateId);
    return defaultTpl && tpl.template !== defaultTpl.template;
  }, [internalData.templates]);

  const handleClose = useCallback(() => {
    if (onSave) onSave({ templates: internalData.templates });
    setEditingItem(null);
    form.resetFields();
    modal.hide();
  }, [form, internalData.templates, onSave]);

  const templatesTabContent = (
    <Flex gap="middle" align="flex-start" style={{ marginBottom: 0 }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{
          border: '1px solid var(--border-color, #f0f0f0)', borderRadius: 6,
          maxHeight: 340, overflowY: 'auto', background: 'var(--bg-primary)',
        }}>
          {internalData.templates.length === 0 ? (
            <div style={{ padding: '16px 12px', textAlign: 'center', color: 'var(--text-tertiary, #8c8c8c)', fontSize: 12 }}>
              {t('noTemplates')}
            </div>
          ) : (
            internalData.templates.map((tpl: any, idx: number) => (
              <div
                key={tpl.id}
                onClick={() => handleSelectTemplate(tpl.id)}
                onContextMenu={(e) => handleContextMenu(tpl.id, e)}
                style={{
                  display: 'flex', alignItems: 'center', padding: '8px 12px',
                  borderBottom: idx < internalData.templates.length - 1 ? '1px solid var(--border-color, #f0f0f0)' : 'none',
                  cursor: 'pointer',
                  background: (editingItem as any)?.templateId === tpl.id ? 'var(--bg-secondary)' : 'transparent',
                  gap: 8,
                }}
              >
                <EditOutlined style={{ fontSize: 12, color: 'var(--text-tertiary)' }} />
                <span style={{ fontSize: 13, flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {tpl.label}
                </span>
                {tpl.isBuiltin && (
                  <span style={{ fontSize: 9, color: 'var(--color-primary)', border: '1px solid var(--color-primary)', borderRadius: 3, padding: '0 4px', flexShrink: 0, lineHeight: '16px' }}>
                    {t('builtin')}
                  </span>
                )}
                {tpl.isBuiltin && isBuiltinModified(tpl.id) && (
                  <span style={{ width: 6, height: 6, borderRadius: '50%', background: 'var(--color-primary)', flexShrink: 0 }} />
                )}
                <span style={{ fontSize: 10, color: 'var(--text-tertiary, #8c8c8c)', flexShrink: 0 }}>
                  {(tpl.contextTypes || []).map((ct: string) => getContextTypeShortLabel(ct)).join('、')}
                </span>
              </div>
            ))
          )}
        </div>
        <Button type="dashed" icon={<PlusOutlined />} onClick={handleAddNew} style={{ marginTop: 8, width: '100%' }}>
          {t('addTemplate')}
        </Button>
      </div>
      <div style={{ flex: 1.2, minWidth: 0 }}>
        {editingItem ? (
          <Form form={form} layout="vertical">
            <Form.Item label="" name="name" rules={[{ required: true, message: t('inputName') }]} style={{ marginBottom: 8 }}>
              <Input placeholder={t('templateNamePlaceholder')} />
            </Form.Item>
            <div style={{ marginBottom: 8 }}>
              <Checkbox.Group options={CONTEXT_TYPE_OPTIONS} value={selectedContextTypes} onChange={setSelectedContextTypes} />
            </div>
            <Form.Item label="" name="templateText" style={{ marginBottom: 8 }} rules={[{ required: true, message: t('inputContent') }]}>
              <Input.TextArea rows={5} placeholder={t('templateTextPlaceholder')} style={{ fontFamily: 'monospace', fontSize: 12 }} />
            </Form.Item>
            <div style={{ padding: '8px 12px', background: 'var(--bg-elevated, rgba(0,0,0,0.04))', borderRadius: 4, fontSize: 11, color: 'var(--text-tertiary)', lineHeight: 1.8 }}>
              <div style={{ fontWeight: 500, marginBottom: 2 }}>{t('placeholders')}</div>
              <div>{t('placeholderContent')}</div>
              <div>{t('placeholderTitle')}</div>
              <div style={{ fontWeight: 500, marginTop: 4, marginBottom: 2 }}>{t('sceneSpecific')}</div>
              <div>{t('placeholderPage')}</div>
              <div>{t('placeholderSummary')}</div>
              <div style={{ marginTop: 4, fontStyle: 'italic', opacity: 0.8 }}>{t('tipReuse')}</div>
            </div>
            {(editingItem as any).templateId && (() => {
              const tpl = internalData.templates.find((t: any) => t.id === (editingItem as any).templateId);
              return tpl?.isBuiltin && isBuiltinModified((editingItem as any).templateId);
            })() && (
              <div style={{ marginTop: 8, textAlign: 'right' }}>
                <Tooltip title={t('tooltipRestoreDefault')}>
                  <Button type="link" size="small" icon={<UndoOutlined />} onClick={handleResetBuiltin} style={{ fontSize: 12, color: 'var(--text-tertiary)' }}>
                    {t('restoreDefault')}
                  </Button>
                </Tooltip>
              </div>
            )}
          </Form>
        ) : (
          <div style={{ height: 200, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', color: 'var(--text-tertiary, #8c8c8c)', border: '1px dashed var(--border-color, #e0e0e0)', borderRadius: 6 }}>
            <EditOutlined style={{ fontSize: 24, marginBottom: 8 }} />
            <div style={{ fontSize: 13 }}>{t('clickToEdit')}</div>
          </div>
        )}
      </div>
    </Flex>
  );

  const systemPromptTabContent = (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      <div style={{ padding: '8px 12px', background: 'var(--bg-elevated, rgba(0,0,0,0.04))', borderRadius: 4, fontSize: 11, color: 'var(--text-tertiary)', lineHeight: 1.8 }}>
        <div>{t('customPromptHint', { defaultPrompt: DEFAULT_PERSONAS[agentType] })}</div>
        <div>{t('systemPromptTip')}</div>
      </div>
      <Input.TextArea
        value={editingSystemPrompt}
        onChange={(e) => setEditingSystemPrompt(e.target.value)}
        rows={6}
        placeholder={`${t('defaultPrompt')}\n\n${DEFAULT_PERSONAS[agentType]}`}
        style={{ fontFamily: 'monospace', fontSize: 12, resize: 'vertical' }}
      />
      <div style={{ textAlign: 'right' }}>
        <Tooltip title={t('resetToDefault')}>
          <Button type="link" size="small" icon={<UndoOutlined />} onClick={handleSystemPromptReset} style={{ fontSize: 12, color: 'var(--text-tertiary)' }}>
            {t('restoreDefault')}
          </Button>
        </Tooltip>
      </div>
    </div>
  );

  const contextPreviewTabContent = (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div style={{ padding: '8px 12px', background: 'var(--bg-elevated, rgba(0,0,0,0.04))', borderRadius: 4, fontSize: 11, color: 'var(--text-tertiary)', lineHeight: 1.8 }}>
        {t('contextExplanation')}
      </div>
      {contextPreviewData.map(layer => (
        <div key={layer.key} style={{
          padding: '10px 12px',
          background: layer.active ? 'var(--bg-primary)' : 'transparent',
          border: `1px solid ${layer.active ? 'var(--border-color)' : 'var(--border-color-light, #f0f0f0)'}`,
          borderRadius: 6, opacity: layer.active ? 1 : 0.5,
        }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: layer.details.length > 0 ? 4 : 0 }}>
            <Tag color={layer.active ? 'blue' : 'default'} style={{ fontSize: 10, margin: 0, lineHeight: '18px' }}>{layer.key}</Tag>
            <span style={{ fontSize: 13, fontWeight: 500, color: 'var(--text-primary)' }}>{layer.label}</span>
            <span style={{ fontSize: 11, color: layer.active ? 'var(--status-success)' : 'var(--text-tertiary)' }}>
              {layer.active ? t('common:enabled') : t('common:disabled')}
            </span>
          </div>
          {layer.details.map((detail, i) => (
            <div key={i} style={{ fontSize: 12, color: 'var(--text-secondary, #595959)', marginLeft: 32 }}>{detail}</div>
          ))}
        </div>
      ))}
    </div>
  );

  return (
    <Modal
      title={t('title')}
      open={modal.visible}
      onCancel={handleClose}
      closable={false}
      destroyOnHidden
      zIndex={10001}
      centered
      getContainer={false}
      wrapClassName="ai-chat-modal"
      width={680}
      footer={
        <Flex justify="flex-end" gap={8}>
          <Button onClick={handleClose}>{t('common:close')}</Button>
          {activeTab === 'templates' && editingItem && (
            <Button type="primary" onClick={handleSave}>
              {(editingItem as any)?.isNew ? t('common:add') : t('common:save')}
            </Button>
          )}
          {activeTab === 'systemPrompt' && (
            <Button type="primary" onClick={handleSystemPromptSave}>{t('common:save')}</Button>
          )}
        </Flex>
      }
    >
      <Tabs
        activeKey={activeTab}
        onChange={setActiveTab}
        size="small"
        items={[
          { key: 'templates', label: t('actionTemplates'), children: templatesTabContent },
          { key: 'systemPrompt', label: t('systemPrompt'), children: systemPromptTabContent },
          { key: 'contextPreview', label: t('contextPreview'), children: contextPreviewTabContent },
        ]}
      />
      {contextMenu && (
        <div ref={contextMenuRef} onClick={handleCloseContextMenu} onContextMenu={(e) => { e.preventDefault(); handleCloseContextMenu(); }}
          style={{ position: 'fixed', top: 0, left: 0, right: 0, bottom: 0, zIndex: 10002 }}>
          <div onClick={(e) => e.stopPropagation()} onContextMenu={(e) => e.stopPropagation()}
            style={{ position: 'fixed', top: contextMenu.y, left: contextMenu.x, background: 'var(--bg-primary, #fff)', border: '1px solid var(--border-color, #e0e0e0)', borderRadius: 6, boxShadow: '0 4px 12px rgba(0,0,0,0.15)', padding: '4px 0', minWidth: 120, zIndex: 10003 }}>
            <div onClick={handleContextMenuDelete}
              style={{ padding: '6px 16px', fontSize: 13, color: 'var(--danger-text)', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 8, transition: 'background 0.15s' }}
              onMouseEnter={(e) => { e.currentTarget.style.background = 'var(--bg-secondary, rgba(0,0,0,0.04))'; }}
              onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}>
              <DeleteOutlined style={{ fontSize: 12 }} />{t('deleteTemplate')}
            </div>
          </div>
        </div>
      )}
    </Modal>
  );
});

export default TemplateManagerModal;
