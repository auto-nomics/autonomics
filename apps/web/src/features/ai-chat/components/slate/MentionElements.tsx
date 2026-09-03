/**
 * Slate Mention 元素组件集合
 *
 * 包含 Slate 编辑器中所有自定义元素的渲染逻辑：
 * - VibeCardMention：VibeCard @提及 的胶囊标签渲染
 * - FileMentionElement：文件路径 @提及 的胶囊标签渲染（点击可复制完整路径）
 * - Element：Slate renderElement 回调的 dispatcher，根据 element.type 分发到上述组件
 * - Leaf：Slate renderLeaf 回调，当前为简单 span 包裹（预留富文本格式扩展）
 *
 * 抽出原因：原 SlateInputWithSender 内的渲染组件集合，与输入/发送主流程无耦合，
 * 独立成文件让 SlateInputWithSender 专注于消息构建和发送逻辑。
 *
 * @module ai-chat/components/slate/MentionElements
 */

import React from 'react';
import { Descendant } from 'slate';
import { useFocused, useSelected } from 'slate-react';

// ===== Slate 元素类型定义 =====

/** VibeCard mention 元素类型 */
export interface VibeCardMentionElement {
    type: 'vibecard-mention';
    vibeCardId: string;
    vibeCardName: string;
    children: Descendant[];
}

/** File mention 元素类型 */
export interface FileMentionElement {
    type: 'file-mention';
    filePath: string;
    fileName: string;
    children: Descendant[];
}

/** 段落元素类型 */
export interface ParagraphElement {
    type: 'paragraph';
    children: Array<Descendant | VibeCardMentionElement | FileMentionElement>;
}

// ===== Mention 渲染组件 =====

/**
 * VibeCard Mention 渲染组件
 * 用于在 Slate 编辑器中渲染 VibeCard 的 @提及 元素
 * 当用户在输入框中引用一个 VibeCard（如文献卡片）时，
 * 该组件会将该引用渲染为一个带圆角边框的标签样式
 *
 * @param {Object} attributes - Slate 传递的 DOM 属性（必须展开到最外层元素上）
 * @param {React.ReactNode} children - Slate 管理的子节点（光标位置等）
 * @param {Object} element - Slate 元素数据，包含 vibeCardId 和 vibeCardName
 */
export const VibeCardMention = ({ attributes, children, element }: { attributes: React.HTMLAttributes<HTMLElement>; children: React.ReactNode; element: VibeCardMentionElement }) => {
    const selected = useSelected();
    const focused = useFocused();

    return (
        <span
            {...attributes}
            contentEditable={false}
            style={{
                padding: '2px 6px',
                margin: '0 2px',
                verticalAlign: 'baseline',
                display: 'inline-block',
                borderRadius: '12px',
                backgroundColor: 'var(--bg-secondary)',
                border: '1px solid var(--border-color)',
                color: 'var(--text-primary)',
                fontSize: '0.9em',
                fontWeight: '500',
                boxShadow: selected && focused ? '0 0 0 2px rgba(0,0,0,0.06)' : 'none',
                cursor: 'default',
                transition: 'background-color .2s, border-color .2s, box-shadow .2s',
            }}
        >
            <span contentEditable={false}>
                @{element.vibeCardName || element.vibeCardId}
                {children}
            </span>
        </span>
    );
};

/**
 * File Mention 渲染组件
 * 用于在 Slate 编辑器中渲染文件路径的 @提及 元素
 * 显示为带文件图标的胶囊标签，点击可复制完整路径
 */
export const FileMentionElement = ({ attributes, children, element }: { attributes: React.HTMLAttributes<HTMLElement>; children: React.ReactNode; element: FileMentionElement }) => {
    const selected = useSelected();
    const focused = useFocused();

    return (
        <span
            {...attributes}
            contentEditable={false}
            title={element.filePath}
            style={{
                padding: '2px 6px',
                margin: '0 2px',
                verticalAlign: 'baseline',
                display: 'inline-block',
                borderRadius: '12px',
                backgroundColor: 'var(--bg-secondary)',
                border: '1px solid var(--border-color)',
                color: 'var(--text-primary)',
                fontSize: '0.9em',
                fontWeight: '500',
                boxShadow: selected && focused ? '0 0 0 2px rgba(0,0,0,0.06)' : 'none',
                cursor: 'default',
                transition: 'background-color .2s, border-color .2s, box-shadow .2s',
                maxWidth: '200px',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
            }}
        >
            <span contentEditable={false}>
                {element.filePath?.endsWith('/') || !element.filePath?.includes('.') ? '📁' : '📄'}{element.fileName || element.filePath}
                {children}
            </span>
        </span>
    );
};

/**
 * Slate 元素渲染器（Element）
 * 根据 Slate 元素的 type 字段，决定使用哪个组件来渲染该元素
 * 这是 Slate.js 的标准模式：通过 renderElement 回调为不同类型的元素提供不同的渲染方式
 *
 * @returns 对应类型的 React 渲染结果
 */
export const Element = ({ attributes, children, element }: { attributes: React.HTMLAttributes<HTMLElement>; children: React.ReactNode; element: VibeCardMentionElement | FileMentionElement | ParagraphElement }) => {
    switch (element.type) {
        case 'vibecard-mention':
            return <VibeCardMention attributes={attributes} children={children} element={element} />;
        case 'file-mention':
            return <FileMentionElement attributes={attributes} children={children} element={element} />;
        default:
            return <div {...attributes}>{children}</div>;
    }
};

/**
 * Slate 叶子节点渲染器（Leaf）
 * 用于渲染文本的格式化样式（如加粗、斜体等）
 * 当前实现为简单的 span 包裹，未添加任何特殊格式化样式
 * 可在此扩展支持加粗、斜体、代码等富文本格式
 *
 * @returns 渲染后的 span 元素
 */
export const Leaf = ({ attributes, children }: { attributes: React.HTMLAttributes<HTMLElement>; children: React.ReactNode }) => {
    return <span {...attributes}>{children}</span>;
};
