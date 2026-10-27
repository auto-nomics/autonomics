import { spawn } from 'node:child_process';
import { once } from 'node:events';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const origin='http://127.0.0.1:1428';
const server=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1428','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='';server.stdout.on('data',chunk=>output+=chunk);server.stderr.on('data',chunk=>output+=chunk);
try {
  const readyDeadline=Date.now()+30_000;
  while(!output.includes(origin)){if(server.exitCode!==null||Date.now()>readyDeadline)throw new Error(`Vite readiness failed (exit=${server.exitCode}): ${output}`);await new Promise(resolve=>setTimeout(resolve,50));}
  const run=spawn('xvfb-run',['-a','-s','-screen 0 1440x1000x24','../../target/debug/examples/bibliography_native_smoke'],{stdio:'inherit',env:{...process.env,GDK_BACKEND:'x11',BIB_NATIVE_SMOKE_ORIGIN:origin,BIB_NATIVE_SMOKE_OUTPUT:'/tmp/bib-native-smoke.json'}});
  const [code]=await once(run,'exit');assert.equal(code,0);const result=JSON.parse(readFileSync('/tmp/bib-native-smoke.json','utf8'));assert.equal(result.ok,true);console.log('Native Tauri WebView search/filter/save/evidence/return passed');
} finally {server.kill('SIGTERM');await once(server,'exit');}
