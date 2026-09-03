/**
 * 功能级错误边界组件
 *
 * 包裹在 PdfViewer、ChatPanel、ParagraphGuide 等核心功能区外部。
 * 单个面板崩溃不会影响其他面板，只显示局部降级 UI。
 *
 * 与根级 ErrorBoundary 的区别：
 * - 根级 ErrorBoundary：整个应用白屏时显示全屏错误页
 * - FeatureErrorBoundary：单个功能面板崩溃时显示局部降级 UI
 *
 * 设计理念：
 * - 使用 React 类组件实现（错误边界 API 只支持类组件）
 * - 提供默认降级 UI，也支持自定义 fallback
 * - 支持面板名称显示，便于用户识别哪个功能出错
 * - 提供重试功能，方便用户恢复面板
 *
 * @module components/FeatureErrorBoundary
 */
import React from 'react'; // 导入 React 核心库
import { Alert, Button } from 'antd'; // 导入 Ant Design 的 Alert 警告组件和 Button 按钮组件
import {
  ReloadOutlined, // ReloadOutlined 重新加载图标
  WarningOutlined, // WarningOutlined 警告图标
} from '@ant-design/icons'; // 导入 Ant Design 图标组件
import i18next from 'i18next';

/**
 * FeatureErrorBoundary 功能级错误边界类组件
 *
 * 使用示例：
 * ```jsx
 * <FeatureErrorBoundary name="PDF 阅读器">
 *   <PdfViewer />
 * </FeatureErrorBoundary>
 *
 * // 自定义降级 UI
 * <FeatureErrorBoundary
 *   name="AI 对话"
 *   fallback={<div>对话功能暂时不可用</div>}
 * >
 *   <ChatPanel />
 * </FeatureErrorBoundary>
 * ```
 */
interface FeatureErrorBoundaryProps {
  name: string;
  children: React.ReactNode;
  fallback?: React.ReactNode;
}

interface FeatureErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
  errorInfo: { componentStack: string } | null;
}

class FeatureErrorBoundary extends React.Component<FeatureErrorBoundaryProps, FeatureErrorBoundaryState> {
  /**
   * 构造函数：初始化组件状态
   *
   * @param {object} props - 组件属性对象
   */
  constructor(props: FeatureErrorBoundaryProps) {
    super(props); // 调用父类 React.Component 的构造函数

    // 初始化状态：hasError 标记是否发生了错误
    // 初始值为 false，表示没有错误
    this.state = {
      hasError: false, // 错误状态标志：false=正常，true=出错
      error: null, // 捕获到的错误对象（便于调试和显示）
      errorInfo: null, // 错误详细信息（包含组件堆栈）
    };
  }

  /**
   * 静态方法：在错误抛出后立即调用，用于更新状态
   *
   * 这是 React 专门为错误边界设计的静态生命周期方法。
   * 在渲染阶段调用，不允许有副作用（如网络请求）。
   * 必须返回更新后的状态对象。
   *
   * @param {Error} error - 捕获到的错误对象
   * @returns {object} 更新后的状态对象
   */
  static getDerivedStateFromError(error: Error) {
    // 更新状态，使下一次渲染显示降级 UI
    return {
      hasError: true, // 设置错误标志为 true
      error, // 保存错误对象
    };
  }

  /**
   * 生命周期方法：在错误被捕获后调用，用于记录错误信息
   *
   * 这个方法在"提交阶段"被调用，允许执行副作用操作：
   * - 记录错误到日志服务
   * - 发送错误报告到监控平台
   * - 在控制台输出详细信息
   *
   * @param {Error} error - 捕获到的错误对象
   * @param {object} errorInfo - 包含错误堆栈信息的对象
   * @param {string} errorInfo.componentStack - 错误发生时的组件堆栈
   */
  componentDidCatch(error: Error, errorInfo: { componentStack: string }) {
    // 不降级，直接抛出错误让问题暴露
    console.error(`[${this.props.name}] 功能面板崩溃（不降级，直接抛出）:`, error);
    console.error('组件堆栈:', errorInfo.componentStack);
    throw error;
  }

  /**
   * 处理"重试"按钮点击事件
   *
   * 重试策略：
   * - 重置 hasError 状态为 false
   * - 这会导致组件重新渲染，尝试再次渲染子组件
   * - 如果错误是暂时性的（如网络波动），重试可能成功
   */
  handleRetry = () => {
    // 重置错误状态，触发重新渲染
    this.setState({ hasError: false, error: null, errorInfo: null }); // 清除所有错误状态
  };

  /**
   * 渲染方法：根据状态决定渲染正常内容或降级 UI
   *
   * @returns {React.ReactNode} 正常的子组件或错误提示 UI
   */
  render() {
    // 从 props 中解构获取属性
    const { name, children, fallback } = this.props; // name：面板名称，children：子组件，fallback：自定义降级 UI

    // 如果有错误发生，渲染降级 UI
    if (this.state.hasError) {
      // 如果用户提供了自定义降级 UI，直接渲染
      if (fallback) {
        return fallback; // 返回自定义降级 UI
      }

      // 使用默认降级 UI：居中显示的错误提示
      return (
        <div style={{
          // 容器样式：使用 flex 布局使内容居中
          display: 'flex', // 启用 flex 布局
          flexDirection: 'column', // 纵向排列子元素
          justifyContent: 'center', // 垂直居中
          alignItems: 'center', // 水平居中
          height: '100%', // 占满父容器高度
          padding: '24px', // 内边距 24px
          background: 'var(--bg-tertiary, #fafafa)', // 浅灰色背景：使用CSS变量适配深色模式
          color: 'var(--text-secondary, #666)', // 次要文字颜色：使用CSS变量适配深色模式
          textAlign: 'center', // 文本居中对齐
        }}>
          {/* Ant Design Alert 组件：提供统一的错误展示 UI */}
          <Alert
            type="warning" // 警告类型，显示黄色警告图标
            icon={<WarningOutlined />} // 使用警告图标
            message={i18next.t('common:feature.errorTitle', { name })} // 主标题：显示面板名称
            description={i18next.t('common:feature.errorDesc')} // 副标题说明
            showIcon // 显示图标
            style={{
              maxWidth: 400, // 最大宽度 400px
              marginBottom: 16, // 底部间距 16px
            }}
          />

          {/* 开发模式下显示详细错误信息 */}
          {process.env.NODE_ENV === 'development' && this.state.error && (
            <div style={{
              width: '100%',
              maxWidth: 400,
              marginBottom: 16,
              padding: 12,
              // 浅红色背景：Ant Design 错误色系，保持不变以维持错误提示的语义清晰度
              background: 'var(--bg-error-tint)',
              // 错误边框色
              border: '1px solid var(--border-error)',
              borderRadius: 4,
              fontSize: 12,
              // 错误文字颜色
              color: 'var(--text-error)',
              textAlign: 'left',
              whiteSpace: 'pre-wrap', // 保留换行和空格
              overflow: 'auto', // 内容过长时显示滚动条
              maxHeight: 200, // 最大高度 200px
            }}>
              <div style={{ fontWeight: 'bold', marginBottom: 4 }}>{i18next.t('common:error.info')}</div>
              <div>{this.state.error?.toString()}</div>
              {this.state.errorInfo?.componentStack && (
                <>
                  <div style={{ fontWeight: 'bold', marginBottom: 4, marginTop: 8 }}>{i18next.t('common:error.stack')}</div>
                  <div style={{ fontSize: 11 }}>{this.state.errorInfo.componentStack}</div>
                </>
              )}
            </div>
          )}

          {/* 重试按钮：点击后尝试重新渲染组件 */}
          <Button
            type="primary" // 主要按钮样式
            icon={<ReloadOutlined />} // 重新加载图标
            onClick={this.handleRetry} // 点击时重置错误状态
          >
            {i18next.t('common:retry')}
          </Button>
        </div>
      );
    }

    // 没有错误时，正常渲染子组件
    return children; // 返回子组件，让它们正常渲染
  }
}

// 导出功能级错误边界组件，供其他模块使用
export default FeatureErrorBoundary; // 使用默认导出
