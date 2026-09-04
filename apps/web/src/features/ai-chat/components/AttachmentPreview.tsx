/**
 * AttachmentPreview.jsx
 * ============================================================
 * Autonomics AI Chat 附件预览组件 - 显示在聊天输入框上方
 * 用于预览用户已选择/上传的所有附件，在发送前提供可视化管理
 *
 * 功能：
 * - 以缩略图/卡片网格形式展示已选择的附件
 * - 支持三种附件类型的预览：
 *   1. 图片附件：64x64 缩略图，点击查看大图
 *   2. 文本文件：文件卡片，显示文件图标、文件名和语言标签
 *   3. 二进制文件：文件卡片，显示转换状态（转换中/已完成/失败）
 * - 支持删除已选择的附件
 * - 图片附件支持大图预览（Lightbox 弹窗）
 * ============================================================
 */

// 导入 React 核心钩子：useState（状态管理）
import React from 'react';
// 导入 Ant Design 图标组件
import {
    CloseOutlined,          // 关闭/删除图标（×）
    FileTextOutlined,       // 文本文件图标
    FilePdfOutlined,        // PDF 文件图标
    FileWordOutlined,       // Word 文档图标
    FileExcelOutlined,      // Excel 表格图标
    FileOutlined,           // 通用文件图标
    CheckCircleOutlined,    // 成功/完成图标（绿色对勾）
    LoadingOutlined,        // 加载中图标（旋转动画）
    ExclamationCircleOutlined, // 错误/警告图标
} from '@ant-design/icons';
// 导入 Ant Design UI 组件：Spin（加载动画）
import { Spin } from 'antd';
import NiceModal from '@ebay/nice-modal-react';

/**
 * 根据文件扩展名获取对应的图标组件
 * 不同类型的文件使用不同的图标，方便用户快速识别
 *
 * @param {string} ext - 文件扩展名（小写），如 'pdf'、'docx'、'py'
 * @param {object} style - 图标的自定义样式
 * @returns {React.ReactElement} 对应的图标组件
 */
const getFileIcon = (ext: string, style: Record<string, any> = {}) => {
    // 根据扩展名选择对应的图标
    switch (ext) {
        // PDF 文件：使用红色 PDF 图标
        case 'pdf':
            return <FilePdfOutlined style={{ ...style, color: 'var(--file-icon-pdf)' }} />;
        // Word 文档：使用蓝色 Word 图标
        case 'docx': case 'doc':
            return <FileWordOutlined style={{ ...style, color: 'var(--file-icon-word)' }} />;
        // Excel 表格：使用绿色 Excel 图标
        case 'xlsx': case 'xls': case 'csv':
            return <FileExcelOutlined style={{ ...style, color: 'var(--file-icon-excel)' }} />;
        // 文本文件：使用蓝色文本图标
        case 'txt': case 'md': case 'markdown': case 'log':
            return <FileTextOutlined style={{ ...style, color: 'var(--file-icon-text)' }} />;
        // 默认：使用灰色通用文件图标
        default:
            return <FileTextOutlined style={{ ...style, color: 'var(--file-icon-default)' }} />;
    }
};

/**
 * 根据文件扩展名获取语言标签文本
 * 在文件卡片上显示简短的语言/类型标签
 *
 * @param {string} ext - 文件扩展名
 * @param {string} category - 文件类别（image/text/binary）
 * @returns {string} 语言/类型标签文本，如 "Python"、"PDF"、"Word"
 */
const getLanguageLabel = (ext: string, category: string) => {
    // 图片文件：显示 "图片" 标签
    if (category === 'image') return '图片';
    // PDF 文件：显示 "PDF" 标签
    if (ext === 'pdf') return 'PDF';
    // Word 文档：显示 "Word" 标签
    if (ext === 'docx' || ext === 'doc') return 'Word';
    // Excel 表格：显示 "Excel" 标签
    if (ext === 'xlsx' || ext === 'xls') return 'Excel';
    // CSV 文件：显示 "CSV" 标签
    if (ext === 'csv') return 'CSV';
    // Markdown 文件：显示 "Markdown" 标签
    if (ext === 'md' || ext === 'markdown') return 'Markdown';
    // 其他文本文件：将扩展名转为大写作为标签
    // 如 'py' -> 'Python'、'json' -> 'JSON'
    const langMap = {
        'py': 'Python', 'js': 'JavaScript', 'ts': 'TypeScript',
        'jsx': 'JSX', 'tsx': 'TSX', 'c': 'C', 'cpp': 'C++',
        'java': 'Java', 'go': 'Go', 'rs': 'Rust', 'rb': 'Ruby',
        'css': 'CSS', 'html': 'HTML', 'json': 'JSON', 'xml': 'XML',
        'yaml': 'YAML', 'yml': 'YAML', 'toml': 'TOML', 'sql': 'SQL',
        'sh': 'Shell', 'bash': 'Bash', 'tex': 'LaTeX',
    };
    return (langMap as Record<string, string>)[ext] || ext.toUpperCase();
};

/**
 * AttachmentPreview 附件预览组件
 * 显示在输入框上方，以缩略图/卡片网格形式展示待发送的附件
 *
 * @param {Object} props - 组件属性
 * @param {Array} [props.attachments=[]] - 附件数组
 *   每个元素是一个附件对象，包含以下字段：
 *   - 图片: { category: 'image', base64: string, name: string, sourceType: string }
 *   - 文本: { category: 'text', name: string, textContent: string, language: string }
 *   - 二进制: { category: 'binary', name: string, originalType: string, markdown: string, status: string, error?: string }
 * @param {Function} props.onRemove - 移除附件的回调函数，参数为附件索引 index
 */
const AttachmentPreview = ({ attachments = [], onRemove }: { attachments?: any[]; onRemove?: (index: number) => void }) => {
    // 如果没有附件，不渲染任何内容
    if (!attachments || attachments.length === 0) return null;

    /**
     * 点击图片附件时显示大图预览
     */
    const handlePreview = (attachment: any) => {
        void NiceModal.show('image-lightbox', {
            src: attachment.base64 || '',
            alt: attachment.name || '图片',
        });
    };

    /**
     * 从文件名提取扩展名
     *
     * @param {string} filename - 文件名
     * @returns {string} 小写的扩展名
     */
    const getExt = (filename: string) => {
        return (filename.split('.').pop() || '').toLowerCase();
    };

    /**
     * 渲染单个图片附件的缩略图
     *
     * @param {Object} attachment - 图片附件对象
     * @param {number} index - 附件在数组中的索引
     * @returns {React.ReactElement} 缩略图元素
     */
    const renderImagePreview = (attachment: any, index: number) => (
        // 外层容器：包含缩略图和删除按钮
        <div
            key={index}  // 使用索引作为 React key
            style={{
                position: 'relative',   // 相对定位（删除按钮使用绝对定位）
                padding: '6px 6px 0 0', // 为右上角的删除按钮预留空间
            }}
        >
            {/* 图片缩略图容器 */}
            <div
                style={{
                    position: 'relative',
                    width: 64,                   // 缩略图宽度 64px
                    height: 64,                  // 缩略图高度 64px
                    borderRadius: 8,             // 圆角
                    overflow: 'hidden',          // 超出部分隐藏（配合圆角裁剪）
                    border: '1px solid var(--border-color-light, #e0e0e0)',  // 边框：使用全局浅色边框变量
                    backgroundColor: 'var(--bg-secondary, #f5f5f5)',  // 背景色：使用全局次级背景色变量
                    cursor: 'pointer',           // 鼠标悬停显示手型光标
                    transition: 'transform 0.2s, box-shadow 0.2s',  // 悬停动画过渡
                }}
                onClick={() => handlePreview(attachment)}  // 点击查看大图
                onMouseEnter={(e) => {
                    // 鼠标进入：略微放大并添加阴影
                    e.currentTarget.style.transform = 'scale(1.02)';
                    e.currentTarget.style.boxShadow = '0 2px 8px rgba(0,0,0,0.15)';
                }}
                onMouseLeave={(e) => {
                    // 鼠标离开：恢复原始状态
                    e.currentTarget.style.transform = 'scale(1)';
                    e.currentTarget.style.boxShadow = 'none';
                }}
                title="点击查看大图"  // 悬停提示
            >
                {/* 缩略图图片 */}
                <img
                    src={attachment.base64 || ''}   // 图片源（base64 Data URL）
                    alt={attachment.name || 'preview'}  // 替代文本
                    style={{
                        width: '100%',          // 宽度撑满容器
                        height: '100%',         // 高度撑满容器
                        objectFit: 'cover',     // 裁剪模式（保持比例填充）
                    }}
                />
            </div>

            {/* 删除按钮 - 位于右上角 */}
            {renderRemoveButton(index)}
        </div>
    );

    /**
     * 渲染单个文件附件的预览卡片
     * 适用于文本文件和二进制文件
     *
     * @param {Object} attachment - 文件附件对象
     * @param {number} index - 附件在数组中的索引
     * @returns {React.ReactElement} 文件卡片元素
     */
    const renderFilePreview = (attachment: any, index: number) => {
        // 提取文件扩展名，用于选择图标和标签
        const ext = attachment.originalType || getExt(attachment.name);
        // 获取语言/类型标签文本
        const label = getLanguageLabel(ext, attachment.category);

        return (
            // 外层容器：包含文件卡片和删除按钮
            <div
                key={index}  // 使用索引作为 React key
                style={{
                    position: 'relative',   // 相对定位
                    padding: '6px 6px 0 0', // 为删除按钮预留空间
                }}
            >
                {/* 文件信息卡片 */}
                <div
                    style={{
                        position: 'relative',
                        width: 120,                  // 卡片宽度 120px（比图片缩略图宽，需要显示文件名）
                        height: 64,                  // 卡片高度 64px（与图片缩略图对齐）
                        borderRadius: 8,             // 圆角
                        border: '1px solid var(--border-color-light, #e0e0e0)',  // 边框：使用全局浅色边框变量
                        backgroundColor: 'var(--bg-secondary, #f5f5f5)',  // 背景色：使用全局次级背景色变量
                        padding: '6px 8px',          // 内边距
                        display: 'flex',             // 弹性布局
                        flexDirection: 'column',     // 纵向排列（图标+文件名 / 状态标签）
                        justifyContent: 'center',    // 垂直居中
                        overflow: 'hidden',          // 超出部分隐藏
                        transition: 'transform 0.2s, box-shadow 0.2s',  // 悬停动画过渡
                        cursor: 'default',           // 默认光标（不可点击预览）
                    }}
                    onMouseEnter={(e) => {
                        // 鼠标进入：略微放大并添加阴影
                        e.currentTarget.style.transform = 'scale(1.02)';
                        e.currentTarget.style.boxShadow = '0 2px 8px rgba(0,0,0,0.1)';
                    }}
                    onMouseLeave={(e) => {
                        // 鼠标离开：恢复原始状态
                        e.currentTarget.style.transform = 'scale(1)';
                        e.currentTarget.style.boxShadow = 'none';
                    }}
                    title={attachment.name}  // 悬停显示完整文件名
                >
                    {/* 第一行：文件图标 + 文件名 */}
                    <div style={{
                        display: 'flex',          // 弹性布局
                        alignItems: 'center',     // 垂直居中
                        gap: 4,                   // 图标和文件名之间的间距
                        minWidth: 0,              // 允许缩小（配合 textOverflow）
                    }}>
                        {/* 文件类型图标 */}
                        {getFileIcon(ext, { fontSize: 14, flexShrink: 0 })}
                        {/* 文件名（超长时显示省略号） */}
                        <span style={{
                            fontSize: 11,           // 小字号
                            overflow: 'hidden',     // 溢出隐藏
                            textOverflow: 'ellipsis', // 显示省略号
                            whiteSpace: 'nowrap',   // 不换行
                            minWidth: 0,            // 允许缩小
                            lineHeight: 1.2,        // 行高
                        }}>
                            {attachment.name}
                        </span>
                    </div>

                    {/* 第二行：状态标签 */}
                    <div style={{
                        marginTop: 3,               // 与第一行的间距
                        display: 'flex',            // 弹性布局
                        alignItems: 'center',       // 垂直居中
                        gap: 3,                     // 图标和文字之间的间距
                    }}>
                        {/* 根据附件状态显示不同的标签 */}
                        {renderStatusLabel(attachment)}
                    </div>
                </div>

                {/* 删除按钮 - 位于右上角 */}
                {renderRemoveButton(index)}
            </div>
        );
    };

    /**
     * 渲染文件状态标签
     * 根据附件的转换状态显示不同的图标和文字
     *
     * @param {Object} attachment - 附件对象
     * @returns {React.ReactElement} 状态标签元素
     */
    const renderStatusLabel = (attachment: any) => {
        // 文本文件：直接显示语言/类型标签
        if (attachment.category === 'text') {
            return (
                <span style={{
                    fontSize: 10,                // 小字号
                    color: 'var(--color-primary)',            // 主题色文字
                    backgroundColor: 'var(--color-primary-bg)',  // 主题色背景
                    padding: '1px 4px',          // 内边距
                    borderRadius: 3,             // 圆角
                    lineHeight: 1.2,             // 行高
                }}>
                    {getLanguageLabel(
                        attachment.language || attachment.name.split('.').pop().toLowerCase(),
                        attachment.category
                    )}
                </span>
            );
        }

        // 二进制文件：根据转换状态显示不同标签
        if (attachment.category === 'binary') {
            const ext = attachment.originalType || '';

            // 转换中状态：显示加载图标和"转换中"文字
            if (attachment.status === 'converting') {
                return (
                    <span style={{
                        fontSize: 10,            // 小字号
                        color: 'var(--text-warning)',        // 橙色文字
                        display: 'flex',         // 弹性布局
                        alignItems: 'center',    // 垂直居中
                        gap: 3,                  // 图标和文字间距
                    }}>
                        {/* 加载旋转图标 */}
                        <LoadingOutlined style={{ fontSize: 10 }} />
                        转换中...
                    </span>
                );
            }

            // 转换完成状态：显示绿色对勾和类型标签
            if (attachment.status === 'ready') {
                return (
                    <span style={{
                        fontSize: 10,                // 小字号
                        color: 'var(--status-success)',            // 绿色文字
                        backgroundColor: 'var(--bg-secondary)',  // 次级背景色
                        padding: '1px 4px',          // 内边距
                        borderRadius: 3,             // 圆角
                        display: 'flex',             // 弹性布局
                        alignItems: 'center',        // 垂直居中
                        gap: 2,                      // 图标和文字间距
                        lineHeight: 1.2,             // 行高
                    }}>
                        {/* 绿色对勾图标 */}
                        <CheckCircleOutlined style={{ fontSize: 9 }} />
                        {getLanguageLabel(ext, 'binary')}
                    </span>
                );
            }

            // 转换失败状态：显示红色感叹号和"失败"文字
            if (attachment.status === 'error') {
                return (
                    <span style={{
                        fontSize: 10,                // 小字号
                        color: 'var(--text-error)',            // 错误文字颜色
                        backgroundColor: 'var(--bg-error-tint)',  // 错误背景色
                        padding: '1px 4px',          // 内边距
                        borderRadius: 3,             // 圆角
                        display: 'flex',             // 弹性布局
                        alignItems: 'center',        // 垂直居中
                        gap: 2,                      // 图标和文字间距
                        lineHeight: 1.2,             // 行高
                    }}
                    title={attachment.error || '转换失败'}  // 悬停显示错误信息
                    >
                        {/* 红色感叹号图标 */}
                        <ExclamationCircleOutlined style={{ fontSize: 9 }} />
                        失败
                    </span>
                );
            }
        }

        // 默认：显示文件类型标签
        return (
            <span style={{
                fontSize: 10,                // 小字号
                color: 'var(--text-tertiary)',            // 辅助文字颜色
                backgroundColor: 'var(--bg-secondary)',  // 次级背景色
                padding: '1px 4px',          // 内边距
                borderRadius: 3,             // 圆角
                lineHeight: 1.2,             // 行高
            }}>
                {attachment.name.split('.').pop().toUpperCase()}
            </span>
        );
    };

    /**
     * 渲染删除按钮（位于附件右上角的圆形×按钮）
     * 同时用于图片和文件附件
     *
     * @param {number} index - 附件在数组中的索引
     * @returns {React.ReactElement} 删除按钮元素
     */
    const renderRemoveButton = (index: number) => (
        <button
            onClick={(e) => {
                e.stopPropagation();      // 阻止冒泡，防止触发父元素的点击事件
                onRemove?.(index);        // 调用删除回调（安全调用）
            }}
            style={{
                position: 'absolute',     // 绝对定位（相对于外层容器）
                top: 0,                   // 贴顶
                right: 0,                 // 贴右
                width: 18,                // 按钮直径 18px
                height: 18,
                padding: 0,               // 去除内边距
                border: 'none',           // 去除边框
                borderRadius: '50%',      // 圆形按钮
                backgroundColor: 'rgba(0, 0, 0, 0.45)',  // 半透明黑色背景
                cursor: 'pointer',        // 手型光标
                display: 'flex',          // 弹性布局
                alignItems: 'center',     // 垂直居中
                justifyContent: 'center', // 水平居中
                transition: 'background-color 0.2s',  // 背景色过渡动画
                zIndex: 1,                // 确保在图片/卡片之上
            }}
            onMouseEnter={(e) => {
                // 鼠标悬停：加深背景色
                e.currentTarget.style.backgroundColor = 'rgba(0, 0, 0, 0.65)';
            }}
            onMouseLeave={(e) => {
                // 鼠标离开：恢复背景色
                e.currentTarget.style.backgroundColor = 'rgba(0, 0, 0, 0.45)';
            }}
            title="移除附件"  // 悬停提示
        >
            {/* 关闭图标（×） */}
            <CloseOutlined style={{ fontSize: 10, color: 'var(--text-inverted)' }} />
        </button>
    );

    // ========== 主渲染 ==========
    return (
        <>
            {/* 附件预览网格容器 */}
            <div
                style={{
                    display: 'flex',           // 弹性布局
                    flexWrap: 'wrap',           // 允许换行（附件过多时自动换到下一行）
                    gap: 12,                   // 附件之间的间距 12px
                    padding: '8px 12px',       // 内边距
                    borderBottom: '1px solid var(--border-color-light, #e0e0e0)',  // 底部分隔线：使用全局浅色边框变量
                }}
            >
                {/* 遍历渲染每个附件的预览 */}
                {attachments.map((attachment, index) => {
                    // 根据附件类别选择不同的预览渲染方式
                    if (attachment.category === 'image') {
                        // 图片附件：渲染缩略图
                        return renderImagePreview(attachment, index);
                    } else {
                        // 文本/二进制附件：渲染文件卡片
                        return renderFilePreview(attachment, index);
                    }
                })}
            </div>

            {/* 图片大图预览通过 NiceModal 命令式调用，无需在此渲染 */}
        </>
    );
};

// 导出 AttachmentPreview 组件
export default AttachmentPreview;
