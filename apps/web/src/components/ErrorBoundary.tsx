/**
 * Autonomics 错误边界组件
 *
 * 这是一个 React 类组件，用于捕获子组件树中的 JavaScript 错误。
 * 错误边界是 React 组件，可以在子组件树任何地方捕获 JavaScript 错误，
 * 记录这些错误，并显示降级 UI，而不是使整个应用崩溃。
 *
 * 为什么错误边界必须是类组件？
 * - React 的错误边界 API（getDerivedStateFromError 和 componentDidCatch）只在类组件中可用
 * - 函数组件暂不支持这些生命周期方法
 * - 这是 React 官方限制，与类组件和函数组件的实现机制有关
 *
 * 错误边界的工作原理：
 * - getDerivedStateFromError: 在错误抛出后立即渲染降级 UI
 * - componentDidCatch: 用于记录错误信息（如发送到错误监控服务）
 *
 * 错误边界无法捕获的错误：
 * - 事件处理器中的错误（如 onClick 中的错误）
 * - 异步代码中的错误（如 setTimeout、requestAnimationFrame）
 * - 服务端渲染错误
 * - 错误边界组件本身的错误
 *
 * @module components/ErrorBoundary
 */
import React from 'react'; // 导入 React 核心库
import { Result, Button } from 'antd'; // 导入 Ant Design 的 Result 结果展示组件和 Button 按钮组件
import i18next from 'i18next';

/**
 * ErrorBoundary 错误边界类组件
 *
 * 使用示例：
 * ```jsx
 * <ErrorBoundary>
 *   <App />
 * </ErrorBoundary>
 * ```
 */
interface ErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
  errorInfo: { componentStack: string } | null;
}

interface ErrorBoundaryProps {
  children: React.ReactNode;
}

class ErrorBoundary extends React.Component<ErrorBoundaryProps, ErrorBoundaryState> {
  /**
   * 构造函数：初始化组件状态
   *
   * @param {object} props - 组件属性对象
   */
  constructor(props: ErrorBoundaryProps) {
    super(props); // 调用父类 React.Component 的构造函数

    // 初始化状态：hasError 标记是否发生了错误
    // 初始值为 false，表示没有错误
    this.state = { hasError: false, error: null, errorInfo: null }; // 设置初始状态：没有错误
  }

  /**
   * 静态方法：在错误抛出后立即调用，用于更新状态
   *
   * 为什么使用静态方法？
   * - 这是 React 专门为错误边界设计的静态生命周期方法
   * - 在渲染阶段调用，不允许有副作用（如网络请求）
   * - 必须返回更新后的状态对象
   *
   * 这个方法在组件抛出错误后、渲染降级 UI 之前被调用，
   * 适合用来设置 hasError 状态标志位。
   *
   * @param {Error} error - 捕获到的错误对象
   * @returns {object} 更新后的状态对象
   */
  static getDerivedStateFromError(error: Error) {
    // 更新状态，使下一次渲染显示降级 UI
    return { hasError: true, error }; // 返回状态更新：标记有错误发生
  }

  /**
   * 生命周期方法：在错误被捕获后调用，用于记录错误信息
   *
   * 这个方法在"提交阶段"被调用，允许执行副作用操作：
   * - 记录错误到日志服务
   * - 发送错误报告到监控平台
   * - 在控制台输出详细信息
   *
   * 注意：此方法在开发模式下会被调用两次（严格模式），
   * 在生产模式下只会调用一次。
   *
   * @param {Error} error - 捕获到的错误对象
   * @param {object} errorInfo - 包含错误堆栈信息的对象
   * @param {string} errorInfo.componentStack - 错误发生时的组件堆栈
   */
  componentDidCatch(error: Error, errorInfo: { componentStack: string }) {
    // 在控制台输出错误信息，便于开发时调试
    console.error('ErrorBoundary 捕获到错误:', error); // 输出错误对象
    console.error('组件堆栈:', errorInfo.componentStack); // 输出组件堆栈信息
    this.setState({ errorInfo }); // 保存错误信息到状态

    // TODO: 在生产环境中，可以将错误发送到错误监控服务
    // 例如：logErrorToService(error, errorInfo);
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
    this.setState({ hasError: false }); // 清除错误标志，恢复正常渲染
  };

  /**
   * 处理"刷新页面"按钮点击事件
   *
   * 为什么需要刷新页面功能？
   * - 某些错误可能是状态累积导致的，重试无效
   * - 完整刷新可以重置所有状态和内存
   * - 给用户一个"强制重启"的选项
   */
  handleRefresh = () => {
    // 重新加载当前页面，重置整个应用状态
    window.location.reload(); // 调用浏览器 API 刷新页面
  };

  /**
   * 渲染方法：根据状态决定渲染正常内容或降级 UI
   *
   * @returns {React.ReactNode} 正常的子组件或错误提示 UI
   */
  render() {
    // 如果有错误发生，渲染降级 UI（错误提示页面）
    if (this.state.hasError) { // 检查错误状态标志
      return ( // 返回错误提示 UI
        <div style={{
          // 容器样式：使用 flex 布局使内容居中
          display: 'flex', // 启用 flex 布局
          justifyContent: 'center', // 水平居中
          alignItems: 'center', // 垂直居中
          minHeight: '100vh', // 最小高度占满整个视口高度
          padding: '24px', // 内边距 24px
          background: 'var(--bg-secondary)', // 次级背景色
        }}>
          {/* Ant Design Result 组件：提供统一的错误展示 UI */}
          <Result
            status="error" // 状态类型为"错误"，显示错误图标
            title={i18next.t('common:error.title')} // 主标题文字
            subTitle={i18next.t('common:error.subtitle')} // 副标题说明
            extra={[
              <div key="error-detail" style={{ textAlign: 'left', maxWidth: 800, margin: '0 auto 16px', padding: 16, background: 'var(--bg-error-tint)', borderRadius: 8, border: '1px solid var(--border-error)', whiteSpace: 'pre-wrap', fontSize: 13, fontFamily: 'monospace' }}>
                <div style={{ fontWeight: 'bold', marginBottom: 8, color: 'var(--text-error)' }}>{i18next.t('common:error.info')}</div>
                <div>{this.state.error?.toString()}</div>
                {this.state.errorInfo?.componentStack && (
                  <div style={{ marginTop: 8 }}>
                    <div style={{ fontWeight: 'bold', marginBottom: 4, color: 'var(--text-error)' }}>{i18next.t('common:error.stack')}</div>
                    <div>{this.state.errorInfo.componentStack}</div>
                  </div>
                )}
              </div>,
              <Button
                type="primary"
                key="retry"
                onClick={this.handleRetry}
              >
                {i18next.t('common:retry')}
              </Button>,
              <Button
                key="refresh"
                onClick={this.handleRefresh}
              >
                {i18next.t('common:refresh')}
              </Button>,
            ]}
          />
        </div>
      );
    }

    // 没有错误时，正常渲染子组件
    return this.props.children; // 返回子组件，让它们正常渲染
  }
}

// 导出错误边界组件，供其他模块使用
export default ErrorBoundary; // 使用默认导出
