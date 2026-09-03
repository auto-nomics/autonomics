/**
 * JayRead Playwright 端到端测试配置
 *
 * 配置 E2E 测试的运行环境，包括：
 * - 测试目录：./e2e
 * - 并行执行策略
 * - CI 环境下的重试和单 worker 策略
 * - 开发服务器自动启动
 * - 仅使用 Chromium 浏览器（桌面端）
 */
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  /** 测试文件目录 */
  testDir: './e2e',
  /** 启用完全并行执行（各测试文件独立运行） */
  fullyParallel: true,
  /** CI 环境下禁止 test.only（防止跳过其他测试） */
  forbidOnly: !!process.env.CI,
  /** CI 环境下失败重试 2 次，本地不重试 */
  retries: process.env.CI ? 2 : 0,
  /** CI 环境下使用单 worker 避免资源竞争，本地使用默认值 */
  workers: process.env.CI ? 1 : undefined,
  /** 使用 HTML 报告器生成可视化测试报告 */
  reporter: 'html',
  /** 全局测试配置 */
  use: {
    /** 开发服务器地址 */
    baseURL: 'http://localhost:5173',
    /** 首次重试时录制 trace（用于调试失败用例） */
    trace: 'on-first-retry',
  },
  /** 浏览器项目配置：仅使用桌面端 Chromium */
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  /** 开发服务器配置：自动启动 Vite 开发服务器 */
  webServer: {
    /** 启动命令 */
    command: 'npm run dev',
    /** 服务器地址 */
    url: 'http://localhost:5173',
    /** 非 CI 环境复用已运行的服务器（避免重复启动） */
    reuseExistingServer: !process.env.CI,
    /** 服务器启动超时时间：120 秒 */
    timeout: 120 * 1000,
  },
});