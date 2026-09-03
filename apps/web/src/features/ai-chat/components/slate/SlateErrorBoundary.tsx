/**
 * Slate 编辑器错误边界
 *
 * 捕获 Slate.js 在 DOM 事件处理中抛出的 "Cannot resolve a DOM node" 错误。
 * 这类错误是非致命的（编辑器仍可正常工作），但会在控制台产生大量错误日志。
 * 错误边界捕获后会静默重新挂载编辑器，恢复干净的 DOM 映射状态。
 *
 * 抽出原因：原 SlateInputWithSender 的独立 class，跟编辑器主流程无耦合，
 * 独立成文件让 SlateInputWithSender 专注于输入/发送逻辑。
 *
 * @module ai-chat/components/slate/SlateErrorBoundary
 */

import React, { Component } from 'react';

/** SlateErrorBoundary Props 类型 */
export interface SlateErrorBoundaryProps {
    onSlateError?: () => void;
    children: React.ReactNode;
}

export interface SlateErrorBoundaryState {
    hasError: boolean;
}

class SlateErrorBoundary extends Component<SlateErrorBoundaryProps, SlateErrorBoundaryState> {
    constructor(props: SlateErrorBoundaryProps) {
        super(props);
        this.state = { hasError: false };
    }

    static getDerivedStateFromError(error: Error): SlateErrorBoundaryState | null {
        const isSlateDomError = error?.message?.includes('Cannot resolve a DOM node from Slate node');
        if (isSlateDomError) {
            return { hasError: true };
        }
        throw error;
    }

    componentDidCatch(_error: Error, _errorInfo: React.ErrorInfo): void {
        // 通知父组件重建 editor 实例，清除过期的 DOM 映射
        if (this.props.onSlateError) {
            this.props.onSlateError();
        }
    }

    render() {
        if (this.state.hasError) {
            requestAnimationFrame(() => {
                this.setState({ hasError: false });
            });
            return <div style={{ minHeight: '36px' }} />;
        }
        return this.props.children;
    }
}

export default SlateErrorBoundary;
