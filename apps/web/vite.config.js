/**
 * JayRead Vite 配置
 * - 开发模式下代理 /api 请求到 Rust 后端（端口 3001）
 * - 代理 /data 请求到 Rust 后端静态文件服务
 * - 配置代码分割策略，优化打包体积
 */
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const host = process.env.TAURI_DEV_HOST || '127.0.0.1';
const apiPort = process.env.API_PORT || '8765';

export default defineConfig({
  plugins: [react({ jsxRuntime: 'automatic' })],

  // ESBuild 配置：生产环境移除 console 和 debugger
  esbuild: {
    drop: process.env.NODE_ENV === 'production' ? ['console', 'debugger'] : [],
  },

  // 禁用 module prefill polyfill（现代浏览器已支持）
  modulePreload: { polyfill: false },

  // WASM 文件支持：pdfium-viewer 使用 PDFium WASM 模块
  assetsInclude: ['**/*.wasm'],
  optimizeDeps: {
    exclude: ['@autonomics/pdfium-viewer'],
  },

  // Tauri build: use relative paths
  ...(process.env.TAURI_ENV ? { base: './' } : {}),
  test: { // Vitest 配置
    environment: 'happy-dom', // 使用 happy-dom 环境（支持 React/JSX）
    globals: true, // 启用全局测试函数（describe, it, expect 等）
    setupFiles: ['./src/test/setup.ts'], // 测试设置文件，注册 React 和 mock
    include: ['src/**/__tests__/**/*.{test,spec}.{js,jsx,ts,tsx}'], // 匹配测试文件
  },
  server: { // 开发服务器配置
    host, // 绑定 IPv4，确保 WebView 能连接
    strictPort: true,
    port: 5173, // 开发服务器端口号
    proxy: { // API 代理配置，将前端请求转发到后端
      '/api': {
        target: `http://localhost:${apiPort}`, // Rust 后端服务地址
        changeOrigin: true, // 修改请求头中的 Origin 为目标地址，解决跨域问题
        // SSE 支持：禁用代理缓冲，确保事件实时推送到浏览器
        configure: (proxy) => { // 代理配置回调函数
          proxy.on('proxyRes', (proxyRes, req, res) => { // 监听代理响应事件
            if (proxyRes.headers['content-type']?.includes('text/event-stream')) { // 检测是否为 SSE 流式响应
              // SSE 必须禁用缓冲，否则事件会被代理攒在一起一次性发送
              proxyRes.headers['cache-control'] = 'no-cache'; // 设置不缓存，确保实时推送
              proxyRes.headers['x-accel-buffering'] = 'no'; // 禁用 Nginx 缓冲（生产环境可能使用 Nginx）
            }
          });
        },
      },
    },
  },
  build: {
    // 目标浏览器环境：ES2020 提供现代 JavaScript 特性支持
    // ES2020 包含可选链 (?.)、空值合并 (??)、BigInt 等特性
    target: 'es2020',
    // CSS 代码分割：将每个组件的 CSS 拆分为独立文件
    // 便于并行加载和按需加载，提升首屏加载性能
    cssCodeSplit: true,
    // 禁用 source map：生产环境不生成源码映射，减小构建产物体积
    // 如需调试生产问题，可改为 true 或 'inline'
    sourcemap: false,
    rollupOptions: {
      output: {
        // JS chunk 文件命名规则：assets/js/[name]-[hash].js
        // [name] 是 chunk 名称（如 vendor-pdf），[hash] 是内容哈希
        chunkFileNames: 'assets/js/[name]-[hash].js',
        // 入口文件命名规则：assets/js/[name]-[hash].js
        // 主入口为 main-[hash].js
        entryFileNames: 'assets/js/[name]-[hash].js',
        // 静态资源命名规则：assets/[ext]/[name]-[hash].[ext]
        // [ext] 是资源类型（css、png、svg 等），[name] 是原始名称
        assetFileNames: 'assets/[ext]/[name]-[hash].[ext]',
        /**
         * 手动代码分割配置
         *
         * 将大型依赖拆分为独立的 chunk，优化缓存策略和加载性能：
         *
         * - vendor-react: React 核心库
         *   应用框架核心，最稳定的依赖，长期缓存
         *
         * - vendor-antd: Ant Design 组件库
         *   UI 组件库，体积较大，单独打包便于缓存
         *
         * - vendor-katex: KaTeX 数学公式渲染
         *   用于 Markdown 中的数学公式，按需加载
         *
         * - vendor-markdown: Markdown 渲染相关
         *   react-markdown 及其插件，用于 AI 回复中的 Markdown 渲染
         *
         * - vendor-slate: Slate 富文本编辑器
         *   用于 AI 对话输入框，按需加载
         *
         * - vendor-codemirror: CodeMirror 6 编辑器
         *   用于笔记编辑器，按需加载
         */
        manualChunks: {
          // React 核心库：独立打包，便于长期缓存
          // 从 antd chunk 中拆出，因为 antd 更新频率可能高于 React
          'vendor-react': ['react', 'react-dom', 'react-router-dom'],
          // Ant Design 组件库：UI 基础库，独立缓存
          'vendor-antd': ['antd', '@ant-design/icons'],
          // KaTeX 数学公式：数学公式渲染，按需加载
          'vendor-katex': ['katex'],
          // Markdown 渲染：AI 回复内容渲染，按需加载
          'vendor-markdown': ['react-markdown', 'rehype-katex', 'rehype-highlight', 'rehype-raw', 'remark-math', 'remark-gfm'],
          // Slate 富文本编辑器：AI 对话输入框，按需加载
          'vendor-slate': ['slate', 'slate-history', 'slate-react'],
          // CodeMirror 6：笔记编辑器，按需加载
          'vendor-codemirror': [
            '@codemirror/view',
            '@codemirror/state',
            '@codemirror/lang-markdown',
            '@codemirror/language',
            '@codemirror/commands',
            '@lezer/highlight',
          ],
          // 代码高亮：highlight.js 语法高亮，按需加载
          'vendor-highlight': ['highlight.js'],
          // PDFium PDF 查看器：WASM PDF 渲染引擎，按需加载
          'vendor-pdfium': ['@autonomics/pdfium-viewer'],
        },
      },
    },
    // 设置 chunk 大小警告阈值（默认 500KB）
    // 适当提高阈值以避免不必要的警告
    chunkSizeWarningLimit: 1000,
  },
});
