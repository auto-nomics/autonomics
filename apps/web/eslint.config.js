// JayRead 前端 ESLint 配置文件
// 使用 ESLint 9+ 的 flat config 格式（替代旧的 .eslintrc.js 格式）
//
// 已知缺口：本配置 files 只覆盖 *.{js,jsx}，而 src/ 全是 ts/tsx——lint 实际
// 是空转。要补 TS 检查需引入 typescript-eslint（parser + 规则集），属独立
// 工作项；在此之前 package.json 的 lint 脚本用显式 glob + --no-error-on-
// unmatched-pattern 保持 CI 可过（ESLint 9 对"全被忽略"报 fatal）。

// 导入 ESLint 官方推荐的 JavaScript 规则集
import js from '@eslint/js';
// 导入 React 插件，用于检测 React 代码中的常见问题和最佳实践
import reactPlugin from 'eslint-plugin-react';
// 导入 React Hooks 插件，用于强制执行 React Hooks 的使用规则（如不能在循环/条件中调用 hooks）
import reactHooksPlugin from 'eslint-plugin-react-hooks';
// 导入 Prettier 配置，必须放在最后，禁用所有与 Prettier 格式化冲突的 ESLint 规则
import prettierConfig from 'eslint-config-prettier';

// 导出配置数组（flat config 格式）
export default [
  // 1. 应用 ESLint 官方推荐的 JavaScript 规则
  // 包含常见的语法错误检测、最佳实践等基础规则
  js.configs.recommended,

  // 2. 针对前端源码的规则配置
  {
    // 仅匹配 src 目录下的 .js 和 .jsx 文件
    files: ['src/**/*.{js,jsx}'],

    // 注册插件（需要在 rules 中引用）
    plugins: {
      // React 插件，提供 react 相关的规则
      react: reactPlugin,
      // React Hooks 插件，提供 hooks 相关的规则
      'react-hooks': reactHooksPlugin,
    },

    // 插件设置
    settings: {
      // 自动检测 React 版本，避免手动配置
      react: { version: 'detect' },
    },

    // 自定义规则配置
    rules: {
      // 展开 React 插件的推荐规则（包含 JSX 语法、组件命名等规则）
      ...reactPlugin.configs.recommended.rules,
      // 展开 React JSX Runtime 规则（使用新的 JSX 转换时不需导入 React）
      ...reactPlugin.configs['jsx-runtime'].rules,
      // 展开 React Hooks 插件的推荐规则（强制 hooks 的正确使用）
      ...reactHooksPlugin.configs.recommended.rules,

      // 关闭 prop-types 规则（项目使用 TypeScript 类型注解，不需要 PropTypes）
      'react/prop-types': 'off',

      // 未使用变量设置为警告（而非错误），允许以下划线 _ 开头的参数（表示故意不使用）
      'no-unused-vars': ['warn', { argsIgnorePattern: '^_' }],
    },
  },

  // 3. Prettier 配置（必须放在最后）
  // 禁用所有与 Prettier 格式化冲突的格式化规则（如缩进、引号、分号等）
  prettierConfig,
];
