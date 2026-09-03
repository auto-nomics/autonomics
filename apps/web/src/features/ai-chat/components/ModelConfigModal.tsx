/**
 * ModelConfigModal.jsx
 * JayRead AI Chat 自定义模型配置弹窗组件
 *
 * 核心功能：
 * - 自定义模型配置管理（添加/编辑/删除配置）
 * - API Key / Base URL 输入（自动清洗非 ASCII 字符）
 * - API 格式选择（OpenAI 兼容 / Anthropic）
 * - 多配置列表管理（新建/编辑/切换/删除）
 * - 全局设置：语义搜索开关、Web 搜索开关
 *
 * 使用 NiceModal 实现命令式调用。
 */

import React, { useEffect, useRef, useState, useCallback } from 'react';
import { PlusOutlined, DeleteOutlined } from '@ant-design/icons';
import { useTranslation } from 'react-i18next';
import { Button, Dropdown, App, Form, Input, Modal, Select, Switch, Tooltip } from 'antd';
import NiceModal, { useModal } from '@ebay/nice-modal-react';
import { formatCustomModelLabel, zoteroL10n } from '../zoteroL10n';

interface ModelConfigModalProps {
  customConfigs?: any[];
  // 允许返回 Promise<boolean>: 返回 false/抛错表示持久化失败,modal 需要回滚乐观更新。
  onSaveConfigs?: (configs: any[]) => Promise<boolean> | boolean | void;
  selectedConfigId?: string;
  onModelChange?: (model: any) => void;
  threadContextInjection?: boolean;
  onThreadContextInjectionToggle?: (enabled: boolean) => void;
}

const ModelConfigModal = NiceModal.create(({
  customConfigs = [],
  onSaveConfigs,
  selectedConfigId,
  onModelChange,
  threadContextInjection = false,
  onThreadContextInjectionToggle,
}: ModelConfigModalProps) => {
  const { t } = useTranslation('chat');
  const modal = useModal();
  const { modal: antdModal, message: antMessage } = App.useApp();
  const [form] = Form.useForm();

  const [internalCustomConfigs, setInternalCustomConfigs] = useState(customConfigs);
  const [editingConfigId, setEditingConfigId] = useState<string | null>(null);
  const [addFormFlash, setAddFormFlash] = useState(false);
  const apiFormatWatched = Form.useWatch('apiFormat', form);
  const apiFormatForUrlPlaceholder = apiFormatWatched ?? 'openai';
  // supportsVision 是 per-config 字段(跟当前编辑的自定义模型绑定),不是应用级开关;
  // 用 useWatch 监听让 global-setting-row 风格的 div 能随表单数据切换 enabled 状态。
  const [supportsVision, setSupportsVision] = useState(false);
  const [rotationEnabled, setRotationEnabled] = useState(true);
  const [localThreadInjection, setLocalThreadInjection] = useState(threadContextInjection);

  const cleanNonAscii = useCallback((value: string) => {
    if (!value) return { cleaned: '', hasNonAscii: false, removedCount: 0 };
    const cleaned = value.replace(/[^\x00-\x7F]/g, '');
    const removedCount = value.length - cleaned.length;
    return { cleaned, hasNonAscii: removedCount > 0, removedCount };
  }, []);

  const handleFieldChange = useCallback((formInstance: any, fieldName: string, label: string) => (e: any) => {
    const value = e?.target?.value ?? e;
    const { cleaned, hasNonAscii, removedCount } = cleanNonAscii(value);
    if (hasNonAscii) {
      formInstance.setFieldsValue({ [fieldName]: cleaned });
      antMessage.warning(`${label} 中包含 ${removedCount} 个不可见字符，已自动清除`);
    }
  }, [cleanNonAscii]);

  useEffect(() => {
    setInternalCustomConfigs(customConfigs);
  }, [customConfigs]);

  // 通过 ref 持有最新值,让"打开弹窗挑默认值"的逻辑只依赖 modal.visible,
  // 不会因为 internalCustomConfigs / selectedConfigId 变化(比如刚 save 完)而重跑。
  // 否则 NiceModal 把 selectedConfigId prop 冻结在 show 时的旧值,
  // 每次 save 后都会把 editingConfigId 强行覆盖回旧选中项,造成高亮跳回 + 表单清空。
  const internalConfigsRef = useRef(internalCustomConfigs);
  internalConfigsRef.current = internalCustomConfigs;
  const selectedConfigIdRef = useRef(selectedConfigId);
  selectedConfigIdRef.current = selectedConfigId;

  // 弹窗从关闭→打开时挑一个合理的 config 进入编辑态(优先 selectedConfigId,否则第一项)。
  // 依赖列表只放 modal.visible: 用户保存/删除/切换之后,editingConfigId 由对应 handler 维护,
  // 这里不再插手。
  useEffect(() => {
    if (!modal.visible) return;
    const list = internalConfigsRef.current;
    if (!list.length) {
      setEditingConfigId(null);
      form.resetFields();
      form.setFieldsValue({ apiFormat: 'openai' });
      setRotationEnabled(true);
      return;
    }
    const pick = list.find((c: any) => c.id === selectedConfigIdRef.current) || list[0];
    setEditingConfigId(pick.id);
    form.setFieldsValue({
      baseUrl: pick.baseUrl,
      apiKey: pick.apiKey,
      modelName: pick.modelName,
      apiFormat: pick.apiFormat || 'openai',
      supportsVision: pick.supportsVision === true,
    });
    setSupportsVision(pick.supportsVision === true);
    setRotationEnabled(pick.rotationEnabled !== false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modal.visible, form]);

  // 列表清空时(用户删掉最后一个 config)切到 add 模式。
  // 单独拆出来,不跟"挑默认值"耦合。
  useEffect(() => {
    if (!modal.visible) return;
    if (internalCustomConfigs.length === 0) {
      setEditingConfigId(null);
      form.resetFields();
      form.setFieldsValue({ apiFormat: 'openai' });
      setRotationEnabled(true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [internalCustomConfigs.length, modal.visible]);

  const handleSaveConfig = async () => {
    // 1. 校验。失败时 Antd 自动在字段上显示红字,这里直接 return。
    let values: any;
    try {
      values = await form.validateFields();
    } catch {
      return;
    }

    const wasEditing = !!editingConfigId;
    const { baseUrl: rawBaseUrl, apiKey: rawApiKey, modelName, apiFormat } = values;
    const baseUrl = rawBaseUrl.replace(/[^\x00-\x7F]/g, '');
    const apiKey = rawApiKey.replace(/[^\x00-\x7F]/g, '');

    const previousConfigs = internalCustomConfigs; // 持久化失败时回滚用
    const configs = [...internalCustomConfigs];
    const configToSave = {
      baseUrl,
      apiKey,
      modelName,
      apiFormat: apiFormat || 'openai',
      supportsVision: supportsVision,
      rotationEnabled: rotationEnabled,
    };

    let target: any;
    if (editingConfigId) {
      const idx = configs.findIndex((c: any) => c.id === editingConfigId);
      if (idx >= 0) {
        configs[idx] = { ...configs[idx], ...configToSave };
        target = configs[idx];
      }
    } else {
      const newCfg = {
        id: `custom-${Date.now()}`,
        ...configToSave
      };
      configs.push(newCfg);
      target = newCfg;
    }

    // 2. 乐观更新 UI,让用户立即看到列表里多了/改了这一项。
    setInternalCustomConfigs(configs);

    // 3. 持久化,失败回滚 + 不更新编辑态/不弹假成功提示。
    // 旧版本是 fire-and-forget 调用 onSaveConfigs,失败时 UI 已显示新状态但 DB/父组件没更新,
    // 重启就丢;这是用户实际遇到的"sensenova 没存进去"的根因之一。
    if (onSaveConfigs) {
      try {
        const ok = await onSaveConfigs(configs);
        if (ok === false) {
          // onSaveConfigs 内部已 message.error,这里只需回滚 UI。
          setInternalCustomConfigs(previousConfigs);
          return;
        }
      } catch (err: any) {
        setInternalCustomConfigs(previousConfigs);
        antMessage.error('保存配置失败: ' + (err?.message || String(err)));
        return;
      }
    }

    // 4. 持久化成功后再同步编辑态、刷新表单(可能因 ASCII 清洗而变化)、提示、通知父组件。
    if (target) {
      setEditingConfigId(target.id);
      form.setFieldsValue({
        baseUrl: target.baseUrl,
        apiKey: target.apiKey,
        modelName: target.modelName,
        apiFormat: target.apiFormat || 'openai',
        supportsVision: target.supportsVision === true,
      });
      setSupportsVision(target.supportsVision === true);
      setRotationEnabled(target.rotationEnabled !== false);
    } else {
      setEditingConfigId(null);
      form.resetFields();
      form.setFieldsValue({ apiFormat: 'openai' });
    }

    antMessage.success(zoteroL10n(wasEditing ? 'vibe-ai-chat-config-updated' : 'vibe-ai-chat-config-added'));

    if (onModelChange && configs.length > 0 && target) {
      onModelChange({
        key: 'custom',
        label: formatCustomModelLabel(target.modelName),
        configId: target.id,
        config: target
      });
    }
  };

  const handleDeleteConfig = (config: any) => {
    const displayName = getConfigDisplayName(config);
    antdModal.confirm({
      title: zoteroL10n('vibe-ai-chat-confirm-delete-title'),
      content: zoteroL10n('vibe-ai-chat-confirm-delete-body', { name: displayName }),
      okText: zoteroL10n('vibe-ai-chat-button-delete'),
      cancelText: zoteroL10n('general-cancel'),
      okButtonProps: { danger: true },
      centered: true,
      styles: { body: { textAlign: 'center' } },
      wrapClassName: 'ai-chat-modal',
      zIndex: 10002,
      onOk: async () => {
        const idToDelete = config.id;
        const previousConfigs = internalCustomConfigs; // 回滚用
        const previousEditingId = editingConfigId;     // 回滚用
        const configs = internalCustomConfigs.filter((c: any) => c.id !== idToDelete);

        // 乐观更新
        setInternalCustomConfigs(configs);
        if (editingConfigId === idToDelete) {
          if (configs.length === 0) {
            setEditingConfigId(null);
            form.resetFields();
            form.setFieldsValue({ apiFormat: 'openai' });
          } else {
            const next = configs[0];
            setEditingConfigId(next.id);
            form.setFieldsValue({
              baseUrl: next.baseUrl,
              apiKey: next.apiKey,
              modelName: next.modelName,
              apiFormat: next.apiFormat || 'openai',
              supportsVision: next.supportsVision === true,
      });
      setSupportsVision(next.supportsVision === true);
      setRotationEnabled(next.rotationEnabled !== false);
          }
        }

        // 持久化,失败回滚
        if (onSaveConfigs) {
          try {
            const ok = await onSaveConfigs(configs);
            if (ok === false) {
              setInternalCustomConfigs(previousConfigs);
              setEditingConfigId(previousEditingId);
              return;
            }
          } catch (err: any) {
            setInternalCustomConfigs(previousConfigs);
            setEditingConfigId(previousEditingId);
            antMessage.error('删除配置失败: ' + (err?.message || String(err)));
            return;
          }
        }

        antMessage.success(zoteroL10n('vibe-ai-chat-config-deleted'));
      }
    });
  };

  const handleAddConfig = () => {
    const alreadyOnAddPage = editingConfigId === null;
    setEditingConfigId(null);
    form.resetFields();
    form.setFieldsValue({ apiFormat: 'openai' });
    if (alreadyOnAddPage) {
      setAddFormFlash(false);
      requestAnimationFrame(() => {
        setAddFormFlash(true);
        window.setTimeout(() => setAddFormFlash(false), 550);
      });
    }
  };

  const handleSelectConfigToEdit = (config: any) => {
    setEditingConfigId(config.id);
    form.setFieldsValue({
      baseUrl: config.baseUrl,
      apiKey: config.apiKey,
      modelName: config.modelName,
      apiFormat: config.apiFormat || 'openai',
      supportsVision: config.supportsVision === true,
    });
    setSupportsVision(config.supportsVision === true);
    setRotationEnabled(config.rotationEnabled !== false);
  };

  const getConfigDisplayName = (c: any) => c.modelName || c.name || zoteroL10n('vibe-ai-chat-unnamed');

  const handleCancel = () => {
    setEditingConfigId(null);
    form.resetFields();
    modal.hide();
  };

  return (
    <Modal
      title={zoteroL10n('vibe-ai-chat-custom-model-settings-title')}
      open={modal.visible}
      closeIcon={null}
      onCancel={handleCancel}
      footer={null}
      destroyOnHidden
      maskClosable={false}
      zIndex={10001}
      centered
      getContainer={false}
      wrapClassName="ai-chat-modal model-config-modal"
      width={540}
    >
      <div className="model-config-content" onContextMenu={(e) => e.stopPropagation()}>
        <div className="model-config-list">
          <div className="model-config-list-body">
            {internalCustomConfigs.map((c: any, idx: number) => (
              <Dropdown
                key={c.id}
                menu={{
                  items: [
                    {
                      key: 'delete',
                      label: zoteroL10n('vibe-ai-chat-button-delete'),
                      danger: true,
                      icon: <DeleteOutlined />,
                      onClick: () => handleDeleteConfig(c),
                    }
                  ]
                }}
                trigger={['contextMenu']}
              >
                <div
                  onClick={() => handleSelectConfigToEdit(c)}
                  className={`model-config-item${editingConfigId === c.id ? ' active' : ''}${idx === internalCustomConfigs.length - 1 ? ' last' : ''}`}
                >
                  <span className="model-config-item-name">
                    {getConfigDisplayName(c)}
                  </span>
                </div>
              </Dropdown>
            ))}
          </div>
          <div style={{ display: 'flex', gap: 8, marginTop: 'auto', paddingTop: 12, overflow: 'hidden' }}>
            <Button
              style={{ flex: 1, minWidth: 0, overflow: 'hidden' }}
              onClick={handleAddConfig}
            >
              {zoteroL10n('vibe-ai-chat-button-add')}
            </Button>
            <Button
              style={{ flex: 1, minWidth: 0, overflow: 'hidden' }}
              onClick={handleSaveConfig}
            >
              {zoteroL10n('vibe-ai-chat-button-update')}
            </Button>
            <Button
              style={{ flex: 1, minWidth: 0, overflow: 'hidden' }}
              onClick={handleCancel}
            >
              {zoteroL10n('vibe-ai-chat-button-close')}
            </Button>
          </div>
        </div>

        <div className={addFormFlash ? 'model-config-form custom-config-form-flash' : 'model-config-form'}>
          <Form form={form} layout="vertical" requiredMark={false} initialValues={{ apiFormat: 'openai' }}>
            <div className="base-url-inline-row">
              <Form.Item name="apiFormat" noStyle initialValue="openai" rules={[{ required: true }]}>
                <Select
                  style={{ minWidth: 60, flexShrink: 0, width: 'auto' }}
                  popupMatchSelectWidth={false}
                  suffixIcon={null}
                  options={[
                    { value: 'openai', label: zoteroL10n('vibe-ai-chat-api-format-label-openai') },
                    { value: 'anthropic', label: zoteroL10n('vibe-ai-chat-api-format-label-anthropic') }
                  ]}
                />
              </Form.Item>
              <Form.Item name="baseUrl" noStyle rules={[{ required: true }]}>
                <Input
                  placeholder={
                    apiFormatForUrlPlaceholder === 'anthropic'
                      ? zoteroL10n('vibe-ai-chat-api-base-url-placeholder-anthropic')
                      : zoteroL10n('vibe-ai-chat-api-base-url-placeholder-openai')
                  }
                  onChange={handleFieldChange(form, 'baseUrl', 'Base URL')}
                />
              </Form.Item>
            </div>
            <div className="api-key-inline-row">
              <Form.Item
                name="apiKey"
                rules={[{ required: true }]}
                noStyle
              >
                <Input.Password
                  placeholder={zoteroL10n('vibe-ai-chat-api-key-placeholder')}
                  autoComplete="off"
                  visibilityToggle={false}
                  onChange={handleFieldChange(form, 'apiKey', 'API Key')}
                />
              </Form.Item>
            </div>
            <div className="model-name-inline-row">
              <Form.Item
                name="modelName"
                rules={[{ required: true }]}
                noStyle
              >
                <Input placeholder={zoteroL10n('vibe-ai-chat-model-name-placeholder')} />
              </Form.Item>
            </div>
          </Form>

          <div className="model-config-global-settings">
            <Tooltip title="仅对当前自定义模型生效。启用后，此模型被标记为支持图片输入。">
              <div
                className={`global-setting-row global-setting-clickable${supportsVision ? ' model-toggle-active' : ''}`}
                onClick={() => setSupportsVision(!supportsVision)}
              >
                <span className="global-setting-label">
                  支持图片
                </span>
              </div>
            </Tooltip>
            <Tooltip title="启用后，此模型加入批量整理的模型轮转池。当当前模型 429 限额超限时，自动切换到其他启用的模型。">
              <div
                className={`global-setting-row global-setting-clickable${rotationEnabled ? ' model-toggle-active' : ''}`}
                onClick={() => setRotationEnabled(!rotationEnabled)}
              >
                <span className="global-setting-label">
                  模型轮转
                </span>
              </div>
            </Tooltip>
            <Tooltip title="启用后，AI 主对话会自动包含追问及回复摘要，让对话更连贯。">
              <div
                className={`global-setting-row global-setting-clickable${localThreadInjection ? ' model-toggle-active' : ''}`}
                onClick={() => { const v = !localThreadInjection; setLocalThreadInjection(v); if (onThreadContextInjectionToggle) onThreadContextInjectionToggle(v); }}
              >
                <span className="global-setting-label">
                  线程上下文注入
                </span>
              </div>
            </Tooltip>
          </div>
        </div>
      </div>
    </Modal>
  );
});

export default ModelConfigModal;
