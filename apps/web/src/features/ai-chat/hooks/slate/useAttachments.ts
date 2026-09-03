/**
 * 附件管理 Hook
 *
 * 统一管理 SlateInputWithSender 的附件状态和处理逻辑：
 * - 状态：attachedFiles / isDragging / dragCounterRef
 * - handleFileSelect：根据 category 添加附件（image 直接 / text 直接 / binary 调 parseFile）
 * - handleAttachmentRemove：按索引删除
 * - handlePaste：剪贴板文件收集 + 去重 + processFiles
 * - handleFileDrop：拖拽文件（不含 VibeCard 部分，VibeCard 在 SlateInputWithSender 处理）
 *
 * 关键改进：原 handlePaste 与 handleDrop 内部"逐个文件分类+读取+handleFileSelect"的循环
 * 是 verbatim 重复（~50 行），抽到 processFiles 共享。
 *
 * 抽出原因：原 SlateInputWithSender 内附件逻辑 ~300 行 + paste/drop 重复，
 * 独立成 hook 让 SlateInputWithSender 专注于编辑器/发送主流程。
 *
 * @module ai-chat/hooks/slate/useAttachments
 */

import { useCallback, useRef, useState } from 'react';
import { App as AntApp } from 'antd';
import { classifyFile } from '../../components/FileUploader';
import { parseFile } from '../../../../services/filesApi';
import { zoteroL10n } from '../../zoteroL10n';
import type { ModelSelection } from '../useModelConfig';

/** 附件类型（图片/文本/二进制统一表示） */
export interface AttachmentFile {
    category: 'image' | 'text' | 'binary';
    name: string;
    base64?: string;
    sourceType?: string;
    textContent?: string;
    language?: string;
    originalType?: string;
    markdown?: string;
    status?: 'converting' | 'ready' | 'error';
    error?: string;
    file?: File;
}

/** 文件大小上限：10MB */
const MAX_FILE_BYTES = 10 * 1024 * 1024;

/** useInputHistory 参数 */
interface UseAttachmentsParams {
    /** 当前模型是否支持图片/多模态输入 */
    visionCapable: boolean;
    /** 当前选中的模型（用于不支持图片时给出提示） */
    currentModel?: ModelSelection;
}

/**
 * 附件管理 Hook
 */
export function useAttachments({ visionCapable, currentModel }: UseAttachmentsParams) {
    // ===== antd message（通过 App.useApp 拿到带 context 的实例） =====
    // 这里使用 hook 形式而非静态 import，确保 message 弹窗挂载在当前 React 树下
    // 注意：调用方必须在 <App> context 内渲染，否则 antd 会回退到无样式 message
    const { message: antMessage } = AntApp.useApp();

    // ===== 状态 =====
    const [attachedFiles, setAttachedFiles] = useState<AttachmentFile[]>([]);
    const [isDragging, setIsDragging] = useState(false);
    const dragCounterRef = useRef(0);

    /** 从 MIME 类型提取图片扩展名（image/png → png，image/jpeg → jpg，无法识别 → png） */
    const extFromImageMime = useCallback((mime: string): string => {
        if (!mime || !mime.startsWith('image/')) return 'png';
        const sub = mime.slice('image/'.length).toLowerCase();
        if (sub === 'jpeg') return 'jpg';
        return sub.replace(/[^a-z0-9]/g, '') || 'png';
    }, []);

    /**
     * 统一的附件添加入口（FileUploader 输出 / paste / drop 都走这里）
     * 根据 category 执行不同逻辑：
     * - image：检查 visionCapable → 直接加入附件列表
     * - text：直接加入附件列表（前端已读取）
     * - binary：先以 'converting' 占位 → 调 parseFile → 更新为 'ready' 或 'error'
     */
    const handleFileSelect = useCallback(async (fileData: AttachmentFile & { file?: File }) => {
        if (fileData.category === 'image') {
            if (!visionCapable) {
                antMessage.warning(
                    zoteroL10n('vibe-ai-chat-multimodal-not-supported', {
                        model: currentModel?.label || currentModel?.key || '',
                    } as Record<string, string>, '')
                );
                return;
            }
            setAttachedFiles(prev => [...prev, {
                category: 'image',
                base64: fileData.base64,
                name: fileData.name,
                sourceType: fileData.sourceType || 'upload',
            }]);
            antMessage.success('已添加图片');
        } else if (fileData.category === 'text') {
            setAttachedFiles(prev => [...prev, {
                category: 'text',
                name: fileData.name,
                textContent: fileData.textContent,
                language: fileData.language || '',
            }]);
            antMessage.success(`已添加文件: ${fileData.name}`);
        } else if (fileData.category === 'binary') {
            // 先以 'converting' 占位，让用户立即看到加载卡片
            const tempAttachment: AttachmentFile = {
                category: 'binary',
                name: fileData.name,
                originalType: fileData.originalType,
                markdown: '',
                status: 'converting',
            };
            setAttachedFiles(prev => [...prev, tempAttachment]);
            try {
                const result = await parseFile(fileData.file!);
                setAttachedFiles(prev => prev.map(a =>
                    a === tempAttachment
                        ? { ...a, markdown: result.markdown || '', status: 'ready' }
                        : a
                ));
                antMessage.success(`文件解析完成: ${fileData.name}`);
            } catch (err) {
                console.error('[useAttachments] 文件解析失败:', err);
                setAttachedFiles(prev => prev.map(a =>
                    a === tempAttachment
                        ? { ...a, status: 'error' as const, error: (err as Error)?.message || '文件解析失败' }
                        : a
                ));
                antMessage.error(`文件解析失败: ${fileData.name}`);
            }
        }
    }, [visionCapable, currentModel?.label, currentModel?.key, antMessage]);

    /** 移除指定索引的附件 */
    const handleAttachmentRemove = useCallback((index: number) => {
        setAttachedFiles(prev => prev.filter((_, i) => i !== index));
    }, []);

    /** 读取图片文件为 base64 Data URL（Promise 包装 FileReader） */
    const readAsDataURL = (file: File): Promise<string | undefined> => new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = (e) => resolve(e.target?.result as string);
        reader.onerror = () => reject(new Error('读取图片失败'));
        reader.readAsDataURL(file);
    });

    /** 读取文本文件为 UTF-8 字符串（Promise 包装 FileReader） */
    const readAsText = (file: File): Promise<string> => new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = (e) => resolve(e.target?.result as string);
        reader.onerror = () => reject(new Error('读取文件失败'));
        reader.readAsText(file);
    });

    /**
     * 共享的"逐文件分类+读取+handleFileSelect"循环
     * 原 handlePaste 与 handleDrop 内部这一段是 verbatim 重复，统一在这里
     *
     * @param files 文件列表
     * @param sourceType 'paste' | 'drop'，用于命名兜底文件名和 sourceType 字段
     */
    const processFiles = useCallback(async (files: File[], sourceType: 'paste' | 'drop') => {
        for (let i = 0; i < files.length; i++) {
            const file = files[i];
            // 大小校验（统一 10MB）
            if (file.size > MAX_FILE_BYTES) {
                antMessage.error(`文件 ${file.name || i} 大小不能超过 10MB`);
                continue;
            }

            if (file.type?.startsWith('image/')) {
                // 图片：读 base64 → handleFileSelect
                let base64: string | undefined;
                try {
                    base64 = await readAsDataURL(file);
                } catch (err) {
                    console.error(`[useAttachments] ${sourceType} 读图失败:`, err);
                    antMessage.error((err as Error)?.message || '读取图片失败');
                    continue;
                }
                if (!base64) continue;

                const ext = extFromImageMime(file.type);
                const safeName = file.name && String(file.name).trim()
                    ? file.name
                    : `${sourceType}_${Date.now()}_${i}.${ext}`;

                await handleFileSelect({
                    category: 'image',
                    base64,
                    file,
                    sourceType,
                    name: safeName,
                });
            } else {
                // 非图片：classifyFile 分类 → 文本前端读 / 二进制交给 handleFileSelect 调 parseFile
                const classification = classifyFile(file.name || '');
                const fileData: AttachmentFile & { file?: File; category: 'text' | 'binary'; language?: string; textContent?: string; originalType?: string } = {
                    name: file.name,
                    file,
                    category: 'binary',
                };

                if (classification.category === 'text') {
                    fileData.category = 'text';
                    fileData.language = classification.lang || '';
                    try {
                        fileData.textContent = await readAsText(file);
                    } catch (err) {
                        console.error(`[useAttachments] ${sourceType} 读取文件失败:`, err);
                        antMessage.error((err as Error)?.message || '读取文件失败');
                        continue;
                    }
                } else {
                    fileData.category = 'binary';
                    fileData.originalType = classification.ext;
                }
                await handleFileSelect(fileData);
            }
        }
    }, [extFromImageMime, handleFileSelect, antMessage]);

    /**
     * 粘贴事件处理：收集剪贴板文件（去重）+ processFiles
     * 没有文件时返回 false，调用方让 Slate 默认粘贴文本
     */
    const handlePaste = useCallback(async (event: React.ClipboardEvent): Promise<void> => {
        const dt = event.clipboardData;
        if (!dt) return;

        // 收集 + 去重（dt.items 与 dt.files 是镜像关系，同时遍历会重复）
        const allFiles: File[] = [];
        const seen = new Set<string>();
        const pushFile = (file: File | null) => {
            if (!file) return;
            const key = `${file.size}:${file.type}:${file.lastModified}`;
            if (seen.has(key)) return;
            seen.add(key);
            allFiles.push(file);
        };

        if (dt.items?.length) {
            for (let i = 0; i < dt.items.length; i++) {
                const item = dt.items[i];
                if (item.kind === 'file') pushFile(item.getAsFile());
            }
        } else if (dt.files?.length) {
            for (let i = 0; i < dt.files.length; i++) pushFile(dt.files[i]);
        }

        if (allFiles.length === 0) return;

        event.preventDefault();
        event.stopPropagation();
        await processFiles(allFiles, 'paste');
    }, [processFiles]);

    /**
     * 拖拽放置事件中的"文件部分"处理
     * VibeCard 拖拽由调用方自行处理（需要 editor），此处只处理 File 列表
     *
     * @returns true 表示已处理（含文件），false 表示无文件
     */
    const handleFileDrop = useCallback(async (files: File[]): Promise<void> => {
        if (files.length === 0) return;
        await processFiles(files, 'drop');
    }, [processFiles]);

    return {
        // 状态
        attachedFiles,
        setAttachedFiles,
        isDragging,
        setIsDragging,
        dragCounterRef,
        // actions
        handleFileSelect,
        handleAttachmentRemove,
        handlePaste,
        handleFileDrop,
    };
}
