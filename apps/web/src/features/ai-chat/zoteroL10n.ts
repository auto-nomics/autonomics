/**
 * zoteroL10n.js
 * ============================================================
 * JayRead AI Chat 面板国际化（Localization）模块
 * 适配自原 Zotero 插件版本，移除 Zotero 依赖，改为独立运行
 * 使用内置翻译表实现中英文切换
 * 根据 navigator.language 在中文/英文之间自动选择
 * ============================================================
 */

/**
 * 获取 Zotero 全局对象
 * JayRead 不依赖 Zotero，始终返回 null
 *
 * @returns {null} 始终返回 null
 */
export function getZotero() {
    // JayRead 是独立应用，不依赖 Zotero
    return null;
}

/**
 * 判断当前是否使用中文 UI
 * 直接使用 navigator.language 判断浏览器语言设置
 *
 * @returns {boolean} 是否为中文语言环境
 */
export function isZhLocale() {
    // 检查浏览器语言标识是否以 "zh" 开头
    // 涵盖 zh-CN、zh-TW、zh-HK 等
    const lang = typeof navigator !== 'undefined' ? navigator.language : '';
    return String(lang).toLowerCase().startsWith('zh');
}

/**
 * 模板字符串插值函数
 * 将模板中的占位符 { $variableName } 替换为实际参数值
 * 用于实现类似 Fluent 国际化系统的变量替换功能
 *
 * 模板格式: "你好 { $name }，欢迎使用 { $product }"
 * 参数: { name: "张三", product: "JayRead" }
 * 结果: "你好 张三，欢迎使用 JayRead"
 *
 * @param {string} template - 包含占位符的模板字符串
 * @param {Object|undefined} args - 参数键值对映射
 * @returns {string} 替换后的字符串
 */
function interpolate(template: string, args?: Record<string, string | number>) {
    // 如果没有参数或模板为空，直接返回模板
    if (!args || !template) return template;
    // 正则匹配 { $variableName } 格式的占位符
    // \{\s*\$ - 匹配 "{ " + "$"（允许花括号内有空格）
    // ([a-zA-Z0-9_]+) - 捕获变量名（字母、数字、下划线）
    // \s*\} - 匹配 " }"（允许花括号内有空格）
    // g 标志: 全局匹配，替换所有占位符
    return template.replace(/\{\s*\$([a-zA-Z0-9_]+)\s*\}/g, (_, k) =>
        // 如果参数值存在则转为字符串替换，否则替换为空字符串
        (args[k] != null ? String(args[k]) : ''));
}

/**
 * 内置翻译字符串表
 * 包含所有 AI Chat 面板中需要显示的文本，分为中文（zh）和英文（en）两个版本
 */
const STR = {
    // 中文翻译表
    zh: {
        // 通用按钮文本
        'general-cancel': '取消',
        // 模型分层标签
        'vibe-ai-chat-model-tier-advanced': '高级模型',
        // 高级模型仅限 PRO 及以上用户的提示后缀
        'vibe-ai-chat-model-tier-advanced-pro-only-suffix': '（PRO及以上可使用）',
        // 高级模型需要 PRO 订阅的完整提示
        'vibe-ai-chat-advanced-models-require-pro': '高级模型仅 PRO / Ultimate 活跃订阅可用，请升级套餐或改用标准模型。',
        // 标准模型标签
        'vibe-ai-chat-model-tier-standard': '标准模型',
        // 各 AI 服务商/模型名称
        'vibe-ai-chat-model-chatgpt': 'ChatGPT',
        'vibe-ai-chat-model-grok': 'Grok',
        'vibe-ai-chat-model-gemini': 'Gemini',
        'vibe-ai-chat-model-kimi': 'Kimi',
        'vibe-ai-chat-model-minimax': 'MiniMax',
        'vibe-ai-chat-model-qwen': 'Qwen',
        'vibe-ai-chat-model-doubao': 'Doubao',
        'vibe-ai-chat-model-deepseek': 'DeepSeek',
        'vibe-ai-chat-model-zhipu': '智谱 GLM',
        // 自定义模型相关
        'vibe-ai-chat-custom-model-named': '{ $modelName }',
        'vibe-ai-chat-custom-model-fallback': '自定义模型',
        'vibe-ai-chat-manage-custom-models': '管理自定义模型',
        'vibe-ai-chat-custom-model-settings-title': '自定义模型设置',
        'vibe-ai-chat-saved-configurations': '已保存的配置',
        'vibe-ai-chat-add-configuration': '添加新配置',
        // 自定义模型配置表单
        'vibe-ai-chat-api-base-url': 'API Base URL',
        'vibe-ai-chat-api-base-url-row-tooltip':
            '左侧选择 OpenAI 或 Anthropic 格式。OpenAI 兼容使用 Bearer；Anthropic 使用 x-api-key。OpenAI 可填根地址或 /v1；Anthropic 可填 https://api.anthropic.com 或完整 …/v1/messages。',
        'vibe-ai-chat-api-base-url-placeholder-openai': 'https://api.openai.com/v1',
        'vibe-ai-chat-api-base-url-placeholder-anthropic': 'https://api.anthropic.com',
        'vibe-ai-chat-api-format-label-openai': 'OpenAI',
        'vibe-ai-chat-api-format-label-anthropic': 'Anthropic',
        'vibe-ai-chat-api-key-shared': 'API Key',
        'vibe-ai-chat-api-key-tooltip': '两种格式共用此输入框：按所选格式填写对应服务商的密钥。',
        'vibe-ai-chat-api-key-placeholder': '粘贴服务商提供的 API Key',
        'vibe-ai-chat-model-name': '模型名称',
        'vibe-ai-chat-model-name-tooltip': '例如 gpt-4o、deepseek-chat 等，以服务商文档为准。',
        'vibe-ai-chat-model-name-placeholder': 'model name:如 gpt-4o',
        // 配置操作反馈
        'vibe-ai-chat-config-updated': '配置已更新',
        'vibe-ai-chat-config-added': '配置已添加',
        'vibe-ai-chat-config-save-failed': '保存失败',
        // 删除确认对话框
        'vibe-ai-chat-confirm-delete-title': '确认删除',
        'vibe-ai-chat-confirm-delete-body': '确定要删除配置「{ $name }」吗？',
        'vibe-ai-chat-config-deleted': '配置已删除',
        // 按钮文本
        'vibe-ai-chat-button-update': '更新',
        'vibe-ai-chat-button-add': '添加',
        'vibe-ai-chat-button-close': '关闭',
        'vibe-ai-chat-button-delete': '删除',
        // 其他 UI 文案
        'vibe-ai-chat-unnamed': '未命名',
        'vibe-ai-chat-current-model-title': '当前模型：{ $model }',
        'vibe-ai-chat-prompt-configure-custom-first': '请先在模型菜单中打开「管理自定义模型」并添加、选择配置。',
        'vibe-ai-chat-text-only-model-badge': '纯文本模型（不支持附图）',
        // 多模态不支持错误提示
        'vibe-ai-chat-multimodal-not-supported':
            '「{ $model }」不支持带图片或多模态输入，请切换其他模型。'
    },
    // 英文翻译表（结构与中文表完全对应）
    en: {
        'general-cancel': 'Cancel',
        'vibe-ai-chat-model-tier-advanced': 'Advanced',
        'vibe-ai-chat-model-tier-advanced-pro-only-suffix': ' (PRO & Ultimate only)',
        'vibe-ai-chat-advanced-models-require-pro':
            'Advanced models require an active PRO or Ultimate plan. Upgrade or pick a Standard model.',
        'vibe-ai-chat-model-tier-standard': 'Standard',
        'vibe-ai-chat-model-chatgpt': 'ChatGPT',
        'vibe-ai-chat-model-grok': 'Grok',
        'vibe-ai-chat-model-gemini': 'Gemini ',
        'vibe-ai-chat-model-kimi': 'Kimi',
        'vibe-ai-chat-model-minimax': 'MiniMax',
        'vibe-ai-chat-model-qwen': 'Qwen',
        'vibe-ai-chat-model-doubao': 'Doubao ',
        'vibe-ai-chat-model-deepseek': 'DeepSeek',
        'vibe-ai-chat-model-zhipu': 'Zhipu GLM',
        'vibe-ai-chat-custom-model-named': '{ $modelName }',
        'vibe-ai-chat-custom-model-fallback': 'Custom model',
        'vibe-ai-chat-manage-custom-models': 'Manage Custom Models',
        'vibe-ai-chat-custom-model-settings-title': 'Custom Model Settings',
        'vibe-ai-chat-saved-configurations': 'Saved configurations',
        'vibe-ai-chat-add-configuration': 'Add configuration',
        'vibe-ai-chat-api-base-url': 'API Base URL',
        'vibe-ai-chat-api-base-url-row-tooltip':
            'Choose OpenAI-compatible or Anthropic on the left. OpenAI uses Bearer; Anthropic uses x-api-key. URL: OpenAI root or /v1; Anthropic e.g. https://api.anthropic.com or full …/v1/messages.',
        'vibe-ai-chat-api-base-url-placeholder-openai': 'https://api.openai.com/v1',
        'vibe-ai-chat-api-base-url-placeholder-anthropic': 'https://api.anthropic.com',
        'vibe-ai-chat-api-format-label-openai': 'OpenAI',
        'vibe-ai-chat-api-format-label-anthropic': 'Anthropic',
        'vibe-ai-chat-api-key-shared': 'API Key',
        'vibe-ai-chat-api-key-tooltip': 'One field for both formats: use the key for the provider you selected.',
        'vibe-ai-chat-api-key-placeholder': "Paste your provider's API key",
        'vibe-ai-chat-model-name': 'Model name',
        'vibe-ai-chat-model-name-tooltip': 'Examples: gpt-4o, deepseek-chat, etc. (see your provider docs).',
        'vibe-ai-chat-model-name-placeholder': 'model name:e.g. gpt-4o',
        'vibe-ai-chat-config-updated': 'Configuration updated',
        'vibe-ai-chat-config-added': 'Configuration added',
        'vibe-ai-chat-config-save-failed': 'Failed to save',
        'vibe-ai-chat-confirm-delete-title': 'Remove configuration',
        'vibe-ai-chat-confirm-delete-body': 'Remove "{ $name }"? This cannot be undone.',
        'vibe-ai-chat-config-deleted': 'Configuration removed',
        'vibe-ai-chat-button-update': 'Update',
        'vibe-ai-chat-button-add': 'Add',
        'vibe-ai-chat-button-close': 'Close',
        'vibe-ai-chat-button-delete': 'Delete',
        'vibe-ai-chat-unnamed': 'Unnamed',
        'vibe-ai-chat-current-model-title': 'Current model: { $model }',
        'vibe-ai-chat-prompt-configure-custom-first':
            'Add and select a custom model under Manage Custom Models in the model menu.',
        'vibe-ai-chat-text-only-model-badge': 'Text-only model (no images)',
        'vibe-ai-chat-multimodal-not-supported':
            '{ $model } does not support images or multimodal input. Switch to other model.'
    }
};

/**
 * 国际化文本获取函数（核心函数）
 * 根据当前语言环境从内置翻译表中获取对应的翻译文本
 * 查找优先级：当前语言表 → 英文表（兜底） → 自定义回退值 → 键名本身
 *
 * @param {string} id - 翻译键名（如 'vibe-ai-chat-model-chatgpt'）
 * @param {Record<string, string|number>|undefined} args - 模板参数（用于变量替换）
 * @param {string} [fallback] - 当翻译键不存在时的回退文本
 * @returns {string} 翻译后的文本（已替换模板变量）
 */
export function zoteroL10n(id: string, args?: Record<string, string | number>, fallback?: string) {
    // 根据语言环境选择对应的翻译表
    const lang = isZhLocale() ? 'zh' : 'en';
    const table = STR[lang];
    // 按优先级查找翻译文本：
    // 1. 当前语言表中是否存在该键
    // 2. 英文表中是否存在该键（兜底，确保至少有英文）
    // 3. 使用传入的回退值
    // 4. 使用键名本身作为最后兜底
    const raw = (table as any)[id] ?? (STR.en as any)[id] ?? fallback ?? id;
    // 使用 interpolate 函数替换模板变量
    return interpolate(raw, args);
}

/**
 * 格式化自定义模型的显示标签
 * 根据模型名称是否为空，返回不同的显示文本
 *
 * @param {string|undefined} modelName - 自定义模型名称
 * @returns {string} 格式化后的模型显示标签
 */
export function formatCustomModelLabel(modelName?: string) {
    // 去除模型名称首尾空格
    const name = (modelName || '').trim();
    if (name) {
        // 如果模型名称非空，使用包含名称的模板
        // 例如 "自定义模型（gpt-4o）"
        return zoteroL10n('vibe-ai-chat-custom-model-named', { modelName: name });
    }
    // 如果模型名称为空，使用通用回退文本 "自定义模型"
    return zoteroL10n('vibe-ai-chat-custom-model-fallback');
}
