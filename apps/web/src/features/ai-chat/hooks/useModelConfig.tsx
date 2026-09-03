/**
 * AI 聊天模型配置管理 Hook —— Phase 4 store thin wrapper
 *
 * 自 Phase 4 起，状态由 useChatStore（zustand）承载；本 hook 退化为 thin wrapper：
 * - 从 store 订阅 currentModel/customConfigs/selectedConfigId
 * - 保留副作用：updateSettings API 调用 + antd message
 * - 保留 useMemo 计算属性（displayModel/dropdownModelItems/...）
 *
 * 调用方（ChatPanel）签名完全不变。
 *
 * @module ai-chat/hooks/useModelConfig
 */

import { useCallback, useMemo } from 'react';
import { App } from 'antd';
import { updateSettings } from '../../../services/settingsApi';
import { useChatStore } from '../../../stores/useChatStore';
import { formatCustomModelLabel } from '../zoteroL10n';

/** 模型选择 */
export interface ModelSelection {
  key: string;
  label: string;
  configId?: string;
  config?: CustomModelConfig;
}

/** 自定义模型配置 */
export interface CustomModelConfig {
  id: string;
  name: string;
  modelName: string;
  baseUrl: string;
  apiKey: string;
  apiFormat: string;
  /** 用户在自定义配置 UI 勾选「支持图片」后为 true,会作为 supports_vision 传给后端 */
  supportsVision?: boolean;
}

interface UseModelConfigParams {
  settings: Record<string, unknown>;
}

export function useModelConfig({ settings }: UseModelConfigParams) {
  const { message } = App.useApp();

  // ===== 订阅 store 状态 =====
  const currentModel = useChatStore((s) => s.currentModel);
  const setCurrentModel = useChatStore((s) => s.setCurrentModel);
  const customConfigs = useChatStore((s) => s.customConfigs);
  const setCustomConfigs = useChatStore((s) => s.setCustomConfigs);
  const selectedConfigId = useChatStore((s) => s.selectedConfigId);
  const setSelectedConfigId = useChatStore((s) => s.setSelectedConfigId);

  /**
   * 初始化自定义模型配置
   *
   * 从 settings 中解析 custom_model_configs + selected_custom_model_config_id，
   * 写入 store。若用户没保存过 selectedConfigId 但已有配置，自动选第一个并持久化。
   */
  const initializeCustomConfigs = useCallback((settingsData: Record<string, unknown>) => {
    const savedConfigs = settingsData?.custom_model_configs;
    let parsedConfigs: CustomModelConfig[] = [];
    if (savedConfigs) {
      try {
        parsedConfigs = JSON.parse(savedConfigs as string);
      } catch (e) {
        console.error('解析自定义模型配置失败:', e);
      }
    }

    const savedConfigId = settingsData?.selected_custom_model_config_id as string | undefined;
    const selectedConfig = savedConfigId
      ? parsedConfigs.find((c: CustomModelConfig) => c.id === savedConfigId)
      : parsedConfigs[0];

    setCustomConfigs(parsedConfigs);

    if (selectedConfig) {
      setCurrentModel({
        key: 'custom',
        label: selectedConfig.modelName || '自定义模型',
        configId: selectedConfig.id,
        config: selectedConfig,
      });
      if (!savedConfigId) {
        // 自动选第一个时，持久化到后端
        setSelectedConfigId(selectedConfig.id);
        updateSettings({ selected_custom_model_config_id: selectedConfig.id } as any);
      } else {
        setSelectedConfigId(savedConfigId);
      }
    } else if (savedConfigId) {
      setSelectedConfigId(savedConfigId);
    }
  }, [setCustomConfigs, setCurrentModel, setSelectedConfigId]);

  /**
   * 处理模型切换
   *
   * 自定义模型：同步 selectedConfigId 并持久化；预定义模型：清空 selectedConfigId。
   */
  const handleModelChange = useCallback((newModel: ModelSelection) => {
    setCurrentModel(newModel);
    if (newModel.key === 'custom' && newModel.configId) {
      setSelectedConfigId(newModel.configId);
      updateSettings({ selected_custom_model_config_id: newModel.configId } as any);
    } else {
      setSelectedConfigId(null);
    }
  }, [setCurrentModel, setSelectedConfigId]);

  /**
   * 保存自定义模型配置列表
   * 序列化为 JSON 后调 updateSettings，失败时弹 antd message。
   */
  const handleSaveConfigs = useCallback(async (configs: CustomModelConfig[]): Promise<boolean> => {
    try {
      await updateSettings({
        custom_model_configs: JSON.stringify(configs),
      } as any);
      setCustomConfigs(configs);
      return true;
    } catch (err: any) {
      console.error('保存自定义模型配置失败:', err);
      message.error('保存配置失败: ' + err.message);
      return false;
    }
  }, [setCustomConfigs, message]);

  /**
   * 切换选中的自定义配置 ID 并持久化
   */
  const handleConfigIdChange = useCallback(async (configId: string) => {
    setSelectedConfigId(configId);
    try {
      await updateSettings({ selected_custom_model_config_id: configId || '' } as any);
    } catch (err) {
      console.error('保存选中的配置 ID 失败:', err);
    }
  }, [setSelectedConfigId]);

  // ===== 计算属性 =====

  /** 当前模型的显示名称（自定义模型用 formatCustomModelLabel 格式化） */
  const displayModel = useMemo(() => {
    if (!currentModel) return null;
    if (currentModel?.key === 'custom') {
      const cfg = customConfigs.find(c => c.id === currentModel?.configId);
      return cfg
        ? formatCustomModelLabel(cfg.modelName || cfg.name)
        : (currentModel?.label || null);
    }
    return currentModel?.label || null;
  }, [currentModel, customConfigs]);

  /** 模型下拉菜单项 */
  const dropdownModelItems = useMemo(() => {
    return customConfigs.map((c) => {
      const customLabel = formatCustomModelLabel(c.modelName || c.name);
      return {
        key: c.id,
        label: (
          <div style={{
            display: 'flex',
            alignItems: 'center',
            gap: 6,
            minWidth: 0,
            maxWidth: 280,
          }} title={customLabel}>
            <span style={{
              flex: 1,
              minWidth: 0,
              fontSize: 12,
              lineHeight: 1.25,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}>
              {customLabel}
            </span>
            <span style={{
              width: 18,
              flexShrink: 0,
              textAlign: 'right',
              fontSize: 12,
              color: 'var(--color-primary)',
            }}>
              {currentModel?.configId === c.id ? '✓' : ''}
            </span>
          </div>
        ),
      };
    });
  }, [customConfigs, currentModel]);

  /** 当前选中的菜单项 key（用于 Dropdown 高亮） */
  const selectedMenuKey = useMemo(() => {
    return currentModel?.configId || selectedConfigId || customConfigs[0]?.id || '__custom_manage__';
  }, [currentModel, selectedConfigId, customConfigs]);

  /** 菜单选中的 keys 集合（仅当 key 合法时返回） */
  const menuSelectedKeys = useMemo(() => {
    const validKeys = new Set([...customConfigs.map(c => c.id), '__custom_manage__']);
    return selectedMenuKey && validKeys.has(selectedMenuKey) ? [selectedMenuKey] : [];
  }, [selectedMenuKey, customConfigs]);

  return {
    // 状态（来自 store）
    currentModel,
    customConfigs,
    selectedConfigId,

    // store setters（透传，保持调用方签名兼容）
    setCurrentModel,
    setCustomConfigs,

    // 副作用 handler
    initializeCustomConfigs,
    handleModelChange,
    handleSaveConfigs,
    handleConfigIdChange,

    // 计算属性
    displayModel,
    dropdownModelItems,
    selectedMenuKey,
    menuSelectedKeys,
  };
}
