/**
 * FileUploader.jsx
 * ============================================================
 * Autonomics AI Chat 通用文件上传组件
 * 提供上传各类文件的按钮（回形针图标），支持图片、文本、文档等格式
 *
 * 功能说明：
 * - 使用隐藏的 <input type="file"> 实现文件选择
 * - 支持图片、文本、PDF、Word、Excel、CSV 等多种文件格式
 * - 根据文件扩展名自动分类为三种类型：
 *   1. image（图片）：读取为 base64 Data URL
 *   2. text（文本文件）：读取为 UTF-8 字符串
 *   3. binary（二进制文件）：传递原始 File 对象，由父组件调用后端 API 解析
 * - 分类结果通过 onFileSelect 回调传递给父组件
 *
 * 设计原则：
 * - 组件本身是无状态的（stateless），只负责文件选择和分类
 * - 父组件负责后续处理（如调用后端 API、更新附件列表等）
 * - 这样设计使得组件可复用于不同的上传场景
 * ============================================================
 */

// 导入 React 核心钩子：
// - useRef：引用 DOM 元素（如隐藏的 file input）
// - useCallback：缓存回调函数，避免不必要的子组件重渲染
// - forwardRef：允许父组件通过 ref 访问子组件内部的 DOM 节点或命令式方法
// - useImperativeHandle：自定义通过 ref 暴露给父组件的命令式方法（如 triggerUpload）
import React, { useRef, useCallback, forwardRef, useImperativeHandle } from 'react';
// 导入 Ant Design 图标组件：链接图标（LinkOutlined，作为附件上传按钮图标）
import { LinkOutlined } from '@ant-design/icons';
// 导入 Ant Design UI 组件：Button（按钮）、message（全局消息提示）
import { Button, App } from 'antd';

// ========== 文件类型分类工具 ==========

/**
 * 图片文件扩展名集合
 * 包含所有常见的图片格式扩展名（小写）
 * 用于判断上传的文件是否为图片类型
 */
const IMAGE_EXTENSIONS = new Set([
    'png',                  // PNG 图片格式（无损压缩，支持透明度）
    'jpg', 'jpeg',          // JPEG 图片格式（有损压缩，照片常用）
    'gif',                  // GIF 图片格式（支持动画）
    'webp',                 // WebP 图片格式（Google 开发，高压缩率）
    'bmp',                  // BMP 位图格式（无压缩，文件较大）
    'svg',                  // SVG 矢量图格式（可缩放，适合图标和插图）
]);

/**
 * 文本文件扩展名集合
 * 包含所有支持直接在前端读取的文本格式扩展名
 * 文本文件通过 FileReader.readAsText() 读取，无需后端处理
 */
const TEXT_EXTENSIONS = new Set([
    // ===== Markdown 和纯文本 =====
    'md', 'markdown',       // Markdown 标记语言
    'txt',                  // 纯文本文件
    'text',                 // 纯文本（另一种扩展名）
    'log',                  // 日志文件
    // ===== 编程语言源码 =====
    'py',                   // Python
    'js',                   // JavaScript
    'ts',                   // TypeScript
    'jsx', 'tsx',           // React JSX/TSX
    'c', 'cpp', 'cc', 'cxx', // C/C++
    'h', 'hpp',             // C/C++ 头文件
    'java',                 // Java
    'go',                   // Go
    'rs',                   // Rust
    'rb',                   // Ruby
    'php',                  // PHP
    'swift',                // Swift
    'kt',                   // Kotlin
    'r',                    // R
    // ===== Web 相关 =====
    'css', 'scss', 'less',  // 样式表
    'html', 'htm',          // HTML
    'vue', 'svelte',        // 组件框架
    // ===== 配置和数据 =====
    'json',                 // JSON
    'xml',                  // XML
    'yaml', 'yml',          // YAML
    'toml',                 // TOML
    'ini', 'cfg', 'conf',   // 配置文件
    'env',                  // 环境变量
    // ===== 脚本和查询 =====
    'sh', 'bash', 'zsh',    // Shell 脚本
    'sql',                  // SQL
    // ===== 其他 =====
    'tex',                  // LaTeX
    'diff', 'patch',        // 差异/补丁文件
    'dockerfile',           // Dockerfile
    'makefile',             // Makefile
    'graphql', 'gql',       // GraphQL
]);

/**
 * 扩展名到编程语言标识的映射表
 * 用于在消息中标注代码的语言类型（如 ```python）
 * AI 模型能根据语言标识提供更准确的代码分析
 */
const EXT_TO_LANG = {
    'py': 'python',         // Python 语言标识
    'js': 'javascript',     // JavaScript 语言标识
    'ts': 'typescript',     // TypeScript 语言标识
    'jsx': 'jsx',           // React JSX 标识
    'tsx': 'tsx',           // React TSX 标识
    'c': 'c',               // C 语言标识
    'cpp': 'cpp',           // C++ 语言标识
    'java': 'java',         // Java 语言标识
    'go': 'go',             // Go 语言标识
    'rs': 'rust',           // Rust 语言标识
    'rb': 'ruby',           // Ruby 语言标识
    'css': 'css',           // CSS 标识
    'html': 'html',         // HTML 标识
    'json': 'json',         // JSON 标识
    'xml': 'xml',           // XML 标识
    'yaml': 'yaml',         // YAML 标识
    'sql': 'sql',           // SQL 标识
    'sh': 'shell',          // Shell 标识
    'bash': 'bash',         // Bash 标识
    'tex': 'latex',         // LaTeX 标识
    'md': 'markdown',       // Markdown 标识
};

/**
 * 文件类型分类函数
 * 根据文件扩展名判断文件属于哪种类型类别
 *
 * 分类规则：
 * - image: 图片文件，前端直接读取为 base64
 * - text: 文本文件，前端直接读取为字符串
 * - binary: 二进制文件（PDF/Word/Excel 等），需要后端解析
 *
 * @param {string} filename - 文件名（含扩展名），如 "report.pdf"
 * @returns {{ category: 'image'|'text'|'binary', ext: string, lang?: string }}
 *   分类结果对象：
 *   - category: 文件类型类别
 *   - ext: 小写的文件扩展名
 *   - lang: 编程语言标识（仅文本文件）
 */
function classifyFile(filename: string) {
    // 提取文件扩展名（小写），如 "report.PDF" -> "pdf"
    const ext = (filename.split('.').pop() || '').toLowerCase();

    // 判断是否为图片文件
    if (IMAGE_EXTENSIONS.has(ext)) {
        return { category: 'image', ext };
    }

    // 判断是否为文本文件
    if (TEXT_EXTENSIONS.has(ext)) {
        // 从映射表获取编程语言标识，用于代码围栏标注
        const lang = (EXT_TO_LANG as Record<string, string>)[ext] || '';
        return { category: 'text', ext, lang };
    }

    // 其他文件类型视为二进制文件，需要后端解析
    // 包括 PDF、Word (.docx)、Excel (.xlsx)、CSV 等
    return { category: 'binary', ext };
}

/**
 * FileUploader 通用文件上传组件
 * 提供回形针图标的文件选择按钮，支持多种文件格式
 *
 * @param {Object} props - 组件属性
 * @param {Function} props.onFileSelect - 文件选择完成后的回调函数
 *   根据文件类型，参数格式不同：
 *   - 图片: { category: 'image', base64: string, name: string, sourceType: 'upload' }
 *   - 文本: { category: 'text', name: string, textContent: string, language: string }
 *   - 二进制: { category: 'binary', name: string, originalType: string, file: File }
 * @param {Object} [props.iconStyle={}] - 按钮图标的自定义样式
 * @param {boolean} [props.disabled=false] - 是否禁用按钮
 * @param {string} [props.disabledTitle=''] - 禁用时的提示信息
 */
// 使用 forwardRef 包裹组件，允许父组件通过 ref 访问子组件暴露的命令式方法
// 这是 React 中"子组件向父组件暴露方法"的标准模式
// ref 参数由 forwardRef 自动注入，与 props 一起传递给组件
const FileUploader = forwardRef(({ onFileSelect, iconStyle = {}, disabled = false, disabledTitle = '', size }: { onFileSelect?: (file: any) => void; iconStyle?: Record<string, any>; disabled?: boolean; disabledTitle?: string; size?: 'small' | 'middle' | 'large' }, ref) => {
    const { message } = App.useApp();
    // 创建对隐藏 file input 元素的引用
    // 用于在用户点击按钮或调用 triggerUpload() 时，通过 JS 触发文件选择对话框
    const fileInputRef = useRef<HTMLInputElement>(null);

    // ========== 命令式 API：向父组件暴露 triggerUpload 方法 ==========
    // 使用 useImperativeHandle 自定义通过 ref 暴露给父组件的方法
    // 这样父组件可以通过 fileUploaderRef.current.triggerUpload() 来触发文件选择
    // 适用场景：
    // 1. 斜杠命令 /attachment 触发文件上传（无需点击按钮）
    // 2. 其他组件需要通过编程方式打开文件选择对话框
    useImperativeHandle(ref, () => ({
        /**
         * 触发文件选择对话框
         * 与点击回形针按钮效果完全一致
         * 通过编程方式调用隐藏 <input type="file"> 的 click 事件
         */
        triggerUpload: () => {
            // 如果按钮处于禁用状态，显示提示并返回
            if (disabled) {
                if (disabledTitle) message.warning(disabledTitle);
                return;
            }
            // 通过引用触发隐藏的 file input 的点击事件
            // 这会打开浏览器的文件选择对话框，与用户手动点击按钮效果相同
            fileInputRef.current?.click();
        },
    }), [disabled, disabledTitle]); // 依赖：禁用状态和禁用提示文本（变化时重新创建暴露的方法）

    /**
     * 处理文件选择事件
     * 用户选择文件后，根据文件类型执行不同的处理逻辑：
     * - 图片：读取为 base64 Data URL
     * - 文本：读取为 UTF-8 字符串
     * - 二进制：直接传递 File 对象给父组件
     *
     * @param {Event} event - 文件选择事件对象
     */
    const handleFileChange = useCallback((event: React.ChangeEvent<HTMLInputElement>) => {
        // 如果按钮被禁用，忽略文件选择事件
        if (disabled) return;

        // 获取用户选择的第一个文件
        // event.target.files 是 FileList 对象，可能为空（用户取消选择）
        const file = event.target.files?.[0];
        if (!file) return;  // 没有选择文件，直接返回

        // 根据文件扩展名对文件进行分类
        const classification = classifyFile(file.name);

        // 根据不同的分类执行不同的处理逻辑
        if (classification.category === 'image') {
            // ===== 图片文件：读取为 base64 Data URL =====
            // 图片以 base64 编码存储，随对话消息发送给 AI 模型

            // 文件大小校验：单张图片最大 10MB
            if (file.size > 10 * 1024 * 1024) {
                message.error('图片大小不能超过 10MB');
                return;
            }

            // 创建 FileReader 用于读取文件内容
            const reader = new FileReader();
            // 读取成功回调：将 base64 数据传递给父组件
            reader.onload = (e) => {
                // 获取读取结果（Data URL 格式：data:image/png;base64,...）
                const base64 = e.target?.result;
                if (base64 && onFileSelect) {
                    // 调用父组件的回调函数，传递图片数据
                    onFileSelect({
                        category: 'image',      // 文件类别：图片
                        base64,                 // base64 编码的图片数据
                        name: file.name,        // 原始文件名
                        sourceType: 'upload',   // 来源类型：上传
                    });
                }
            };
            // 读取失败回调：提示用户
            reader.onerror = () => {
                message.error('读取图片失败');
            };
            // 以 Data URL 格式读取文件（结果形如 data:image/png;base64,...）
            reader.readAsDataURL(file);

        } else if (classification.category === 'text') {
            // ===== 文本文件：读取为 UTF-8 字符串 =====
            // 文本文件直接在前端读取内容，无需调用后端 API

            // 文件大小校验：文本文件最大 1MB（超过此大小的文本文件通常不适合直接注入消息）
            if (file.size > 1 * 1024 * 1024) {
                message.error('文本文件大小不能超过 1MB');
                return;
            }

            // 创建 FileReader 用于读取文件内容
            const reader = new FileReader();
            // 读取成功回调：将文本内容传递给父组件
            reader.onload = (e) => {
                // 获取读取结果（UTF-8 解码后的字符串）
                const textContent = e.target?.result;
                if (textContent !== undefined && onFileSelect) {
                    // 调用父组件的回调函数，传递文本数据
                    onFileSelect({
                        category: 'text',           // 文件类别：文本
                        name: file.name,            // 原始文件名
                        textContent: String(textContent),  // 文件文本内容
                        language: classification.lang || '',  // 编程语言标识
                    });
                }
            };
            // 读取失败回调：提示用户
            reader.onerror = () => {
                message.error('读取文件失败');
            };
            // 以文本格式读取文件（默认使用 UTF-8 编码解码）
            reader.readAsText(file);

        } else {
            // ===== 二进制文件：传递原始 File 对象给父组件 =====
            // 二进制文件（PDF/Word/Excel 等）需要后端解析
            // 父组件负责调用后端 API 进行文件转换

            // 文件大小校验：二进制文件最大 20MB
            if (file.size > 20 * 1024 * 1024) {
                message.error('文件大小不能超过 20MB');
                return;
            }

            // 直接调用父组件的回调函数，传递 File 对象
            // 父组件将负责：
            // 1. 显示"转换中"的加载状态
            // 2. 调用后端 /api/files/parse 接口
            // 3. 更新附件的 Markdown 内容
            if (onFileSelect) {
                onFileSelect({
                    category: 'binary',          // 文件类别：二进制
                    name: file.name,             // 原始文件名
                    originalType: classification.ext,  // 文件类型（扩展名，如 'pdf'、'docx'）
                    file,                        // 原始 File 对象（供后端 API 使用）
                });
            }
        }

        // 重置 file input 的值，允许重复选择同一个文件
        // 如果不重置，用户选择同一个文件后不会触发 change 事件
        event.target.value = '';
    }, [onFileSelect, disabled]);  // 依赖：回调函数和禁用状态

    /**
     * 触发文件选择对话框
     * 用户点击回形针按钮时调用
     */
    const handleUploadClick = useCallback(() => {
        // 如果按钮被禁用，显示提示信息
        if (disabled) {
            if (disabledTitle) message.warning(disabledTitle);
            return;
        }
        // 通过引用触发隐藏的 file input 的点击事件
        // 这会打开浏览器的文件选择对话框
        fileInputRef.current?.click();
    }, [disabled, disabledTitle]);

    // ========== 渲染组件 ==========
    return (
        <>
            {/* 隐藏的文件选择输入框 */}
            {/* 通过 display:none 隐藏，由按钮的点击事件间接触发 */}
            <input
                ref={fileInputRef}                  // 引用，用于 JS 触发点击
                type="file"                         // 文件选择类型
                accept=".png,.jpg,.jpeg,.gif,.webp,.bmp,.svg,.pdf,.docx,.doc,.xlsx,.xls,.csv,.md,.txt,.py,.json,.js,.ts,.jsx,.tsx,.css,.scss,.html,.xml,.yaml,.yml,.toml,.sh,.bash,.zsh,.sql,.c,.cpp,.h,.java,.go,.rs,.rb,.r,.tex,.log,.env,.ini,.cfg,.conf,.vue,.svelte,.php,.swift,.kt,.graphql,.gql,.diff,.patch"  // 允许的文件类型列表
                onChange={handleFileChange}         // 文件选择事件处理
                style={{ display: 'none' }}         // 隐藏输入框
            />

            {/* 回形针图标按钮 */}
            {/* 点击后打开文件选择对话框 */}
            <Button
                type="text"                         // 文本按钮样式（无边框、无背景）
                size={size}                         // 按钮尺寸（可选 'small' | 'middle' | 'large'）
                icon={<LinkOutlined />}               // 链接图标（🔗，代表附件上传）
                onClick={handleUploadClick}          // 点击打开文件选择
                title={disabled ? (disabledTitle || '当前不可上传') : '上传附件'}  // 悬停提示文字
                disabled={disabled}                  // 禁用状态
                style={{ ...iconStyle, opacity: disabled ? 0.45 : 1 }}  // 禁用时降低透明度
            />
        </>
    );
}); // forwardRef 包裹结束：FileUploader 现在支持 ref 转发，父组件可通过 ref 调用 triggerUpload()

// 导出 FileUploader 组件，供 ChatPanel 等父组件使用
// forwardRef 包裹后的组件用法：<FileUploader ref={myRef} onFileSelect={...} />
export default FileUploader;

// 导出分类函数和常量，供其他组件复用
export { classifyFile, IMAGE_EXTENSIONS, TEXT_EXTENSIONS, EXT_TO_LANG };
