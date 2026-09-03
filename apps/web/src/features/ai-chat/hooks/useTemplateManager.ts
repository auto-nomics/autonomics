/**
 * useTemplateManager - 模板管理 Hook
 *
 * 管理聊天模板数据的加载和保存。
 * 从 ChatPanel 中提取，封装模板状态和持久化逻辑。
 */
import { useState, useCallback } from 'react';
import { App } from 'antd';
import { updateSettings } from '../../../services/settingsApi';
import { registerTemplates, getDefaultTemplates } from '../../../components/ContextBridge';

export function useTemplateManager() {
  const { message } = App.useApp();
  const [isTemplateModalOpen, setIsTemplateModalOpen] = useState(false);
  const [templateData, setTemplateData] = useState<{ templates: any[] }>({ templates: [] });

  // 从设置数据中加载模板（由 init effect 调用）
  const loadTemplates = useCallback((settingsData: Record<string, any>) => {
    let resolvedTemplates;
    const savedTemplates = settingsData?.custom_prompt_templates;
    if (savedTemplates) {
      try {
        resolvedTemplates = JSON.parse(savedTemplates);
      } catch (e) {
        console.error('解析模板数据失败:', e);
        resolvedTemplates = { templates: getDefaultTemplates() };
      }
    } else {
      resolvedTemplates = { templates: getDefaultTemplates() };
    }
    const finalTemplates = registerTemplates(resolvedTemplates);
    setTemplateData({ templates: finalTemplates });
  }, []);

  // 保存模板到后端并注册到 ContextBridge
  const handleSaveTemplates = useCallback(async (data: any) => {
    try {
      registerTemplates(data);
      await updateSettings({
        custom_prompt_templates: JSON.stringify(data),
      } as any);
      setTemplateData(data);
    } catch (err: any) {
      console.error('保存模板数据失败:', err);
      message.error('保存模板失败: ' + err.message);
    }
  }, []);

  return {
    isTemplateModalOpen,
    setIsTemplateModalOpen,
    templateData,
    loadTemplates,
    handleSaveTemplates,
  };
}
