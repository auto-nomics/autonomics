#!/usr/bin/env node
// Optional unattended renderer. Reuses the production GUI and its authenticated queue.
import { chromium } from 'playwright';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
if (process.argv.includes('--help')) {
  console.log('Set AUTONOMICS_RENDER_ORIGIN and AUTONOMICS_RENDER_TOKEN_FILE, then run npm run canvas:render-worker. Optional: AUTONOMICS_RENDER_CHROMIUM. Stop with SIGINT/SIGTERM.');
  process.exit(0);
}
const stop = new AbortController();
let browser;
const shutdown = () => { stop.abort(); void browser?.close().catch(() => {}); };
process.once('SIGINT', shutdown); process.once('SIGTERM', shutdown);
try {
  if (!process.env.AUTONOMICS_RENDER_ORIGIN || !process.env.AUTONOMICS_RENDER_TOKEN_FILE) throw new Error('AUTONOMICS_RENDER_ORIGIN and AUTONOMICS_RENDER_TOKEN_FILE are required.');
  const url = new URL(process.env.AUTONOMICS_RENDER_ORIGIN);
  if (!['http:','https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/') throw new Error('Render origin must be an HTTP(S) origin without credentials, query or path.');
  browser = await chromium.launch({ headless:true, executablePath:process.env.AUTONOMICS_RENDER_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args:process.getuid?.()===0 ? ['--no-sandbox'] : [] });
  if (stop.signal.aborted) throw new Error("stopped");
  const context = await browser.newContext({viewport:{width:1280,height:900}});
  // Embedded images and local fonts work; never navigate/fetch to another origin.
  await context.route('**/*',route=>{
    const requested = new URL(route.request().url());
    return ['http:','https:'].includes(requested.protocol) && requested.origin !== url.origin ? route.abort('blockedbyclient') : route.continue();
  });
  const authenticate = async () => {
    let token;
    try { token = (await readFile(process.env.AUTONOMICS_RENDER_TOKEN_FILE,'utf8')).trim(); } catch { throw new Error('Unable to read renderer token file.'); }
    if (!token || token.length>32768) throw new Error('Renderer token file is empty or invalid.');
    const login = await context.request.post(`${url.origin}/api/session`,{data:{token},timeout:30000,maxRedirects:0});
    token = undefined;
    const status = login.status(); await login.dispose();
    if (status !== 200) throw new Error(`Renderer login failed (HTTP ${status}).`);
  };
  await authenticate();
  const queue = await context.request.get(`${url.origin}/api/v1/visualization/proposal-preview-requests`,{timeout:30000,maxRedirects:0});
  const queueStatus = queue.status(); await queue.dispose();
  if (queueStatus !== 200) throw new Error(`Renderer queue unavailable (HTTP ${queueStatus}).`);
  const page = await context.newPage();
  let pageFailed = false;
  page.on('pageerror',()=>{pageFailed=true;});
  await page.goto(url.origin,{waitUntil:'domcontentloaded',timeout:60000});
  await page.getByRole('navigation',{name:'工作台活动栏'}).waitFor({timeout:60000});
  console.log(JSON.stringify({event:'renderer_ready',origin:url.origin}));
  while (!stop.signal.aborted) {
    if (!browser.isConnected() || page.isClosed() || pageFailed) throw new Error('Renderer browser stopped or encountered a page error.');
    const session = await context.request.get(`${url.origin}/api/session`,{timeout:10000,maxRedirects:0});
    const status = session.status(); await session.dispose();
    if (status === 401) {
      await authenticate();
      pageFailed = false;
      await page.reload({waitUntil:'domcontentloaded',timeout:60000});
      await page.getByRole('navigation',{name:'工作台活动栏'}).waitFor({timeout:60000});
      console.log(JSON.stringify({event:'renderer_reauthenticated'}));
    } else if (status !== 200) throw new Error(`Renderer session check failed (HTTP ${status}).`);
    await delay(5000,undefined,{signal:stop.signal}).catch(error=>{if(error.name!=='AbortError')throw error;});
  }
} catch (error) {
  if (!stop.signal.aborted) {
    // No response bodies, page contents, cookies or credential values in diagnostics.
    console.error(error instanceof Error && /^(AUTONOMICS_|Render origin|Unable to read|Renderer token|Renderer login|Renderer queue|Renderer browser|Renderer session)/.test(error.message) ? error.message : 'Renderer startup or connection failed.');
    process.exitCode=1;
  }
} finally {
  await browser?.close().catch(()=>{});
  process.removeListener('SIGINT',shutdown);process.removeListener('SIGTERM',shutdown);
}
