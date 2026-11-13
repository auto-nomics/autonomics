// Standalone headless worker + compiled GUI + real authenticated Gateway; no model turns.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { Readable } from 'node:stream';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp,readFile,writeFile,rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const binary=process.env.VISUALIZATION_GATEWAY_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_workflow',import.meta.url));
assert.ok(existsSync(binary),'build the visualization_workflow gateway example first');
const directory=await mkdtemp(join(tmpdir(),'autonomics-render-worker-'));
const host=spawn(binary,[directory],{stdio:['ignore','pipe','pipe']});
let hostOutput='',workerOutput='',worker,proxy,sessionCookie='';
host.stdout.on('data',b=>hostOutput+=b);host.stderr.on('data',b=>hostOutput+=b);
const until=async(predicate,reason)=>{const deadline=Date.now()+90000;while(!await predicate()){if(host.exitCode!==null||host.signalCode!==null)throw new Error('Gateway fixture stopped');if(worker&&(worker.exitCode!==null||worker.signalCode!==null))throw new Error(`Worker stopped: ${workerOutput}`);if(Date.now()>deadline)throw new Error(reason);await new Promise(r=>setTimeout(r,100));}};
try {
  await until(()=>existsSync(join(directory,'ready.json')),'Gateway readiness');
  const {origin,token}=JSON.parse(await readFile(join(directory,'ready.json'),'utf8'));
  const tokenPath=join(directory,'renderer.token');await writeFile(tokenPath,token,{mode:0o600});
  const call=async(method,path,body)=>{const response=await fetch(`${origin}/api/v1/visualization${path}`,{method,headers:{authorization:`Bearer ${token}`,...(body===undefined?{}:{'content-type':'application/json'})},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(30000)});assert.ok(response.ok,`HTTP ${response.status}`);return response.json();};
  assert.equal((await fetch(`${origin}/api/v1/visualization/proposal-preview-requests`)).status,401);
  // Only this disposable test session is observed by the loopback proxy.
  proxy=createServer(async(req,res)=>{
    const abort=new AbortController();res.on('close',()=>abort.abort());
    try {
      const headers={...req.headers};delete headers.host;delete headers.connection;delete headers['transfer-encoding'];
      const upstream=await fetch(new URL(req.url,origin),{method:req.method,headers,...(['GET','HEAD'].includes(req.method)?{}:{body:req,duplex:'half'}),redirect:'manual',signal:abort.signal});
      const responseHeaders=Object.fromEntries(upstream.headers);
      for(const key of ['content-length','content-encoding','transfer-encoding','connection','set-cookie'])delete responseHeaders[key];
      const cookies=upstream.headers.getSetCookie();
      if(cookies.length)responseHeaders['set-cookie']=cookies;
      if(req.method==='POST'&&req.url==='/api/session')sessionCookie=cookies.find(value=>value.startsWith('autonomics_session='))?.split(';')[0]||sessionCookie;
      res.writeHead(upstream.status,responseHeaders);
      if(upstream.body)Readable.fromWeb(upstream.body).on('error',()=>res.destroy()).pipe(res);else res.end();
    }catch{if(!res.headersSent)res.writeHead(502);res.end();}
  });
  await new Promise(resolve=>proxy.listen(0,'127.0.0.1',resolve));
  const workerOrigin=`http://127.0.0.1:${proxy.address().port}`;
  worker=spawn(process.execPath,['scripts/canvas-render-worker.mjs'],{env:{...process.env,AUTONOMICS_RENDER_ORIGIN:workerOrigin,AUTONOMICS_RENDER_TOKEN_FILE:tokenPath},stdio:['ignore','pipe','pipe']});
  worker.stdout.on('data',b=>workerOutput+=b);worker.stderr.on('data',b=>workerOutput+=b);
  await until(()=>workerOutput.includes('renderer_ready'),'Headless renderer readiness');
  await call('POST','/projects',{id:'worker-study',title:'Worker study'});
  const figure=await call('POST','/projects/worker-study/figures',{title:'Headless candidate',format:'excalidraw/v2',document:{type:'excalidraw',version:2,elements:[],files:{}}});
  const path=`/projects/worker-study/figures/${figure.id}`;
  const candidate=await call('POST',`${path}/proposals`,{expected_revision:1,summary:'Render a native scientific label',operations:[{op:'add',element:{id:'label',type:'text',x:0,y:0,width:220,height:25,text:'Headless preview',originalText:'Headless preview',fontSize:20,fontFamily:9,lineHeight:1.25,textAlign:'left',verticalAlign:'top',containerId:null,autoResize:false,angle:0,strokeColor:'#1b2733',backgroundColor:'transparent',opacity:100,roughness:0,groupIds:[],boundElements:[],isDeleted:false}}]});
  await call('POST',`${path}/proposals/${candidate.id}/preview-request`,{expected_revision:1});
  let preview;
  await until(async()=>{preview=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=1`);if(preview.render_status==='failed')throw new Error('Headless candidate rendering failed');return preview.render_status==='ready';},'Headless PNG completion');
  assert.equal(preview.scope,'proposal_candidate');assert.equal(preview.proposal_id,candidate.id);assert.match(preview.renderer,/font9\/[a-f0-9]{64}$/);assert.equal(preview.document_sha256,candidate.document_sha256);
  const png=Buffer.from(preview.dataURL.split(',')[1],'base64');assert.ok(png.readUInt32BE(16)>100);assert.ok(png.readUInt32BE(20)>20);
  await writeFile('/tmp/autonomics-headless-candidate.png',png);
  assert.equal((await call('GET',path)).metadata.revision,1);
  assert.equal((await call('GET',`${path}/proposals/${candidate.id}`)).status,'pending');
  assert.ok(sessionCookie,'worker received a production session');
  const previousSession=sessionCookie;
  const revoked=await fetch(`${origin}/api/session`,{method:'DELETE',headers:{cookie:previousSession}});assert.equal(revoked.status,200);
  assert.equal((await fetch(`${origin}/api/session`,{headers:{cookie:previousSession}})).status,401);
  const renewed=await call('POST',`${path}/proposals`,{expected_revision:1,summary:'Queued during expired session',operations:[{op:'add',element:{...candidate.document.elements[0],id:'renewed-label',text:'Session restored',originalText:'Session restored'}}]});
  await call('POST',`${path}/proposals/${renewed.id}/preview-request`,{expected_revision:1});
  await until(()=>workerOutput.includes('renderer_reauthenticated'),'Worker automatically renews expired session');
  assert.ok(sessionCookie!==previousSession,'Gateway issued a fresh session');
  await until(async()=>{const result=await call('GET',`${path}/proposals/${renewed.id}/preview?expected_revision=1`);if(result.render_status==='failed')throw new Error('Post-renewal rendering failed');return result.render_status==='ready';},'Queued candidate rendered after renewal');
  const invalid=await call('POST',`${path}/proposals`,{expected_revision:1,summary:'Missing asset',operations:[{op:'add',element:{id:'missing',type:'image',fileId:'absent',x:0,y:0,width:100,height:100,status:'saved',scale:[1,1]}}]});
  await call('POST',`${path}/proposals/${invalid.id}/preview-request`,{expected_revision:1});
  await until(async()=>{const result=await call('GET',`${path}/proposals/${invalid.id}/preview?expected_revision=1`);if(result.render_status!=='failed')return false;assert.ok(!result.dataURL);return true;},'Missing asset rejected');
  assert.equal((await call('GET',path)).metadata.revision,1);
  const trace=JSON.parse(await readFile(join(directory,'trace.json'),'utf8'));
  assert.equal(trace.step,0,'render worker never sends a model message');
  assert.deepEqual(trace.actions,[]);
  assert.ok(!workerOutput.includes(token),'worker diagnostics never print credentials');
  worker.kill('SIGTERM');await new Promise(resolve=>worker.once('exit',resolve));assert.equal(worker.exitCode,0);
  worker=undefined;
  console.log('canvas-render-worker: production login/GUI, headless candidate PNG, pinned font/hash, unchanged figure/pending proposal, automatic session renewal/queued work, missing asset failure and graceful shutdown passed');
} finally {
  worker?.kill('SIGTERM');
  if(worker&&worker.exitCode===null&&worker.signalCode===null)await new Promise(resolve=>worker.once('exit',resolve));
  if(proxy){proxy.closeAllConnections();await new Promise(resolve=>proxy.close(resolve));}
  if(host.exitCode===null&&host.signalCode===null){const exited=new Promise(resolve=>host.once('exit',resolve));await writeFile(join(directory,'stop'),'stop');await exited;}
  await rm(directory,{recursive:true,force:true});
}
