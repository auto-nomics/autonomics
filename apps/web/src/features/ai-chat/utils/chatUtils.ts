/**
 * AI Chat 请求构建工具
 *
 * 纯函数集合，从 useChatSender 抽离的"如何把模型配置 + 消息数组变成 HTTP 请求"逻辑：
 * - resolveModelConfig：从 currentModel + settings 解析出 provider/apiKey/model/baseUrl/modelConfig
 * - buildTargetUrl：根据 provider 和 baseUrl 拼出 chat/completions 或 anthropic messages 端点
 * - buildRequestBody：根据 provider 序列化出 OpenAI 或 Anthropic 格式的 JSON 请求体
 *
 * 设计要点：
 * - 完全无副作用、无 React 依赖，便于单元测试
 * - 接收 plain params（不依赖 settings 对象的具体形状），调用方负责传入
 *
 * 抽出原因：原 useChatSender 内的三个 useCallback（~70 行）是纯逻辑，
 * 独立成 utils 后 useChatSender 只剩 sendMessages 主体编排。
 *
 * @module ai-chat/utils/chatUtils
 */

import {
  buildAnthropicSystemPrompt,
  convertToAnthropicContent,
} from './chatMessageUtils';

/** Anthropic / OpenAI 请求默认 max_tokens */
export const DEFAULT_MAX_TOKENS = 8192;
/** OpenAI 默认 temperature */
export const DEFAULT_TEMPERATURE = 0.7;

/** useChatSender 内部的模型配置形状（与 useChatSender 同名接口保持兼容） */
export interface ModelConfig {
  key?: string;
  label?: string;
  configId?: string;
  config?: {
    apiFormat?: string;
    apiKey?: string;
    modelName?: string;
    baseUrl?: string;
    supportsVision?: boolean;
  };
}

/** resolveModelConfig 返回值 */
export interface ResolvedModelConfig {
  provider: string;
  apiKey: string;
  model: string;
  baseUrl: string;
  modelConfig: Record<string, unknown>;
  customConfig: ModelConfig['config'] | null;
}

/**
 * 解析模型配置
 * 优先级：自定义配置 (currentModel.key === 'custom') > settings.model_config 解析 > settings 顶层字段 > 默认值
 *
 * @param currentModel 当前模型对象（含可选 custom config）
 * @param settings 后端 settings 对象（运行时为 Record<string, unknown>，含 model_config / api_key 等字段）
 */
export function resolveModelConfig(
  currentModel: ModelConfig | null | undefined,
  settings: Record<string, unknown>,
): ResolvedModelConfig {
  const customConfig = currentModel?.key === 'custom' ? currentModel.config : null;
  const modelConfig = JSON.parse((settings.model_config as string) || '{}');
  return {
    provider: customConfig?.apiFormat || modelConfig.provider || settings.ai_provider || 'openai',
    apiKey: customConfig?.apiKey || modelConfig.apiKey || settings.api_key || '',
    model: customConfig?.modelName || modelConfig.model || settings.model || 'gpt-4o-mini',
    baseUrl: customConfig?.baseUrl || modelConfig.baseUrl || settings.base_url || 'https://api.openai.com/v1',
    modelConfig,
    customConfig,
  };
}

/**
 * 构建目标 URL
 *
 * Anthropic provider：去 /v1 后缀 → 重新拼 /v1/messages
 * OpenAI 兼容：处理已带 /chat/completions、/vN、/vN/ 三种 baseUrl 形态
 */
export function buildTargetUrl(provider: string, baseUrl: string): string {
  if (provider === 'anthropic') {
    const normalizedBaseUrl = baseUrl.replace(/\/v1\/?$/, '').replace(/\/+$/, '');
    return `${normalizedBaseUrl}/v1/messages`;
  }
  let url = baseUrl.trim().replace(/\/+$/, '');
  if (url.endsWith('/chat/completions')) return url;
  if (/\/v\d+$/.test(url)) return url + '/chat/completions';
  if (/\/v\d+\//.test(url)) return url.replace(/(\/v\d+\/).*$/, '$1chat/completions');
  return url + '/v1/chat/completions';
}

/**
 * 构建请求体
 *
 * Anthropic provider：拆出 system 字段、过滤 system role、转换 content 为 Anthropic 多模态格式
 * OpenAI 兼容：原样透传 messages，附带 temperature
 */
export function buildRequestBody(
  provider: string,
  model: string,
  modelConfig: Record<string, unknown>,
  finalApiMessages: Array<{ role: string; content: unknown }>,
  proxyMeta: Record<string, unknown>,
): string {
  if (provider === 'anthropic') {
    return JSON.stringify({
      _proxy_meta: proxyMeta,
      model,
      max_tokens: DEFAULT_MAX_TOKENS,
      messages: finalApiMessages.filter(m => m.role !== 'system').map(m => ({
        role: m.role,
        content: convertToAnthropicContent(m.content as any),
      })),
      system: buildAnthropicSystemPrompt(finalApiMessages.find(m => m.role === 'system')?.content as any),
      stream: true,
    });
  }
  return JSON.stringify({
    _proxy_meta: proxyMeta,
    model,
    messages: finalApiMessages,
    stream: true,
    max_tokens: DEFAULT_MAX_TOKENS,
    temperature: parseFloat((modelConfig as any).temperature || DEFAULT_TEMPERATURE),
  });
}
