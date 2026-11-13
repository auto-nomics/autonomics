// Native panel compilation, caption clearance, local routing and export; no model invocation.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-panel-route-')),origin='http://127.0.0.1:1458';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1458','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',viteOutput='',browser,page;let sent=0;const diagnostics={requests:[],console:[]};native.stdout.on('data',b=>output+=b);native.stderr.on('data',b=>output+=b);vite.stdout.on('data',b=>viteOutput+=b);vite.stderr.on('data',b=>viteOutput+=b);
const persist=(name,value)=>writeFile(join(directory,name),JSON.stringify(value,null,2),{mode:0o600});
const until=async(fn,label)=>{const deadline=Date.now()+60000;while(!await fn()){assert.ok(Date.now()<deadline,label);await new Promise(r=>setTimeout(r,50));}};
try {
 await until(()=>{assert.equal(native.exitCode,null,output);return output.includes('VISUALIZATION_NATIVE_ADDR=');},'native readiness');await until(()=>{assert.equal(vite.exitCode,null,viteOutput);return viteOutput.includes(origin);},'Vite readiness');
 const backend=`http://${output.match(/VISUALIZATION_NATIVE_ADDR=([^\s]+)/)[1]}`;
 const call=async(method,path,body)=>{const r=await fetch(backend+path,{method,headers:{'content-type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(30000)});const v=await r.json();assert.ok(r.ok,JSON.stringify(v));return v;};
 browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||(existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined),args:['--no-sandbox']});page=await browser.newPage({viewport:{width:1600,height:1000}});const errors=[];page.on('pageerror',e=>errors.push(e.message));page.on('requestfailed',r=>{if(diagnostics.requests.length<100)diagnostics.requests.push({url:r.url(),error:r.failure()?.errorText});});page.on('console',m=>{if(m.type()==='error'&&diagnostics.console.length<100)diagnostics.console.push(m.text().slice(0,1000));});
 await page.route('**/api/session',r=>r.fulfill({json:{}}));
 await page.route('**/api/v1/**',async route=>{const url=new URL(route.request().url()),p=url.pathname.replace('/api/v1','');if(p.startsWith('/visualization/'))return route.fulfill({response:await route.fetch({url:backend+url.pathname+url.search})});if(p==='/events')return new Promise(()=>{});if(p.endsWith('/messages')&&route.request().method()==='POST'){sent++;return route.fulfill({json:{}});}if(p.endsWith('/history'))return route.fulfill({json:{messages:[]}});if(p==='/state')return route.fulfill({json:{last_seq:0,agents:[{path:'/root/alpha',agent_id:'a1',status:'Idle'}],contexts:{'/root/alpha':{id:'s1',title:'Candidate focus',created_at:1,last_active:1}},display_settings:{collapse_thinking:false,collapse_tool_calls:false,collapse_tool_results:false}}});if(p==='/profiles')return route.fulfill({json:['researcher']});if(p==='/model-config')return route.fulfill({json:{models:[],providers:[]}});if(p==='/settings')return route.fulfill({json:{}});if(p==='/storage/agents')return route.fulfill({json:[]});if(p==='/gateway/status')return route.fulfill({json:{running:true}});return route.fulfill({status:404,json:{error:`unrelated fixture ${p}`}});});
 await page.goto(origin,{waitUntil:'domcontentloaded',timeout:60000});await page.getByRole('navigation',{name:'工作台活动栏'}).getByRole('button',{name:'画布',exact:true}).click();await page.getByRole('textbox',{name:'新项目名称'}).fill('Illustration fit');await page.getByRole('button',{name:'创建项目',exact:true}).click();await page.getByRole('button',{name:'新建画布',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const project=await page.getByRole('combobox',{name:'可视化项目'}).inputValue(),figure=(await call('GET',`/api/v1/visualization/projects/${project}/figures`))[0].id,path=`/api/v1/visualization/projects/${project}/figures/${figure}`;
 const initial=await call('GET',path);
 const plan=JSON.parse(await readFile(new URL('../../../crates/visualization/tests/fixtures/ifng-panel-plan.json',import.meta.url),'utf8'));
 const sketch=await call('POST','/test/visualization',{action:'sketch',project_id:project,figure_id:figure,expected_revision:initial.metadata.revision,title:'Native panel route',document:plan});
 const compiled=await call('GET',`${path}/proposals/${sketch.content.id}`);
 const find=(document,id)=>document.elements.find(e=>e.id===id);
 for(const [index,panel]of plan.panels.entries()){
  assert.equal(find(compiled.document,`panel-${index}`).name,'');
  assert.equal(find(compiled.document,`panel-title-${index}`).originalText,panel.title);
 }
 const preview=async(candidate,revision,name)=>{
  await call('POST',`${path}/proposals/${candidate.id}/preview-request`,{expected_revision:revision});
  let rendered;await until(async()=>{rendered=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=${revision}`);assert.notEqual(rendered.render_status,'failed');return rendered.render_status==='ready';},'native candidate PNG');
  assert.equal(rendered.document_sha256,candidate.document_sha256);
  await writeFile(join(directory,name),Buffer.from(rendered.dataURL.split(',')[1],'base64'));
 };
 for(const element of compiled.document.elements.filter(e=>e.type==='text'&&e.containerId?.startsWith('node-'))){
  const node=find(compiled.document,element.containerId);
  assert.ok(element.x>=node.x&&element.y>=node.y);
  assert.ok(element.x+element.width<=node.x+node.width+1e-6);
  assert.ok(element.y+element.height<=node.y+node.height+1e-6);
 }
 assert.ok(find(compiled.document,'label-0').text.includes('\n'),'long scientific grid label wraps before rendering');
 const firstFrame=find(compiled.document,'panel-0'),secondFrame=find(compiled.document,'panel-1'),lastFrame=find(compiled.document,'panel-2');
 assert.ok(secondFrame.x-firstFrame.x-firstFrame.width>=find(compiled.document,'edge-label-1').width+40-1e-6,'horizontal panel gap reserves caption width');
 assert.ok(lastFrame.y-secondFrame.y-secondFrame.height>=find(compiled.document,'edge-label-3').height+40-1e-6,'vertical panel gap reserves caption height');
 assert.ok(find(compiled.document,'edge-label-0').width>=104,'vertical relation gets readable line width');
 for(const id of ['edge-label-1','edge-label-3']){
  const caption=find(compiled.document,id),l=caption.x,r=l+caption.width,t=caption.y,b=t+caption.height;
  for(const frame of compiled.document.elements.filter(e=>e.type==='frame')){
   const fl=frame.x,fr=fl+frame.width,ft=frame.y,fb=ft+frame.height;
   assert.ok(!(l<fr&&r>fl&&((t<ft+2&&b>ft-2)||(t<fb+2&&b>fb-2))||t<fb&&b>ft&&((l<fl+2&&r>fl-2)||(l<fr+2&&r>fr-2))),`${id} clears ${frame.id} border`);
  }
 }
 await preview(compiled,initial.metadata.revision,'compiled.png');
 await call('POST',`${path}/proposals/${compiled.id}/accept`);
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
 const exportFile=async(label,name)=>{const event=page.waitForEvent('download');await page.getByRole('button',{name:label,exact:true}).click();const file=await(await event).path();const bytes=await readFile(file);await writeFile(join(directory,name),bytes);return bytes;};
 const compiledSvg=(await exportFile('导出 SVG','compiled.svg')).toString();
 const svgTexts=await page.evaluate(svg=>Array.from(new DOMParser().parseFromString(svg,'image/svg+xml').querySelectorAll('text'),e=>e.textContent),compiledSvg);
 assert.ok(!svgTexts.some(text=>text==='Frame'),'no automatic Frame heading in actual SVG');
 const visibleText=svgTexts.join('').replace(/\s/g,'');
 for(const panel of plan.panels)assert.ok(visibleText.includes(panel.title.replace(/\s/g,'')),'wrapped scientific panel heading survives SVG export');
 await exportFile('导出 PNG','compiled-export.png');
 const restoredCompiled=JSON.parse((await exportFile('导出 源文件','compiled-source.json')).toString());
 for(const [index,panel]of plan.panels.entries()){
  assert.equal(find(restoredCompiled,`panel-${index}`).name,'');
  assert.equal(find(restoredCompiled,`panel-title-${index}`).originalText,panel.title);
 }
 for(const e of compiled.document.elements){
  const restored=find(restoredCompiled,e.id);
  for(const key of ['x','y','width','height','points','text','originalText','containerId','startBinding','endBinding'])if(Object.hasOwn(e,key))assert.deepEqual(restored[key],JSON.parse(JSON.stringify(e[key])),`compiled ${e.id}/${key} survives JSON native restore`);
 }
 // Preserve the actual old route as a separate revision, then request a local repair.
 const sceneInput=JSON.parse(await readFile(process.env.VISUALIZATION_PANEL_SCENE||new URL('../../../crates/visualization/tests/fixtures/ifng-short-terminal.json',import.meta.url),'utf8'));
 const document=sceneInput.scene?.document??sceneInput.document??sceneInput;
 const head=await call('GET',path);await call('PUT',path,{expected_revision:head.metadata.revision,title:'Native panel route',document});
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
 const formal=await call('GET',path),revision=formal.metadata.revision;
 const propose=async(summary,operations)=>{const p=await call('POST',`${path}/proposals`,{expected_revision:revision,summary,operations});return call('GET',`${path}/proposals/${p.id}`);};
 const before=await propose('Old short terminal',[]);await preview(before,revision,'before.png');
 const after=await propose('Repair terminal approach',[{op:'reroute',id:'edge-3'}]);
 const arrow=find(after.document,'edge-3'),points=arrow.points;
 const span=Math.hypot(points.at(-1)[0]-points.at(-2)[0],points.at(-1)[1]-points.at(-2)[1]);
 assert.ok(span>=24,`visible native arrowhead approach: ${span}`);
 for(const old of document.elements){
  const next=find(after.document,old.id);
  if(!['edge-3','edge-label-3'].includes(old.id))assert.deepEqual(next,old,`unrelated ${old.id}`);
 }
 const oldArrow=find(document,'edge-3'),caption=find(after.document,'edge-label-3'),oldCaption=find(document,'edge-label-3');
 for(const key of ['startBinding','endBinding','startArrowhead','endArrowhead','boundElements'])assert.deepEqual(arrow[key],oldArrow[key]);
 for(const key of ['text','originalText','fontSize','fontFamily','width','height','containerId'])assert.deepEqual(caption[key],oldCaption[key]);
 assert.deepEqual(await call('GET',path),formal,'preview leaves formal scene unchanged');
 await preview(after,revision,'after.png');
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();
 const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.getByRole('button',{name:`${after.summary} · v${revision}`,exact:true}).click();
 await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 await page.screenshot({path:join(directory,'review.png'),animations:'disabled'});
 await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const adopted=await call('GET',path);assert.deepEqual(adopted.scene.document,after.document);
 const exported=JSON.parse((await exportFile('导出 源文件','source.json')).toString());
 for(const e of after.document.elements){const restored=find(exported,e.id);for(const key of ['x','y','width','height','points','startBinding','endBinding','startArrowhead','endArrowhead','customData'])assert.deepEqual(restored[key],e[key],`${e.id}/${key}`);}
 assert.deepEqual(exported.files,after.document.files);assert.deepEqual(errors,[]);
 // Explicit compact width removes invisible box extent without changing the wording.
 const noteOperation={op:'add_wrapped_text',id:'compact-note',text:'Artwork is a generic membrane receptor illustration, not an IFNGR-specific molecular structure.',x:0,y:Math.max(...after.document.elements.map(e=>e.y+e.height))+40,max_width:1180,font_size:16};
 const noteCandidate=async(fit_width,summary)=>{
  const response=await call('POST',`${path}/proposals`,{expected_revision:adopted.metadata.revision,summary,operations:[{...noteOperation,fit_width}]});
  return call('GET',`${path}/proposals/${response.id}`);
 };
 const legacyNote=await noteCandidate(false,'Fixed note width'),compactNote=await noteCandidate(true,'Compact note width');
 const fixed=find(legacyNote.document,'compact-note'),fitted=find(compactNote.document,'compact-note');
 assert.equal(fixed.width,1180);assert.ok(fitted.width<fixed.width);
 for(const key of ['text','originalText','x','y','height','fontSize','fontFamily','lineHeight','textAlign','autoResize'])assert.deepEqual(fitted[key],fixed[key]);
 for(const old of after.document.elements)assert.deepEqual(find(compactNote.document,old.id),old);
 await preview(legacyNote,adopted.metadata.revision,'fixed-note.png');await preview(compactNote,adopted.metadata.revision,'compact-note.png');
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();
 await dialog.getByRole('button',{name:`${legacyNote.summary} · v${adopted.metadata.revision}`,exact:true}).click();
 await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const legacyHead=await call('GET',path);
 const fittedBrief=await call('POST',`${path}/proposals`,{expected_revision:legacyHead.metadata.revision,summary:'Fit existing note width',operations:[{op:'fit_text_width',id:'compact-note'}]});
 const fittedExisting=await call('GET',`${path}/proposals/${fittedBrief.id}`),existingNote=find(fittedExisting.document,'compact-note');
 assert.equal(existingNote.width,fitted.width);
 for(const old of legacyHead.scene.document.elements){
  const next=find(fittedExisting.document,old.id);
  if(old.id!=='compact-note')assert.deepEqual(next,old);
  else for(const key of Object.keys(old).filter(k=>!['x','width','version','versionNonce','updated'].includes(k)))assert.deepEqual(next[key],old[key],`existing note ${key}`);
 }
 await preview(fittedExisting,legacyHead.metadata.revision,'fit-existing-note.png');
 assert.deepEqual(await call('GET',path),legacyHead,'fitting waits for user adoption');
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();
 await dialog.getByRole('button',{name:`${fittedExisting.summary} · v${legacyHead.metadata.revision}`,exact:true}).click();
 await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 await persist('fit-existing-note-proposal.json',fittedExisting);
 const compactExport=JSON.parse((await exportFile('导出 源文件','compact-note-source.json')).toString());
 for(const key of ['text','originalText','x','y','width','height','fontFamily','fontSize','lineHeight','autoResize'])assert.deepEqual(find(compactExport,'compact-note')[key],fitted[key]);
 await exportFile('导出 PNG','compact-note-export.png');
 let sourceNoteResult=null;
 if(process.env.VISUALIZATION_FIT_EXISTING_TEXT_ID){
  const targetId=process.env.VISUALIZATION_FIT_EXISTING_TEXT_ID,head=await call('GET',path),prior=find(head.scene.document,targetId);assert.ok(prior);
  const sourceProposal=async(summary,operations)=>{const brief=await call('POST',`${path}/proposals`,{expected_revision:head.metadata.revision,summary,operations});return call('GET',`${path}/proposals/${brief.id}`);};
  const beforeFit=await sourceProposal('Saved source before text fit',[]),afterFit=await sourceProposal('Fit saved source text',[{op:'fit_text_width',id:targetId}]);
  for(const old of head.scene.document.elements){const next=find(afterFit.document,old.id);if(old.id!==targetId)assert.deepEqual(next,old);else for(const key of Object.keys(old).filter(k=>!['x','width','version','versionNonce','updated'].includes(k)))assert.deepEqual(next[key],old[key]);}
  assert.deepEqual(afterFit.document.files,head.scene.document.files);
  const fittedSource=find(afterFit.document,targetId);assert.ok(fittedSource.width<prior.width);
  await preview(beforeFit,head.metadata.revision,'source-note-before.png');await preview(afterFit,head.metadata.revision,'source-note-after.png');
  await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();await dialog.getByRole('button',{name:`${afterFit.summary} · v${head.metadata.revision}`,exact:true}).click();
  await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
  const sourceExport=JSON.parse((await exportFile('导出 源文件','source-note-export.json')).toString());
  for(const key of ['text','originalText','x','y','width','height','fontSize','fontFamily','customData'])assert.deepEqual(find(sourceExport,targetId)[key],fittedSource[key]);
  await persist('source-note-proposal.json',afterFit);sourceNoteResult={element_id:targetId,before_width:prior.width,after_width:fittedSource.width};
 }
 await persist('compact-note-proposal.json',compactNote);
 await persist('proposal.json',after);await persist('compiled.json',compiled);
 await persist('result.json',{passed:true,model_used:false,terminal_span:span,compact_note:{fixed_width:fixed.width,fitted_width:fitted.width,existing_note_verified:true},source_note:sourceNoteResult,scope:'saved IFNG geometry; new compiled panel names, native candidate PNGs, actual SVG/PNG/source downloads, local reroute preservation, GUI adoption and source restore; no new scientific validation'});
 console.log(`panel-route: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`panel-route: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
