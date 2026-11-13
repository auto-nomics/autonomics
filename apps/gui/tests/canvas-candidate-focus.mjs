import {readCandidatePreparedContext} from './helpers/canvas-prepared-context.mjs';
// Native candidate object focus and exact scoped feedback; no model invocation.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {illustrationConnectorLabelOverlaps} from '../src/features/canvas/canvas-connector-label-overlaps.ts';
import {checkPortableSceneExport} from './helpers/canvas-live-source-review.mjs';
const source=process.env.VISUALIZATION_FIT_SCENE||fileURLToPath(new URL('./fixtures/scientific-egfr-mapk-illustrated.json',import.meta.url));
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-candidate-focus-')),origin='http://127.0.0.1:1456';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1456','--strictPort'],{stdio:['ignore','pipe','pipe']});
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
 const input=JSON.parse(await readFile(source,'utf8'));let document=input.scene?.document??input.document??input;
 if(process.env.VISUALIZATION_CAPTION_PATH_REVIEW==='1') {
  const arrow=document.elements.find(e=>e.type==='arrow'),text=document.elements.find(e=>e.type==='text');assert.ok(arrow&&text);
  const y=Math.max(...document.elements.map(e=>(e.y??0)+(e.height??0)))+100;
  document.elements.push({...arrow,id:'overlap-demo-arrow',x:40,y,width:200,height:0,angle:0,points:[[0,0],[200,0]],startBinding:null,endBinding:null,startArrowhead:null,endArrowhead:'arrow',roundness:null,roughness:0,elbowed:false,frameId:null,groupIds:[],customData:{},boundElements:[{id:'overlap-demo-label',type:'text'}]},
   {...text,id:'overlap-demo-label',x:50,y:y-10,width:180,height:20,angle:0,containerId:'overlap-demo-arrow',frameId:null,groupIds:[],customData:{},boundElements:null,text:'Relation',originalText:'Relation',fontSize:16,fontFamily:9,lineHeight:1.25,autoResize:false});
 }

 if(process.env.VISUALIZATION_INITIAL_LABEL_PLACEMENT==='1') {
  const before=await call('GET',path),plan={message:'Native illustration caption placement test',audience:'Testers',layout:'vertical',panels:[{id:'panel',title:'Synthetic caption placement',column:0,row:0}],nodes:[{id:'egfr',label:'EGFR receptor complex',panel:'panel'},{id:'after',label:'Illustrative response',panel:'panel'}],edges:[{from:'egfr',to:'after',label:'Illustrative transition',evidence:['Synthetic geometry fixture; no biological claim']}]};
  const seed=await call('POST','/test/visualization',{action:'sketch',project_id:project,figure_id:figure,expected_revision:before.metadata.revision,title:'Focus figure',document:plan});
  await call('POST',`${path}/proposals/${seed.content.id}/accept`);const head=await call('GET',path);
  const target=head.scene.document.elements.find(e=>e.customData?.scientificId==='egfr');assert.ok(target);
  const replacement=await call('POST','/test/bioart',{action:'replace',project_id:project,figure_id:figure,expected_revision:head.metadata.revision,element_id:target.id,asset_id:429,markup_ids:[]});
  const proposal=await call('GET',`${path}/proposals/${replacement.content.proposal_id}`);
  assert.deepEqual(await call('GET',path),head,'initial materialization waits for adoption');
  const anchor=proposal.document.elements.find(e=>e.id===target.id),label=proposal.document.elements.find(e=>anchor.customData.visualLabelIds.includes(e.id));
  const old=head.scene.document.elements.find(e=>e.id===label.id);assert.equal(label.originalText,old.originalText);assert.equal(label.fontSize,old.fontSize);
  assert.ok(label.y<target.y+target.height,'caption is placed beside the initial illustration');assert.equal(illustrationConnectorLabelOverlaps(proposal.document).count,0);
  await persist('initial-label-placement.json',proposal);await call('POST',`${path}/proposals/${proposal.id}/accept`);document=proposal.document;
 }
 const anchor=document.elements.find(e=>e.customData?.scientificId==='egfr');assert.ok(anchor);
 if(process.env.VISUALIZATION_CONNECTOR_LABEL_REVIEW==='1') {
  const label=document.elements.find(e=>e.type==='text'&&anchor.customData.visualLabelIds?.includes(e.id));assert.ok(label);
  const template=document.elements.find(e=>e.type==='arrow');assert.ok(template);
  document.elements.push({...structuredClone(template),id:'review-crossing-arrow',x:label.x+label.width/2,y:label.y-20,width:0,height:label.height+40,points:[[0,0],[0,label.height+40]],angle:0,roughness:0,roundness:null,elbowed:false,startBinding:{elementId:anchor.id,focus:0,gap:0},endBinding:null,boundElements:null,groupIds:[],frameId:null,customData:{}});
 }
 if(process.env.VISUALIZATION_PLAN_REVIEW==='1') {
  for(const element of document.elements) if(element.customData) delete element.customData.planningSource;
  const label=document.elements.find(e=>e.type==='text'&&(e.containerId===anchor.id||anchor.customData.visualLabelIds?.includes(e.id)));
  assert.ok(label,'fixture must retain a scientific label');
  anchor.customData.planningSource={version:7,based_on_revision:0,document:{nodes:[{id:'egfr',label:`${label.originalText??label.text} (confirmed)`}],edges:[]}};
 }
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Focus figure',document});page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
 const formal=await call('GET',path),revision=formal.metadata.revision;
 const make=async(summary,operations)=>{const brief=await call('POST',`${path}/proposals`,{expected_revision:revision,summary,operations});return call('GET',`${path}/proposals/${brief.id}`);};
 const addedId='candidate-only-note',add={op:'add_wrapped_text',id:addedId,text:'Candidate-only graphical review',x:0,y:1100,max_width:1000,font_size:16};
 const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
 const show=async p=>{await page.getByRole('tab',{name:'Focus figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();await dialog.getByRole('button',{name:`${p.summary} · v${revision}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();};
 const prepare=async p=>{
  const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill('请仅围绕当前核对对象讨论图形改进，保留科学含义。');await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();
  const composer=page.getByPlaceholder('输入消息…');await composer.waitFor();const draft=await composer.inputValue(),context=await readCandidatePreparedContext(page);
  const focused=p.document.elements.filter(e=>e.id===context.focused_object_id);const scoped=focused.length===1&&focused[0].isDeleted!==true?{element_ids:[context.focused_object_id]}:{};
  assert.deepEqual(context.follow_up_inputs.read_proposal,{action:'read_proposal',project_id:project,figure_id:figure,proposal_id:p.id,expected_revision:revision,...scoped});assert.deepEqual(context.follow_up_inputs.read_proposal_preview,{action:'read_proposal_preview',project_id:project,figure_id:figure,proposal_id:p.id,expected_revision:revision});
  assert.equal(context.reviewed_document_attributes.scope,'document_attributes');assert.equal(context.reviewed_document_attributes.total,0);assert.deepEqual(context.reviewed_document_attributes.fields,[]);assert.equal(context.proposal_id,p.id);assert.equal(context.document_sha256,p.document_sha256);assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${p.id}`),p);assert.equal(sent,0,'preparing object feedback never sends a message');await composer.fill('');return context;
 };
 const unchanged=await make('Unchanged object focus',[add]);await show(unchanged);
 if(process.env.VISUALIZATION_CAPTION_PATH_REVIEW==='1') {
  const check=dialog.locator('details[aria-label="连线说明覆盖检查"]');await check.locator('summary').click();
  assert.ok((await check.textContent()).includes('90%'));
  await check.getByRole('button',{name:'定位拥挤连线 overlap-demo-arrow',exact:true}).click();
  await until(async()=>await dialog.getByRole('region',{name:'候选画布'}).locator('[data-preview-focus="overlap-demo-arrow"]').count()===1,'diagnostic arrow focus');
  await check.getByRole('button',{name:'定位连线说明 overlap-demo-label',exact:true}).click();
  await until(async()=>await dialog.getByRole('region',{name:'候选画布'}).locator('[data-preview-focus="overlap-demo-label"]').count()===1,'diagnostic label focus');
  await page.screenshot({path:join(directory,'caption-path-review.png'),animations:'disabled'});
 }

 if(process.env.VISUALIZATION_CONNECTOR_LABEL_REVIEW==='1') {
  const review=dialog.locator('details[aria-label="连线与插图标签检查"]');await review.locator('summary').click();
  await review.getByRole('button',{name:'定位穿越连线 review-crossing-arrow',exact:true}).click();
  await until(async()=>(await dialog.locator('[data-preview-focus]').evaluateAll(nodes=>nodes.map(n=>n.dataset.previewFocus))).every(id=>id==='review-crossing-arrow'),'both previews focus detected crossing arrow');
  await page.screenshot({path:join(directory,'connector-label-review.png'),animations:'disabled'});
 }
 if(process.env.VISUALIZATION_PLAN_REVIEW==='1') {
  const review=dialog.locator('details[aria-label="确认计划标签核对"]');
  await review.locator('summary').click();assert.ok((await review.textContent()).includes('历史计划 v7'));assert.ok((await review.textContent()).includes('(confirmed)'));
  await review.getByRole('button',{name:'定位节点 1',exact:true}).click();
  await until(async()=>(await dialog.locator('[data-preview-focus]').evaluateAll(nodes=>nodes.map(n=>n.dataset.previewFocus))).every(id=>id===anchor.id),'plan difference focuses native previews');
 }
 await dialog.getByRole('textbox',{name:'搜索候选对象',exact:true}).fill('egfr');await dialog.getByRole('combobox',{name:'候选对象',exact:true}).selectOption(anchor.id);
 await until(async()=>(await dialog.locator('[data-preview-focus]').evaluateAll(nodes=>nodes.map(n=>n.dataset.previewFocus))).every(id=>id===anchor.id),'both previews focus unchanged candidate object');
 if(process.env.VISUALIZATION_INITIAL_LABEL_PLACEMENT==='1')await page.screenshot({path:join(directory,'initial-label-placement.png'),animations:'disabled'});
 const first=await prepare(unchanged);assert.ok(first.candidate_connector_label_check);if(process.env.VISUALIZATION_CONNECTOR_LABEL_REVIEW==='1')assert.ok(first.candidate_connector_label_check.pairs.some(pair=>pair.arrow_id==='review-crossing-arrow'));assert.equal(first.candidate_illustration_label_check.method,'rotated_label_and_image_viewport_rectangles');assert.ok(first.candidate_illustration_label_check.checked>=1);if(process.env.VISUALIZATION_CAPTION_PATH_REVIEW==='1'){assert.equal(first.candidate_caption_path_check.available,true);assert.equal(first.candidate_caption_path_check.proposal_id,unchanged.id);assert.equal(first.candidate_caption_path_check.document_sha256,unchanged.document_sha256);assert.ok(first.candidate_caption_path_check.warnings.some(item=>item.arrow_id==='overlap-demo-arrow'&&item.label_id==='overlap-demo-label'));}if(process.env.VISUALIZATION_PLAN_REVIEW==='1'){assert.equal(first.candidate_plan_label_check.version,7);assert.equal(first.candidate_plan_label_check.items.length,1);assert.equal(first.candidate_plan_label_check.items[0].status,'changed');assert.ok(first.candidate_plan_label_check.items[0].expected.endsWith('(confirmed)'));}assert.equal(first.focused_object_id,anchor.id);assert.equal(first.reviewed_object_change,null,'unchanged object is selectable independently of diff rows');assert.equal(first.focused_object_context.candidate.objects[0].id,anchor.id);assert.deepEqual(first.focused_object_context.candidate,first.focused_object_context.comparison);assert.ok(!JSON.stringify(first.focused_object_context).includes('data:image'));
 const image=document.elements.find(e=>e.type==='image'&&!e.isDeleted);assert.ok(image);await show(unchanged);await dialog.getByRole('textbox',{name:'搜索候选对象',exact:true}).fill(image.id);await dialog.getByRole('combobox',{name:'候选对象',exact:true}).selectOption({value:image.id});
 const imageContext=await prepare(unchanged);assert.deepEqual(imageContext.follow_up_inputs.image_info,{action:'image_info',project_id:project,figure_id:figure,proposal_id:unchanged.id,document_sha256:unchanged.document_sha256,expected_revision:revision,element_id:image.id});assert.equal(imageContext.follow_up_inputs.image_info.markup_ids,undefined);const metadata=await call('POST','/test/visualization',imageContext.follow_up_inputs.image_info);assert.equal(metadata.content.proposal_id,unchanged.id);assert.equal(metadata.content.document_sha256,unchanged.document_sha256);assert.ok(metadata.content.natural_width>0&&metadata.content.natural_height>0);assert.equal(metadata.content.dataURL,undefined);await persist('focused-image-info.json',{context:imageContext.follow_up_inputs.image_info,metadata});
 await show(unchanged);await dialog.getByRole('textbox',{name:'搜索候选对象',exact:true}).fill('Candidate-only graphical');await dialog.getByRole('combobox',{name:'候选对象',exact:true}).selectOption(addedId);
 const second=await prepare(unchanged);assert.equal(second.follow_up_inputs.image_info,undefined,'text focus never invents image inspection arguments');assert.equal(second.focused_object_context.candidate.objects[0].id,addedId);assert.equal(second.focused_object_context.comparison.total,0);assert.deepEqual(second.focused_object_context.comparison.objects,[]);
 const removedId=document.elements.find(e=>e.type==='text'&&!e.containerId&&!e.isDeleted).id,removed=await make('Comparison-only object focus',[{op:'remove',id:removedId}]);await show(removed);await dialog.locator('details[aria-label="逐对象修改明细"] summary').click();await dialog.getByRole('button',{name:`定位对象 ${removedId}`,exact:true}).click();assert.ok((await dialog.getByRole('combobox',{name:'候选对象',exact:true}).locator('option:checked').textContent()).includes('对照版本对象'));
 const third=await prepare(removed);assert.equal(third.focused_object_id,removedId);assert.equal(third.focused_object_context.candidate.total,0);assert.equal(third.focused_object_context.comparison.objects[0].id,removedId);assert.equal(third.reviewed_object_change.kind,'removed');assert.deepEqual(errors,[]);
 const child=async(summary,dx,dy)=>{const result=await call('POST','/test/visualization',{action:'revise_proposal',project_id:project,figure_id:figure,proposal_id:unchanged.id,document_sha256:unchanged.document_sha256,expected_revision:revision,title:summary,allow_branch:true,markup_ids:[],operations:[{op:'translate',element_ids:[addedId],dx,dy}]});return call('GET',`${path}/proposals/${result.content.id}`);};
 const siblingA=await child('Branch A horizontal note',20,0),siblingB=await child('Branch B vertical note',0,30);
 await show(unchanged);await dialog.getByRole('button',{name:`${siblingA.summary} · v${revision} · 修订`,exact:true}).click();
 const alternatives=dialog.locator('[aria-label="同父备选候选"]');await alternatives.waitFor();assert.ok((await alternatives.textContent()).includes('已加载 1 份同父备选'));
 await alternatives.getByRole('button',{name:`核对备选：${siblingB.summary}`,exact:true}).click();
 await until(async()=>await alternatives.getByRole('button',{name:`核对备选：${siblingA.summary}`,exact:true}).count()===1,'sibling switch updates alternatives');
 await dialog.getByRole('button',{name:'对照上一版候选',exact:true}).click();
 const branch=await prepare(siblingB);assert.equal(branch.comparison.kind,'parent_candidate');assert.equal(branch.comparison.proposal_id,unchanged.id);
 assert.deepEqual(await call('GET',`${path}/proposals/${siblingA.id}`),siblingA);assert.deepEqual(await call('GET',`${path}/proposals/${siblingB.id}`),siblingB);assert.deepEqual(await call('GET',path),formal);assert.deepEqual(errors,[]);
 if(process.env.VISUALIZATION_INITIAL_LABEL_PLACEMENT==='1') {
  await page.getByRole('tab',{name:'Focus figure',exact:true}).click();
  const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));await persist('initial-label-source.json',exported);checkPortableSceneExport(exported,formal.scene.document);
 }

 if(process.env.VISUALIZATION_CAPTION_PATH_REVIEW==='1') {
  await page.route('**/proposals/*/geometry-review',route=>route.fulfill({status:503,json:{error:'test diagnostic outage'}}));
  await show(unchanged);
  await dialog.getByText('连线说明检查暂不可用，请结合预览核对。',{exact:true}).waitFor();
  await dialog.getByRole('button',{name:'关闭预览',exact:true}).click();
  let release;const delayed=new Promise(resolve=>release=resolve);
  await page.route('**/proposals/*/geometry-review',async route=>{await delayed;await route.fulfill({status:503,json:{error:'test delayed diagnostic'}}).catch(()=>{});});
  await show(unchanged);
  await dialog.getByText('正在检查连线说明…',{exact:true}).waitFor();
  await dialog.getByText('连线说明检查暂不可用，请结合预览核对。',{exact:true}).waitFor();
  release();
  const unavailable=await prepare(unchanged);assert.deepEqual(unavailable.candidate_caption_path_check,{available:false});
  assert.deepEqual(await call('GET',path),formal);assert.deepEqual(errors,[]);
 }
 await persist('contexts.json',[first,second,third,branch]);await persist('result.json',{passed:true,model_used:false,sent_messages:sent,initial_label_placement:process.env.VISUALIZATION_INITIAL_LABEL_PLACEMENT==='1',caption_path_review:process.env.VISUALIZATION_CAPTION_PATH_REVIEW==='1',scope:'real candidate HTTP/store and native previews; unchanged, candidate-only and comparison-only objects retain distinct bounded feedback contexts'});console.log(`candidate-focus: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`candidate-focus: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
