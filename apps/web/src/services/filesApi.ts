/**
 * filesApi.js
 * ============================================================
 * JayRead 文件解析 API 模块
 * 提供将上传文件发送到后端解析的接口函数
 *
 * 功能说明：
 * - 将用户上传的二进制文件（PDF、Word、Excel、CSV 等）发送到后端
 * - 后端将文件内容转换为 Markdown 文本后返回
 * - 前端将转换后的 Markdown 文本注入到 AI 聊天消息中
 *
 * 注意：
 * - 文本文件和图片文件不需要调用此接口（前端直接处理）
 * - 此接口仅用于需要后端转换的二进制文件
 * ============================================================
 */

// 导入通用 API 客户端封装函数
// request 函数自动处理 URL 拼接、Content-Type 设置和错误处理
// 当 body 是 FormData 时，request 会跳过 Content-Type 设置（浏览器自动处理 boundary）
import request from './client';

/**
 * 解析上传的文件（将二进制文件转换为 Markdown 文本）
 *
 * 处理流程：
 * 1. 将 File 对象包装为 FormData
 * 2. POST 到后端 /files/parse 接口
 * 3. 后端根据文件类型选择合适的解析器（PyMuPDF/python-docx/openpyxl 等）
 * 4. 返回包含 Markdown 文本的结果对象
 *
 * 为什么使用 FormData 而非 JSON？
 * - 文件上传必须使用 multipart/form-data 编码
 * - FormData 会自动设置正确的 Content-Type（含 boundary 分隔符）
 * - 浏览器原生支持 FormData，可以高效传输二进制数据
 *
 * @param {File} file - 用户上传的文件对象（浏览器 File API）
 * @returns {Promise<import('../types/api').ParseFileResponse>} 解析结果对象
 * @throws {Error} 当后端解析失败时抛出包含错误详情的 Error
 */
export async function parseFile(file: File): Promise<import('@/types').ParseFileResponse> {
    // 创建 FormData 对象，用于构建 multipart/form-data 请求体
    // FormData 是浏览器原生 API，用于发送键值对数据（支持文件）
    const formData = new FormData();

    // 将文件添加到 FormData 中
    // key 为 'file'，与后端 FastAPI 的参数名 file 对应
    // FastAPI 的 UploadFile = File(...) 会自动从名为 'file' 的字段读取上传文件
    formData.append('file', file);

    // 发送 POST 请求到后端 /files/parse 接口
    // request 函数检测到 body 是 FormData，会跳过 Content-Type 设置
    // 让浏览器自动生成正确的 multipart/form-data Content-Type（含 boundary）
    return request('/files/parse', {
        method: 'POST',        // HTTP 方法：POST（因为要上传数据）
        body: formData,        // 请求体：包含文件的 FormData 对象
    });
}
