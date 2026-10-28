import { spawn } from 'node:child_process';
import { once } from 'node:events';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
const base=resolve('../../target/bib-p4-validation');mkdirSync(base,{recursive:true});
const root=mkdtempSync(join(base,'p5-native-'));writeFileSync(join(root,'owned-marker'),'bib-p5-native-v1');
const server=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','0'],{stdio:['ignore','pipe','pipe']});
let output='';server.stdout.on('data',c=>output+=c);server.stderr.on('data',c=>output+=c);
try {
  const deadline=Date.now()+30000;let origin;
  while(!(origin=output.match(/http:\/\/127\.0\.0\.1:\d+/)?.[0])){if(server.exitCode!==null||Date.now()>deadline)throw new Error(`Vite: ${output}`);await new Promise(r=>setTimeout(r,50));}
  const xdg=Object.fromEntries(['DATA','CONFIG','CACHE'].map(kind=>{const directory=join(root,'xdg-'+kind.toLowerCase());mkdirSync(directory,{recursive:true});return [`XDG_${kind}_HOME`,directory];}));
  const worker=process.env.BIB_P5_NATIVE_WORKER??resolve('../../target/debug/examples/literature_p5_native_smoke');
  console.log('Owned P5 native output:',root);
  const run=spawn('dbus-run-session',['--','xvfb-run','-a','-s','-screen 0 1440x1000x24',process.execPath,'tests/native-p5-worker.mjs'],{stdio:'inherit',env:{...process.env,...xdg,NO_AT_BRIDGE:'1',GDK_BACKEND:'x11',GTK_USE_PORTAL:'0',BIB_P5_NATIVE_ROOT:root,BIB_P5_NATIVE_WORKER:worker,BIB_NATIVE_SMOKE_ORIGIN:origin}});
  const [code]=await once(run,'exit');
  const result=JSON.parse(readFileSync(join(root,'receipt.json'),'utf8'));console.log(JSON.stringify(result,null,2));assert.equal(code,0);assert.equal(result.ok,true);
} finally {server.kill('SIGTERM');if(server.exitCode===null)await once(server,'exit');}
