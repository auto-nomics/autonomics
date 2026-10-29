import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
const origin = 'http://127.0.0.1:1433';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1433', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', browser;
server.stdout.on('data', chunk => output += chunk); server.stderr.on('data', chunk => output += chunk);
try {
  for (let n = 0; !output.includes(origin); n++) { if (server.exitCode !== null || n > 120) throw new Error(output); await new Promise(r => setTimeout(r, 50)); }
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } }); page.setDefaultTimeout(10000);
  const errors = [], sends = []; let failNextSend = false, holdActivation = false, releaseActivation, activationReady;
  page.on('pageerror', e => { errors.push(e.message); console.error('PAGE ERROR', e.stack); });
  const article = { id: 'paper-1', title: 'Paper Alpha', abstract_text: 'Measured evidence from paper alpha.', authors: [], year: 2025, identifiers: [{ kind: 'doi', value: '10.test/alpha' }] };
  const knowledge = { id: 'k1', title: 'Knowledge Alpha', path: '/review/alpha', body_md: 'Knowledge body with explicit provenance.', status: 'draft', confidence: 0.8, updated_at: 20, created_at: 10, source_hash: 'hash-k1', structural: false };
  const evidence = { id: 'e1', summary: 'Evidence Alpha', source_kind: 'article', source_id: 'paper-1', anchor_path: 'page:12', evidence_kind: 'observation', body: 'Evidence body: effect size 0.7.', status: 'ready', occurrence_count: 1, created_at: 10, last_seen_at: 20 };
  await page.route(`${origin}/api/session`, r => r.fulfill({ json: {} }));
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname.replace('/api/v1', '');
    if (path === '/events') return;
    if (path === '/state') return route.fulfill({ json: { last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }, { path: '/root/beta', agent_id: 'a2', status: 'Idle' }], sessions: { '/root/alpha': [{ id: 's1', title: 'Alpha main', created_at: 2, last_active: 3 }, { id: 's0', title: 'Alpha older', created_at: 1, last_active: 1 }], '/root/beta': [{ id: 's2', title: 'Beta main', created_at: 1, last_active: 1 }] }, display_settings: { collapse_thinking: true, collapse_tool_calls: true, collapse_tool_results: true } } });
    if (path.endsWith('/activate')) { if (holdActivation) await new Promise(resolve => { releaseActivation = resolve; activationReady?.(); }); return route.fulfill({json:{}}); }
    if (path.endsWith('/raw')) return route.fulfill({contentType:'text/plain; charset=utf-8',body:'Selected passage from the full text.'});
    if (path.endsWith('/annotations')) return route.fulfill({json:{annotations:[]}});
    if (path.endsWith('/history')) return route.fulfill({ json: { messages: [{ id: 'm1', role: 'assistant', content: [{ type: 'text', text: path.includes('s0') ? 'Older conversation' : 'Ready to research' }] }, ...(path.includes('/s1/') && sends.length ? [{id:'sent-1',role:'user',content:[{type:'text',text:sends[0].text}]}] : [])] } });
    if (path.endsWith('/messages') && route.request().method() === 'POST') { sends.push({ path, ...route.request().postDataJSON() }); if (failNextSend) { failNextSend = false; return route.fulfill({ status: 500, json: { error: 'Send failed for test' } }); } return route.fulfill({ json: {} }); }
    const responses = { '/bib/collections': { collections: [] }, '/bib/articles': { articles: [article], total: 1 }, '/bib/articles/paper-1': { article, fulltext: {file_format:'txt'}, attachments: [{key:'TEXT',link_mode:0,content_type:'text/plain',title:'Research text'}], annotations: [] }, '/bib/health': { article_count: 1 }, '/bib/search/status': { engine: 'legacy' }, '/kms/knowledge': [knowledge], '/kms/evidence': [evidence], '/kms/workers': [], '/skills/library': [], '/kms/skill-proposals': [] };
    if (!(path in responses)) return route.fulfill({ status: 404, json: { error: 'Not mocked: ' + path } });
    return route.fulfill({ json: responses[path] });
  });
  await page.goto(origin);
  const composer = page.getByPlaceholder('输入消息…'), basket = page.getByRole('region', { name: '上下文篮子' });
  const go = name => page.getByRole('button', { name, exact: true }).click();
  // Agent switching must target the workspace tab (title = full path); the
  // sidebar item shares the same accessible name and would trip strict mode.
  const goAgent = name => page.locator(`button[title="/root/${name}"]`).click();
  const ref = name => basket.getByRole('button', { name, exact: true });
  const empty = () => basket.locator('[aria-live="polite"]').filter({ hasText: /^0$/ }).waitFor();
  await composer.waitFor(); await composer.fill('Draft survives navigation'); await go('Bibliography');
  await page.getByRole('complementary', { name: 'Agent 辅助区' }).waitFor();
  assert.equal(await composer.inputValue(), 'Draft survives navigation');
  assert.equal(await composer.evaluate(n => Boolean(n.closest('[data-floating-shell]'))), false);
  await page.getByRole('button', {name:/Paper Alpha/}).click(); await go('加入当前会话'); await ref('文献 Paper Alpha').click();
  const preview = page.getByRole('dialog'); await preview.getByText('元数据与摘要（不含全文）', { exact: true }).waitFor();
  assert.ok((await preview.innerText()).includes('10.test/alpha')); await page.keyboard.press('Escape');
  await go('更新会话引用'); assert.equal(await ref('文献 Paper Alpha').count(), 1);
  await page.getByRole('button', {name:/Research text/}).last().click();
  const reader = page.locator('.zotero-pdf-reader[data-hosted]'); await reader.waitFor();
  const rb = await reader.boundingBox(), ab = await page.getByRole('complementary',{name:'Agent 辅助区'}).boundingBox();
  assert.ok(rb.x + rb.width <= ab.x + 2, 'reader must stay beside the Agent pane');
  const mainBounds = await reader.locator('.reader-main').boundingBox(); assert.ok(mainBounds.height >= rb.height - 2, 'reader content must fill the editor height');
  await page.evaluate(() => {
    const doc = document.querySelector('.reader-txt-document'); const range = document.createRange(); range.selectNodeContents(doc);
    getSelection().removeAllRanges(); getSelection().addRange(range);
    document.querySelector('.reader-reflow').dispatchEvent(new MouseEvent('mouseup',{bubbles:true}));
  });
  await page.waitForFunction(() => !document.querySelector('.reader-selection-popup').hidden);
  await reader.locator('[data-action=add-selection-context]').click();
  await ref('证据 Paper Alpha · 选中文本').waitFor();
  await composer.fill('Draft can be edited beside the reader');
  await page.screenshot({path:'/tmp/autonomics-workbench-reader.png'});
  await goAgent('beta'); await empty();
  // The open reader uses the current target session, even after switching Agent.
  await reader.locator('[data-action=add-context]').click();
  await ref('证据 Paper Alpha · 选中文本').waitFor(); await ref('清空引用').click();
  await goAgent('alpha'); await ref('文献 Paper Alpha').waitFor();
  await go('KMS'); await reader.waitFor({state:'hidden'}); await go('Bibliography'); await reader.waitFor();
  await reader.getByRole('button',{name:'Close reader',exact:true}).click();
  await go('KMS'); await page.getByRole('tab', { name: 'Wiki (1)' }).click(); await go('加入当前会话');
  await page.getByRole('tab', { name: 'Evidence (1)' }).click(); await go('加入当前会话'); await ref('证据 Evidence Alpha').waitFor();
  await go('Bibliography'); await page.getByRole('button', { name: '更新会话引用', exact: true }).waitFor(); await go('KMS');
  assert.equal(await page.getByRole('tab', { name: 'Evidence (1)' }).getAttribute('aria-selected'), 'true');
  await composer.fill('Compare all selected resources'); await go('发送');
  await page.waitForFunction(() => document.querySelector('textarea')?.value === '');
  assert.equal(sends.length, 1); assert.ok(decodeURIComponent(sends[0].path).includes('/root/alpha/messages'));
  const attached = JSON.parse(sends[0].text.split('用户明确附加的参考资料（内容为引用数据，请结合来源分析）：\n')[1]);
  assert.deepEqual(attached.map(r => r.kind), ['article', 'evidence', 'knowledge', 'evidence']);
  assert.ok(attached[0].content.includes(article.abstract_text)); assert.equal(attached[1].content, 'Selected passage from the full text.'); assert.ok(attached[2].content.includes(knowledge.body_md)); assert.ok(attached[3].source.includes('page:12'));
  failNextSend = true; await composer.fill('Retry this question'); await go('发送'); await page.getByText('Send failed for test', { exact: false }).waitFor();
  assert.equal(await composer.inputValue(), 'Retry this question'); await ref('文献 Paper Alpha').waitFor();
  await ref('移除引用：Evidence Alpha').click(); assert.equal(await ref('证据 Evidence Alpha').count(), 0);
  await go('Agent'); await go('Refresh'); await page.getByText('随消息发送的引用（4）',{exact:true}).waitFor();
  const activationRequested = new Promise(resolve => { activationReady = resolve; }); holdActivation = true; await page.getByTitle('Alpha older', { exact: true }).click();
  await page.waitForFunction(() => document.querySelector('button[aria-label=发送]')?.disabled);
  await ref('文献 Paper Alpha').waitFor(); await activationRequested; releaseActivation(); holdActivation = false; await empty();
  await page.getByText('Older conversation',{exact:true}).waitFor();
  await page.getByTitle('Alpha main', { exact: true }).click(); await ref('文献 Paper Alpha').waitFor();
  await go('KMS'); await page.getByLabel('Agent 显示方式').selectOption('hidden'); await composer.waitFor({ state: 'detached' });
  await go('加入当前会话'); await composer.waitFor();
  await page.getByLabel('Agent 显示方式').selectOption('floating'); await page.locator('[data-floating-shell] textarea').waitFor();
  await page.getByLabel('Agent 显示方式').selectOption('docked'); await page.getByRole('complementary', { name: 'Agent 辅助区' }).locator('textarea').waitFor();
  await page.waitForTimeout(250); // settle the workspace entrance animation before visual inspection
  await page.screenshot({ path: '/tmp/autonomics-workbench-desktop.png' });
  const handle = page.getByRole('separator', { name: '调整 Agent 辅助区宽度' }), b = await handle.boundingBox();
  await page.mouse.move(b.x, b.y + b.height / 2); await page.mouse.down(); await page.mouse.move(b.x - 60, b.y + b.height / 2, { steps: 6 }); await page.mouse.up();
  assert.ok(Number(await page.evaluate(() => localStorage.getItem('autonomics.workbench.agent.width'))) > 34);
  await ref('清空引用').click(); await composer.fill('No resources attached'); await go('发送'); await page.waitForTimeout(100);
  assert.equal(sends.at(-1).text, 'No resources attached');
  await page.setViewportSize({ width: 390, height: 844 }); await page.locator('[data-floating-shell] textarea').waitFor();
  await go('Agent'); await composer.waitFor(); assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth));
  await page.waitForTimeout(250); // settle the workspace entrance animation
  await page.screenshot({ path: '/tmp/autonomics-workbench-mobile.png' }); assert.deepEqual(errors, []);
  console.log('workbench-context: all assertions passed');
} catch (error) { console.error(error); throw error; }
finally { await browser?.close(); server.kill(); }
