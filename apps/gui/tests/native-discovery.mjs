import { spawn } from 'node:child_process';
import { once } from 'node:events';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const origin = 'http://127.0.0.1:1429';
const vite = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1429', '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = ''; vite.stdout.on('data', chunk => output += chunk); vite.stderr.on('data', chunk => output += chunk);
try {
  const deadline = Date.now() + 30000;
  while (!output.includes(origin)) { if (vite.exitCode !== null || Date.now() > deadline) throw new Error(output); await new Promise(resolve => setTimeout(resolve, 50)); }
  const run = spawn('xvfb-run', ['-a', '-s', '-screen 0 1440x1000x24', '../../target/debug/examples/discovery_native_smoke'], { stdio: 'inherit', env: { ...process.env, GDK_BACKEND: 'x11', BIB_DISCOVERY_NATIVE_ORIGIN: origin, BIB_DISCOVERY_NATIVE_OUTPUT: '/tmp/bib-discovery-native.json' } });
  const [code] = await once(run, 'exit'); assert.equal(code, 0);
  assert.equal(JSON.parse(readFileSync('/tmp/bib-discovery-native.json', 'utf8')).ok, true);
  console.log('Actual Tauri discovery/session/import/library/provenance workflow passed');
} finally { vite.kill('SIGTERM'); await once(vite, 'exit'); }
