// Real Excalidraw/workbench; a deterministic HTTP fixture represents external agent writes.
import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { clickCanvasAction } from './helpers/canvas-action.mjs';
const origin='http://127.0.0.1:1458';
const server=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1458','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',browser;server.stdout.on('data',c=>output+=c);server.stderr.on('data',c=>output+=c);
const meta={id:'figure',project_id:'study',title:'Direct canvas',revision:1,updated_at:new Date().toISOString()};
let document={type:'excalidraw',version:2,elements:[],files:{},appState:{viewBackgroundColor:'#ffffff'}};
const previews=new Map(),requests=[];let localWrites=0;let injectConflict=false;
try {
  for(let n=0;!output.includes(origin);n++){if(server.exitCode!==null||n>1200)throw Error(output);await new Promise(r=>setTimeout(r,50));}
  browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||(existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined),args:['--no-sandbox']});
  const page=await browser.newPage({viewport:{width:1440,height:1000}});page.setDefaultTimeout(30000);
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.route(`${origin}/api/session`,r=>r.fulfill({json:{}}));
  await page.route('**/api/v1/**',async route=>{
    const path=new URL(route.request().url()).pathname.replace('/api/v1','');const method=route.request().method();requests.push(path);
    const reply=json=>route.fulfill({json});
    if(path==='/state')return reply({last_seq:0,agents:[{path:'/root/alpha',agent_id:'a1',status:'Idle'}],contexts:{},display_settings:{}});
    if(path==='/visualization/projects')return reply([{id:'study',title:'Study'}]);
    if(path==='/visualization/projects/study/figures')return reply(method==='GET'?[meta]:meta);
    if(path==='/visualization/projects/study/figures/figure/head'){
      if(injectConflict){injectConflict=false;document.elements=[{...document.elements[0],id:'concurrent-agent',isDeleted:false,x:300,y:300}];meta.revision++;}
      return reply(meta);
    }
    if(path==='/visualization/projects/study/figures/figure'){
      if(method==='PUT'){
        const input=route.request().postDataJSON();
        if(input.expected_revision!==meta.revision)return route.fulfill({status:409,json:{error:'revision conflict'}});
        document=input.document;meta.title=input.title;meta.revision++;localWrites++;return reply(meta);
      }
      return reply({metadata:meta,scene:{document},annotations:[]});
    }
    const preview=path.match(/\/revisions\/(\d+)\/preview$/);
    if(preview){const revision=Number(preview[1]);if(method==='PUT'){assert.equal(revision,meta.revision);previews.set(revision,route.request().postDataJSON().dataURL);return reply({revision});}return reply({revision,dataURL:previews.get(revision)});}
    if(path.endsWith('/annotations'))return reply([]);
    if(path.endsWith('/markups'))return reply({items:[],next_before:null});
    if(path==='/agents'&&method==='POST'){const input=route.request().postDataJSON();return reply({path:`${input.parent_path}/${input.name}`});}
    if(path.endsWith('/context'))return reply({agent_id:'a1',context:null});
    if(path.endsWith('/history'))return reply({messages:[]});
    if(path==='/profiles')return reply(['researcher','illustrator']);
    if(path==='/model-config')return reply({models:[],providers:[]});
    if(path==='/gateway/status')return reply({running:true});
    if(path==='/settings')return reply({});
    if(path==='/storage/agents')return reply([]);
    if(path.startsWith('/bib/'))return reply({collections:[],articles:[],items:[],total:0});
    return route.fulfill({status:404,json:{error:path}});
  });
  await page.route('**/api/v1/events',()=>new Promise(()=>{}));
  await page.goto(origin,{waitUntil:'domcontentloaded'});
  await page.getByRole('navigation',{name:'工作台活动栏'}).getByRole('button',{name:'画布',exact:true}).click();
  await page.getByRole('button',{name:'新建画布',exact:true}).click();
  await page.locator('[data-canvas-surface] .excalidraw').waitFor();
  const state=()=>page.evaluate(async()=>{const s=await import('/src/app/workbench/workbench-store.ts');return s.allGroups(s.getWorkbenchState().groups).flatMap(([,g])=>g.editors).find(e=>e.kind==='canvas')?.state;});
  const wait=async predicate=>{const deadline=Date.now()+20000;while(!await predicate()){if(Date.now()>deadline)throw Error(`Timed out; canvas state ${JSON.stringify(await state())}; errors ${errors}`);await page.waitForTimeout(100);}};
  await wait(async()=>(await state())?.selectionReady===true);
  document.elements=[{id:'agent-node',type:'rectangle',x:100,y:100,width:160,height:100,angle:0,strokeColor:'#1971c2',backgroundColor:'#a5d8ff',fillStyle:'solid',strokeWidth:2,strokeStyle:'solid',roughness:0,opacity:100,groupIds:[],frameId:null,index:'a0',roundness:null,seed:1,version:1,versionNonce:1,isDeleted:false,boundElements:[],updated:1,link:null,locked:false}];
  const svg='data:image/svg+xml;base64,'+Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="80" height="60"><rect width="80" height="60" fill="#0088cc"/></svg>').toString('base64');
  document.files={'agent-file':{id:'agent-file',dataURL:svg,mimeType:'image/svg+xml',created:1}};
  document.elements.push({...document.elements[0],id:'agent-image',type:'image',x:300,width:80,height:60,index:'a1',fileId:'agent-file',status:'saved',scale:[1,1],crop:null});
  meta.revision=2;
  await wait(async()=>(await state())?.revision===2&&(await state())?.objectCount===2);
  await wait(()=>previews.has(2));
  assert.equal(localWrites,0,'applying agent changes must not echo-save a duplicate revision');
  assert.ok(!requests.some(p=>p.includes('proposal')),'no proposal endpoints');
  assert.equal(await page.getByText('查看 Agent 提案',{exact:true}).count(),0);
  const downloadPromise=page.waitForEvent('download');await clickCanvasAction(page,'导出 源文件');
  const exported=JSON.parse(readFileSync(await (await downloadPromise).path(),'utf8'));
  assert.equal(exported.elements[0].id,'agent-node');
  assert.equal(exported.elements[1].id,'agent-image');
  assert.equal(exported.files['agent-file'].dataURL,svg);
  await page.keyboard.press('Escape');
  await page.locator('[data-canvas-surface] .excalidraw').focus();
  await page.keyboard.press('Control+z');
  await wait(()=>localWrites>0);
  assert.equal(document.elements.filter(e=>!e.isDeleted).length,0,'native undo of remote change auto-saves');
  assert.equal(meta.revision,3);
  const surface=await page.locator('[data-canvas-surface]').boundingBox();
  await page.keyboard.press('r');
  await page.mouse.move(surface.x+250,surface.y+220);await page.mouse.down();
  await page.mouse.move(surface.x+350,surface.y+280,{steps:4});
  injectConflict=true;await page.mouse.up();
  await page.getByRole('alert').filter({hasText:'本地有未保存'}).waitFor();
  assert.equal(meta.revision,4);assert.equal(localWrites,1,'local dirty scene is not overwritten or saved over remote changes');
  assert.equal((await state()).dirty,true);
  assert.equal((await state()).revision,3,'local revision remains intact until conflict resolution');

  assert.deepEqual(errors,[]);
  console.log('canvas-direct: external revision sync, current PNG, no proposal calls, source export, native undo, autosave and dirty-scene conflict protection passed');
} finally {await browser?.close();server.kill('SIGTERM');}
