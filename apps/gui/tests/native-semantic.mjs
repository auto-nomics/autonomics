import { spawn } from 'node:child_process';
import { once } from 'node:events';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const origin='http://127.0.0.1:1433';
const outputPath=process.env.BIB_NATIVE_SMOKE_OUTPUT||'/tmp/bib-native-semantic-smoke.json';
const vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1433','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='';vite.stdout.on('data',data=>output+=data);vite.stderr.on('data',data=>output+=data);
try {
  const deadline=Date.now()+30000;
  while(!output.includes(origin)){if(vite.exitCode!==null||Date.now()>deadline)throw new Error(output);await new Promise(resolve=>setTimeout(resolve,50));}
  const run=spawn('xvfb-run',['-a','-s','-screen 0 1440x1000x24','../../target/debug/examples/bibliography_native_smoke'],{stdio:'inherit',env:{...process.env,GDK_BACKEND:'x11',BIB_NATIVE_SEMANTIC_SMOKE:'1',BIB_NATIVE_SMOKE_ORIGIN:origin,BIB_NATIVE_SMOKE_OUTPUT:outputPath}});
  const [code]=await once(run,'exit');assert.equal(code,0);
  const receipt=JSON.parse(readFileSync(outputPath,'utf8'));assert.equal(receipt.ok,true);
  console.log('Actual Tauri WebKitGTK: installed Qwen model/settings/hybrid/evidence/reader/saved rule/reload/disable passed');
} finally {vite.kill('SIGTERM');await once(vite,'exit');}
