/**
 * usePaperUpload - PDF 文件上传钩子
 *
 * 本文件封装了 PDF 文件上传功能，包括：
 * - 通用上传（不指定分类，上传后手动分配）
 * - 上传到指定分类（右键菜单触发，上传后自动归入该分类）
 * - 文件类型校验（仅支持 PDF 格式）
 * - 上传状态管理（防止重复上传）
 * - 上传返回即终态（autonomics 服务端同步完成抽取+建档，无后续解析流水线）
 *
 * 上传流程：
 * 1. 用户选择 PDF 文件（拖拽或点击）
 * 2. 前端校验文件类型（.pdf 后缀）
 * 3. 调用后端 API 上传文件
 * 4. 后端返回新论文对象（包含论文 ID 和初始状态）
 * 5. 前端将新论文按 id 去重插入列表（命中已有文献则更新原行）
 *
 * 为什么需要两个上传函数？
 * - handleUpload: 由 Ant Design Dragger 的 beforeUpload 触发，返回值必须是 false
 * - handleUploadToCategory: 右键菜单触发，需要携带目标分类 ID
 */

import { useState, useCallback } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // 导入 Ant Design App 组件
import { uploadPaper } from '../../../services/papersApi'; // 导入上传论文 API 函数
import { FILE_UPLOAD } from '../../../config/constants'; // 导入文件上传常量

/**
 * PDF 文件上传钩子
 *
 * @param {Function} setPapers - 更新论文列表的 setState 函数
 * @param {Function} listenToParseStatus - （保留参数兼容调用方）autonomics 无解析流水线，已桩化不使用
 * @returns {Object} 返回状态和上传处理函数的对象
 */
export function usePaperUpload(setPapers: any, _listenToParseStatus: any) {
  const { message } = App.useApp();
  // ========== 状态定义 ==========

  // 上传状态：控制上传按钮的禁用状态，防止重复上传
  const [uploading, setUploading] = useState(false); // 状态：是否正在上传，初始为 false

  // ========== 上传处理函数 ==========

  /**
   * 处理文件上传（通用上传，不指定分类）
   *
   * 此函数目前作为 handleUploadToCategory 的简化版本保留，
   * 后续如需其他入口调用可复用。
   *
   * 工作流程：
   * 1. 校验文件类型，只接受 PDF
   * 2. 设置上传中状态
   * 3. 调用 API 上传文件
   * 4. 将新论文添加到列表顶部
   * 5. 启动 SSE 监听解析进度
   * 6. 显示成功提示
   *
   * @param {File} file - 用户选择的文件对象
   * @returns {boolean} 返回 false 阻止 Ant Design 的默认上传行为
   */
  const handleUpload = useCallback(async (file: any) => {
    // 校验文件类型，只接受 PDF
    if (!FILE_UPLOAD.ALLOWED_EXTENSIONS.some(ext => file.name.toLowerCase().endsWith(ext))) { // 检查文件扩展名
      message.error('仅支持 PDF 文件'); // 提示不支持该文件类型
      return false; // 阻止上传
    }

    // 设置上传中状态，禁用上传按钮
    setUploading(true); // 将上传状态设为 true

    try { // 开始 try-catch
      // 调用 API 上传文件（不指定分类，用户后续手动分配）。
      // autonomics 端同步完成抽取+建档，返回即终态（无后续解析流水线）
      const paper = await uploadPaper(file); // 调用上传接口，返回完整 Paper

      // 按 id 去重插入：标识符命中已有文献时（created=false）更新原行，
      // 新文献则插到列表顶部
      setPapers((prev: any) => {
        const idx = prev.findIndex((p: any) => String(p.id) === String(paper.id));
        if (idx >= 0) {
          const next = [...prev];
          next[idx] = { ...next[idx], ...paper };
          return next;
        }
        return [paper, ...prev];
      });

      // 显示成功提示（重复上传时告知已并入原条目）
      if (paper.created === false) {
        message.info('该文献已在库中，文件已挂到原条目');
      } else {
        message.success('上传成功');
      }
    } catch (err: any) { // 捕获上传错误
      // 上传失败，显示错误信息
      message.error('上传失败: ' + err.message); // 弹出错误提示
    } finally { // 无论成功失败都执行
      // 无论成功失败都恢复上传按钮状态
      setUploading(false); // 恢复上传状态为 false
    }

    // 阻止 Ant Design 的默认上传行为（我们已手动处理）
    return false; // 返回 false 阻止 Ant Design Upload 的默认上传流程
  }, [setPapers, message]); // 依赖项：更新论文列表函数与 toast（SSE 监听已桩化）

  /**
   * 上传文件到指定分类的处理函数
   *
   * 此函数从 CategorySidebar 的右键菜单"上传文件"功能触发，
   * 与 handleUpload 类似，但额外携带目标分类 ID，实现上传后自动归入指定分类。
   *
   * 为什么需要一个独立的上传函数而不是复用 handleUpload？
   * - handleUpload 由 Ant Design Dragger 的 beforeUpload 触发，返回值必须是 false
   * - handleUpload 不接受 categoryId 参数
   * - 此函数需要将 categoryId 传递给 uploadPaper API，实现一步到位的上传+分类
   *
   * 处理流程：
   * 1. 校验文件类型（仅支持 PDF）
   * 2. 调用 uploadPaper API 上传文件并指定目标分类
   * 3. 将新论文添加到列表顶部
   * 4. 启动 SSE 监听解析进度
   *
   * @param {File} file - 用户选择的 PDF 文件对象
   * @param {number} categoryId - 目标分类 ID（用户右键点击的分类）
   */
  const handleUploadToCategory = useCallback(async (file: any, categoryId: any) => {
    // 校验文件类型：只接受 PDF 格式（双重校验：input accept 属性 + 此处程序校验）
    if (!file.name.toLowerCase().endsWith('.pdf')) { // 不区分大小写检查 .pdf 后缀
      message.error('仅支持 PDF 文件'); // 显示错误提示
      return; // 中止上传
    }

    // 设置上传中状态，禁用主页面的上传区域防止并发上传
    setUploading(true); // 将上传状态设为 true

    try { // 开始 try-catch 错误处理
      // 调用 uploadPaper API，传入文件和目标分类 ID
      // 后端会同时完成文件保存和分类关联，无需二次调用 assignPapers
      const paper = await uploadPaper(file, categoryId); // 上传文件并指定目标分类

      // 按 id 去重插入（与 handleUpload 同一策略）：命中已有文献时更新原行
      setPapers((prev: any) => {
        const idx = prev.findIndex((p: any) => String(p.id) === String(paper.id));
        if (idx >= 0) {
          const next = [...prev];
          next[idx] = { ...next[idx], ...paper };
          return next;
        }
        return [paper, ...prev];
      });

      // 显示成功提示（重复上传时告知已并入原条目）
      if (paper.created === false) {
        message.info('该文献已在库中，文件已挂到原条目');
      } else {
        message.success('上传成功');
      }
    } catch (err: any) { // 捕获上传或解析过程中的错误
      // 上传失败，显示错误信息
      message.error('上传失败: ' + err.message); // 弹出错误提示
    } finally { // 无论成功失败都执行
      // 恢复上传状态，允许后续上传操作
      setUploading(false); // 恢复上传状态为 false
    }
  }, [setPapers, message]); // 依赖项：更新论文列表函数与 toast（SSE 监听已桩化）

  // 返回公共接口
  return {
    uploading,                 // 上传中状态，用于控制上传按钮的禁用
    handleUpload,              // 通用上传处理函数（不指定分类）
    handleUploadToCategory,    // 上传到指定分类的处理函数
  };
}

/**
 * 导出类型定义（用于 TypeScript 项目）
 * 本项目使用 JavaScript，保留类型注释供参考
 */
/**
 * @typedef {Object} UploadResult
 * @property {number} id - 新创建的论文 ID
 * @property {string} title - 论文标题
 * @property {string} parse_status - 解析状态（pending/parsing/done/failed）
 */
