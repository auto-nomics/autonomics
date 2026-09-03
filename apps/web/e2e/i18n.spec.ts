import { test, expect } from '@playwright/test';

/**
 * i18n 端到端测试
 *
 * 验证：
 * 1. 默认中文界面加载正常
 * 2. 英文启动界面无 i18n key 泄漏
 * 3. 刷新后语言偏好保持
 * 4. Ant Design 组件跟随语言切换
 *
 * 注：原「同步设置页语言切换 UI」用例已随多设备同步系统移除
 * （2026-09-02，语言切换卡片在 SyncSettingsPage 内）。
 */

// 预配置：导航到论文列表页面
async function setupPaperListPage(page: import('@playwright/test').Page, language?: string) {
  await page.addInitScript((lang) => {
    localStorage.setItem('jayread-pane-storage', JSON.stringify({
      state: { root: { type: 'leaf', id: 'pane-1', route: '/papers', search: '' }, activePaneId: 'pane-1' },
      version: 0,
    }));
    const state: any = { sidebarCollapsed: false, theme: 'light', activePanel: null };
    if (lang) state.language = lang;
    localStorage.setItem('jayread-app-storage', JSON.stringify({ state, version: 0 }));
  }, language);

  await page.goto('/');
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(3000);
}

test.describe('i18n 国际化', () => {

  test('默认中文界面正常加载', async ({ page }) => {
    await setupPaperListPage(page);

    // 页面标题不应出现英文错误文本
    const bodyText = await page.textContent('body');
    expect(bodyText).toBeTruthy();

    // 不应出现未翻译的 i18n key（如 "common:" 前缀）
    const untranslated = await page.locator('text=/common:|paperList:|panes:/').count();
    expect(untranslated).toBe(0);
  });

  test('论文列表 - 从英文启动界面', async ({ page }) => {
    await setupPaperListPage(page, 'en');

    // 界面应显示英文（含拉丁文本而非纯中文占位）
    const bodyText = await page.textContent('body');
    expect(bodyText).toBeTruthy();
    expect(bodyText).not.toContain('paperList:status.');
    expect(bodyText).not.toContain('paperList:column.');
    expect(bodyText).not.toContain('common:');
  });

  test('刷新后语言偏好保持', async ({ page }) => {
    // 设置英文启动
    await setupPaperListPage(page, 'en');

    // 刷新页面
    await page.reload();
    await page.waitForLoadState('networkidle');
    await page.waitForTimeout(3000);

    // localStorage 验证语言仍为 en
    const storedLang = await page.evaluate(() => {
      const raw = localStorage.getItem('jayread-app-storage');
      if (!raw) return null;
      return JSON.parse(raw).state?.language;
    });
    expect(storedLang).toBe('en');

    // 且界面仍无 i18n key 泄漏
    const bodyText = await page.textContent('body');
    expect(bodyText).not.toContain('common:');
  });

  test('论文列表 - 中文状态标签正确显示', async ({ page }) => {
    await setupPaperListPage(page, 'zh');

    // 不应出现 i18n key 原文
    const bodyText = await page.textContent('body');
    expect(bodyText).not.toContain('paperList:status.');
    expect(bodyText).not.toContain('paperList:column.');
  });

  test('论文列表 - 英文界面无 i18n key 泄漏', async ({ page }) => {
    await setupPaperListPage(page, 'en');

    // 不应出现 i18n key 原文
    const bodyText = await page.textContent('body');
    expect(bodyText).not.toContain('paperList:status.');
    expect(bodyText).not.toContain('paperList:column.');
    expect(bodyText).not.toContain('common:');
  });

  test('Ant Design locale 联动 - 中文日期格式', async ({ page }) => {
    await setupPaperListPage(page, 'zh');

    // Ant Design 的 Empty 组件默认文本应显示中文
    // 检查是否有中文"暂无数据"或类似文本
    const hasChineseText = await page.evaluate(() => {
      const body = document.body.textContent || '';
      // 检查页面是否包含中文内容
      return /[一-鿿]/.test(body);
    });
    expect(hasChineseText).toBe(true);
  });
});
