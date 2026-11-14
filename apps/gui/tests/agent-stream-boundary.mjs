// Live SSE regression: commentary -> tools -> final reply must retain separate text rows.
// Optional AGENT_STREAM_REPLAY uses actual saved model text without making model calls.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync,readFileSync } from 'node:fs';
import http from 'node:http';

const MOCK_PORT = 14398;
const origin = 'http://127.0.0.1:1428';

// ── Mock gateway ──
// Frames written before the browser attaches are buffered and flushed by
// the test at a chosen point, so they can never race the history load.
const snapshot = {
  last_seq: 0,
  agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }],
  contexts: { '/root/alpha': { id: 's1', title: 'Alpha main', created_at: 1, last_active: 2 } },
  display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false },
};
let historyRows=[];
let pushFrame = null;
let flush = null;
const buffered = [];
const mock = http.createServer((req, res) => {
  const path = new URL(req.url, 'http://mock').pathname;
  const json = (body) => { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
  if (path === '/api/session') return json({});
  if (path === '/api/v1/state') return json(snapshot);
  if (path === '/api/v1/agents/a1/sessions/s1/history') return json({ messages: historyRows });
  if (path === '/api/v1/bib/collections/changes') return json({ revision: 0 });
  if (path === '/api/v1/events') {
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
    pushFrame = (frame) => res.write(`id: ${frame.id}\nevent: ${frame.event}\ndata: ${JSON.stringify(frame.data)}\n\n`);
    flush = () => { while (buffered.length) pushFrame(buffered.shift()); };
    req.on('close', () => { pushFrame = null; flush = null; });
    return;
  }
  res.writeHead(404, { 'content-type': 'application/json' });
  res.end('{"error":"not mocked"}');
});

const sendFrame = (frame) => (pushFrame ? pushFrame(frame) : buffered.push(frame));

const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1428', '--strictPort'], {
  stdio: ['ignore', 'pipe', 'pipe'],
  env: { ...process.env, AUTONOMICS_HTTP_API_ADDR: `127.0.0.1:${MOCK_PORT}` },
});
let output = '';
server.stdout.on('data', chunk => { output += chunk; });
server.stderr.on('data', chunk => { output += chunk; });
let browser;
try {
  mock.listen(MOCK_PORT, '127.0.0.1');
  for (let attempt = 0; ; attempt++) {
    if (server.exitCode !== null || attempt > 100) throw new Error(`Vite failed: ${output}`);
    if (output.includes(origin)) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  const executablePath = process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined);
  browser = await chromium.launch({ executablePath, args: ['--no-sandbox'] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => dialog.accept());

  await page.goto(origin);
  await page.getByPlaceholder('输入消息…').waitFor({ state: 'visible' });
  // The history response (instant mock) has settled; flushing later frames
  // cannot be wiped by a late loadHistory overwrite.
  await page.waitForTimeout(300);
  assert.notEqual(flush, null, 'SSE connection must be established');

  await page.context().grantPermissions(['clipboard-read','clipboard-write'],{origin});
  let preface='Sources read. Now saving the plan.',final='The confirmed scientific plan is ready.\n\n| Node | Role |\n| --- | --- |\n| STAT1 | signal |';
  if(process.env.AGENT_STREAM_REPLAY){const history=JSON.parse(readFileSync(process.env.AGENT_STREAM_REPLAY,'utf8'));const assistants=history.filter(m=>m.role==='assistant'&&m.content.some(b=>b.type==='text'));const terminal=assistants.at(-1);final=terminal.content.filter(b=>b.type==='text').map(b=>b.text).join('\n');preface=assistants.filter(m=>m.stop_reason==='tool_use').at(-1).content.filter(b=>b.type==='text').map(b=>b.text).join('\n');}
  let seq=1;const emit=(event,agent='/root/alpha')=>sendFrame({id:seq++,event:'agent',data:{agent,event}});
  emit({LifecycleChanged:'STREAMING'});emit({TextDelta:preface.slice(0,10)});
  emit({ToolCall:{name:'unrelated_agent_tool',input:{}}},'/root/beta');
  emit({TextDelta:preface.slice(10)});emit({UsageUpdate:{input_tokens:100,output_tokens:20}});
  emit({ToolCall:{name:'write_note',input:{version:2}}});emit({ToolResult:{ok:true,content:'{"version":3}'}});
  emit({ToolCall:{name:'read_note',input:{}}});emit({ToolResult:{ok:true,content:'{"status":"ready"}'}});
  emit({TextDelta:final.slice(0,13)});emit({TextDelta:final.slice(13)});emit({LifecycleChanged:'IDLE'});flush();
  const copies=page.getByRole('button',{name:'复制助手回复',exact:true});await copies.nth(1).waitFor();assert.equal(await copies.count(),2,'tool boundary separates commentary from final text');
  await copies.last().click();assert.equal(await page.evaluate(()=>navigator.clipboard.readText()),final);
  await copies.first().click();assert.equal(await page.evaluate(()=>navigator.clipboard.readText()),preface,'other-agent tools do not split this assistant stream');
  historyRows=[{id:'preface',role:'assistant',content:[{type:'text',text:preface},{type:'tool_use',id:'tool',name:'write_note',input:{}}]},{id:'result',role:'user',content:[{type:'tool_result',tool_use_id:'tool',content:'{"version":3}'}]},{id:'final',role:'assistant',content:[{type:'text',text:final}]}];
  await page.reload();await page.getByRole('button',{name:'复制助手回复',exact:true}).nth(1).waitFor();
  await page.getByRole('button',{name:'复制助手回复',exact:true}).last().click();assert.equal(await page.evaluate(()=>navigator.clipboard.readText()),final,'reloaded final text agrees exactly with streamed final text');assert.deepEqual(errors,[]);
  console.log('agent-stream-boundary: distinct copied replies across tools, other-agent isolation and history reload passed');
} finally {
  await browser?.close();
  server.kill();
  mock.close();
}
