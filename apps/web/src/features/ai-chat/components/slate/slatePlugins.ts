/**
 * Slate 编辑器扩展插件集合
 *
 * 包含 Slate 编辑器的自定义插件和插入逻辑：
 * - withVibeCardMentions：扩展编辑器，让 vibecard-mention / file-mention 元素被识别为
 *   inline + void（行内不可编辑的胶囊标签）
 * - insertVibeCardMention：插入 VibeCard mention，可选先删除触发 mention 的搜索文本
 * - insertFileMention：插入文件 mention，同上
 *
 * 类型 CustomEditor / VibeCardData 也在此处导出，因为插件函数依赖它们，
 * 集中放置避免类型分散。
 *
 * 抽出原因：原 SlateInputWithSender 内的纯函数集合，与组件状态/UI 无耦合，
 * 独立成文件让 SlateInputWithSender 专注于编辑器实例和发送逻辑。
 *
 * @module ai-chat/components/slate/slatePlugins
 */

import { BaseEditor, Editor, Node, Range, Transforms } from 'slate';
import { ReactEditor } from 'slate-react';

/** 自定义 Editor 类型（整合 React + History 插件） */
export type CustomEditor = BaseEditor & ReactEditor;

/** VibeCard 数据类型（VibeCard mention 插入时所需的最小字段集） */
export interface VibeCardData {
    id: string;
    name: string;
    type?: string;
    vibeCardType?: string;
}

/**
 * Slate 编辑器扩展插件：withVibeCardMentions
 * 扩展编辑器以支持 VibeCard mention 元素
 * 通过重写 editor.isInline 和 editor.isVoid 方法，
 * 告诉 Slate 将 vibecard-mention 类型的元素视为行内元素和空元素（Void）
 *
 * - isInline = true：元素在文本行内显示（类似 <a> 或 <span>），而非独占一行（如 <div>）
 * - isVoid = true：元素没有可编辑的子内容，Slate 会将其视为一个不可编辑的整体
 */
export const withVibeCardMentions = (editor: CustomEditor): CustomEditor => {
    const { isInline, isVoid } = editor;

    editor.isInline = (element: Node) => {
        const el = element as { type?: string };
        return (el.type === 'vibecard-mention' || el.type === 'file-mention') ? true : isInline(element as Parameters<typeof isInline>[0]);
    };

    editor.isVoid = (element: Node) => {
        const el = element as { type?: string };
        return (el.type === 'vibecard-mention' || el.type === 'file-mention') ? true : isVoid(element as Parameters<typeof isVoid>[0]);
    };

    return editor;
};

/**
 * 插入 VibeCard mention 到编辑器
 * 在当前光标位置（或指定范围）插入一个 VibeCard mention 元素
 * 如果提供了 targetRange 和 searchText，会先删除搜索文本（如 "@卡片名"），再插入 mention
 *
 * @param editor - Slate 编辑器实例
 * @param vibeCardData - VibeCard 数据，包含 id 和 name 字段
 * @param targetRange - 可选的目标范围，用于删除触发 mention 的搜索文本
 * @param searchText - 搜索文本（用于计算需要删除的字符范围长度）
 */
export const insertVibeCardMention = (editor: CustomEditor, vibeCardData: VibeCardData, targetRange: Range | null = null, searchText: string = ''): void => {
    const mention = {
        type: 'vibecard-mention' as const,
        vibeCardId: vibeCardData.id,
        vibeCardName: vibeCardData.name,
        children: [{ text: '' }],
    };

    if (targetRange) {
        try {
            const endPoint = Range.end(targetRange);
            const distance = (searchText?.length || 0) + 1;
            const anchorPoint = Editor.before(editor, endPoint, { distance, unit: 'character' });
            if (anchorPoint) {
                const exactRange = { anchor: anchorPoint, focus: endPoint };
                Transforms.select(editor, exactRange);
                Transforms.delete(editor);
            } else {
                Transforms.select(editor, targetRange);
                Transforms.delete(editor);
            }
        } catch {
            Transforms.select(editor, targetRange);
            Transforms.delete(editor);
        }
    }

    Transforms.insertNodes(editor, mention);
    Transforms.move(editor);
};

/**
 * 插入文件 mention 到编辑器
 * 在当前光标位置（或指定范围）插入一个文件路径 mention 元素
 * 如果提供了 targetRange 和 searchText，会先删除搜索文本（如 "@readme"），再插入 mention
 */
export const insertFileMention = (editor: CustomEditor, fileData: { path: string; name: string }, targetRange: Range | null = null, searchText: string = ''): void => {
    const mention = {
        type: 'file-mention' as const,
        filePath: fileData.path,
        fileName: fileData.name,
        children: [{ text: '' }],
    };

    if (targetRange) {
        try {
            const endPoint = Range.end(targetRange);
            const distance = (searchText?.length || 0) + 1;
            const anchorPoint = Editor.before(editor, endPoint, { distance, unit: 'character' });
            if (anchorPoint) {
                const exactRange = { anchor: anchorPoint, focus: endPoint };
                Transforms.select(editor, exactRange);
                Transforms.delete(editor);
            } else {
                Transforms.select(editor, targetRange);
                Transforms.delete(editor);
            }
        } catch {
            Transforms.select(editor, targetRange);
            Transforms.delete(editor);
        }
    }

    Transforms.insertNodes(editor, mention);
    Transforms.move(editor);
};
