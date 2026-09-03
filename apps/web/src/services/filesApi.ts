/**
 * filesApi.ts
 * ============================================================
 * 文件解析 API 模块
 * ============================================================
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * jayread 用这个端点把用户在聊天里上传的二进制文件（PDF/Word/Excel）转成
 * Markdown 注入对话上下文。autonomics 没有通用文档转换服务，函数桩化。
 *
 * 调用方（聊天附件按钮）会 toast 失败提示；UI 入口的隐藏属 Phase 5 清扫，
 * 本阶段保留导出与签名以维持 43 个消费方文件零改动。
 */

/** 原响应形状（`ParseFileResponse`），保持给未来 Phase 3+ 复用 */
export type ParseFileResponse = import('@/types').ParseFileResponse;

/**
 * 解析上传的文件（将二进制文件转换为 Markdown 文本）
 *
 * ⚠️ 桩：autonomics 无文档转换服务。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {File} file - 用户上传的文件对象
 * @returns {Promise<ParseFileResponse>} 解析结果对象（本实现恒 reject）
 * @throws {Error} 恒抛出，调用方需 catch 并提示用户
 */
export async function parseFile(_file: File): Promise<ParseFileResponse> {
  void _file;
  throw new Error('文件解析暂不可用（autonomics 无文档转换服务）');
}
