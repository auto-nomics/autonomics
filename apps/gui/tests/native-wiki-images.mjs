import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFileSync, rmSync } from 'node:fs';
import assert from 'node:assert/strict';
const origin = 'http://127.0.0.1:1441';
const report = '/tmp/wiki-native-smoke.json';
rmSync(report, { force: true });
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1441', '--strictPort']);
let output = '';
vite.stdout.on('data', chunk => output += chunk); vite.stderr.on('data', chunk => output += chunk);
try {
  const deadline = Date.now() + 30_000;
  while (!output.includes(origin)) { if (vite.exitCode !== null || Date.now() > deadline) throw Error(output); await new Promise(resolve => setTimeout(resolve, 50)); }
  const run = spawn('xvfb-run', ['-a', '-s', '-screen 0 1200x900x24', process.env.WIKI_NATIVE_SMOKE_BINARY || '../../target/debug/examples/wiki_native_smoke'], { stdio: 'inherit', env: { ...process.env, GDK_BACKEND: 'x11', WIKI_NATIVE_SMOKE_ORIGIN: origin, WIKI_NATIVE_SMOKE_OUTPUT: report } });
  const [code] = await once(run, 'exit');
  const result = JSON.parse(readFileSync(report, 'utf8'));
  assert.equal(code, 0, JSON.stringify(result)); assert.equal(result.ok, true, JSON.stringify(result));
  console.log('Real Tauri WebView wiki images: upload, rendering, persistence, preview and resource cleanup passed');
} finally { vite.kill('SIGTERM'); await once(vite, 'exit'); }
