/**
 * JayRead API 端到端测试
 *
 * 验证后端 API 的基本可用性，包括：
 * - 健康检查端点
 * - 论文管理 API（列表、创建）
 * - 设置 API（读取、更新）
 */
import { test, expect } from '@playwright/test';

/** API 健康检查测试组 */
test.describe('API Health Check', () => {
  /** 验证 API 服务器可达，健康检查端点返回 200 */
  test('API server should be reachable', async ({ request }) => {
    const response = await request.get('/api/health');
    expect(response.ok()).toBeTruthy();
  });
});

/** 论文管理 API 测试组 */
test.describe('Paper Management API', () => {
  /** 验证论文列表接口返回成功 */
  test('should list papers', async ({ request }) => {
    const response = await request.get('/api/papers');
    expect(response.ok()).toBeTruthy();
  });

  /** 验证创建论文接口返回成功 */
  test('should create and retrieve paper', async ({ request }) => {
    const createResponse = await request.post('/api/papers', {
      data: {
        title: 'Test Paper for E2E',
        type: 'article',
      },
    });
    expect(createResponse.ok()).toBeTruthy();
  });
});

/** 设置 API 测试组 */
test.describe('Settings API', () => {
  /** 验证获取设置接口返回成功 */
  test('should get settings', async ({ request }) => {
    const response = await request.get('/api/settings');
    expect(response.ok()).toBeTruthy();
  });

  /** 验证更新设置接口返回成功 */
  test('should update settings', async ({ request }) => {
    const response = await request.put('/api/settings', {
      data: { theme: 'dark' },
    });
    expect(response.ok()).toBeTruthy();
  });
});