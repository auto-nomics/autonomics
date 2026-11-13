import {checkMarkupHandoff} from './helpers/canvas-markup-handoff.mjs';
import {checkAnnotationHandoff} from './helpers/canvas-annotation-handoff.mjs';
import {checkHumanCandidateImage} from './helpers/canvas-human-candidate-image.mjs';
import {checkHumanFeedbackDrafts} from './helpers/canvas-human-feedback-drafts.mjs';
import {checkHumanLabelPosition} from './helpers/canvas-human-label-position.mjs';
import {checkHumanCandidateArtwork} from './helpers/canvas-human-candidate-artwork.mjs';
import {checkHumanRestoreCandidate} from './helpers/canvas-human-restore-candidate.mjs';
import {checkHumanCandidateEvidence} from './helpers/canvas-human-candidate-evidence.mjs';
import {checkHumanCandidateRelation} from './helpers/canvas-human-candidate-relation.mjs';
import {checkHumanCandidateDrafts} from './helpers/canvas-human-candidate-drafts.mjs';
import {checkHumanCandidateNode} from './helpers/canvas-human-candidate-node.mjs';
import {checkHumanCandidateText,checkCandidateChildPagination} from './helpers/canvas-human-candidate-text.mjs';
// Derive an isolated scientific label fixture from an explicit saved scene; no model.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
assert.ok(process.env.VISUALIZATION_CANDIDATE_VIEW_MODES!=='1'||process.env.VISUALIZATION_CANDIDATE_DRAFTS==='1','view mode evaluation requires candidate draft flow');
assert.ok(process.env.VISUALIZATION_HISTORICAL_MARKUP!=='1'||process.env.VISUALIZATION_MARKUP_HANDOFF==='1','historical markup requires direct markup flow');
const directory=await mkdtemp(join(tmpdir(),'autonomics-scientific-wrap-')),origin='http://127.0.0.1:1459';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1459','--strictPort'],{stdio:['ignore','pipe','pipe']});
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
 const source=process.env.VISUALIZATION_TEXT_FIT_SCENE,targetId=process.env.VISUALIZATION_TEXT_FIT_ID;
 assert.ok(source&&targetId,'set VISUALIZATION_TEXT_FIT_SCENE and VISUALIZATION_TEXT_FIT_ID for this explicit saved-scene replay');
 const input=JSON.parse(await readFile(source,'utf8')),document=input.scene?.document??input.document??input;
 const fullScene=process.env.VISUALIZATION_TEXT_FIT_FULL_SCENE==='1';
 const tokens=process.env.VISUALIZATION_TEXT_FIT_TOKENS?JSON.parse(process.env.VISUALIZATION_TEXT_FIT_TOKENS):['APAF1','ADP','ATP'];assert.ok(Array.isArray(tokens)&&tokens.length>0&&tokens.every(token=>typeof token==='string'&&token.length>0));
 const label=document.elements.find(e=>e.id===targetId);assert.ok(label);if(!fullScene){document.elements=[{...label,x:0,y:0,containerId:null,frameId:null,groupIds:[],boundElements:null}];document.files={};}
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Fit existing text',document});
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
 const formal=await call('GET',path),revision=formal.metadata.revision,find=(doc,id)=>doc.elements.find(e=>e.id===id),prior=find(document,targetId);assert.ok(prior);
 const propose=async(summary,operations)=>{const brief=await call('POST',`${path}/proposals`,{expected_revision:revision,summary,operations});return call('GET',`${path}/proposals/${brief.id}`);};
 const before=await propose('Before fitting text',[]),after=await propose('Reflow scientific formula',[{op:'set_wrapped_text',id:targetId,text:prior.originalText,...(prior.containerId?{}:{max_width:prior.width})}]),fitted=find(after.document,targetId);
 assert.equal(fitted.width,prior.width);for(const token of tokens)assert.ok(fitted.text.includes(token),token);assert.notEqual(fitted.text,prior.text);assert.equal(fitted.originalText,prior.originalText);
 const restored=structuredClone(after.document),old=structuredClone(prior);
 for(const key of ['x','y','width','height','text','originalText','version','versionNonce','updated']){if(Object.hasOwn(old,key))find(restored,targetId)[key]=old[key];else delete find(restored,targetId)[key];}
 assert.deepEqual(restored,document,'only text reflow, alignment geometry and revision stamps may change');
 const render=async(candidate,name)=>{
  await call('POST',`${path}/proposals/${candidate.id}/preview-request`,{expected_revision:revision});let result;
  await until(async()=>{result=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=${revision}`);assert.notEqual(result.render_status,'failed');return result.render_status==='ready';},'candidate PNG');
  assert.equal(result.document_sha256,candidate.document_sha256);const bytes=Buffer.from(result.dataURL.split(',')[1],'base64');await writeFile(join(directory,name),bytes);return {width:bytes.readUInt32BE(16),height:bytes.readUInt32BE(20)};
 };
 const beforePng=await render(before,'before.png'),afterPng=await render(after,'after.png');
 assert.deepEqual(await call('GET',path),formal,'preview does not adopt');
 if(process.env.VISUALIZATION_EXPECT_TEXT_CANVAS_SHRINK==='1'){assert.ok(afterPng.width<beforePng.width);assert.equal(afterPng.height,beforePng.height);}
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.getByRole('button',{name:`${after.summary} · v${revision}`,exact:true}).click();
 await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const adopted=await call('GET',path);assert.deepEqual(adopted.scene.document,after.document);
 const event=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await event).path(),'utf8'));
 for(const e of after.document.elements){const actual=find(exported,e.id);for(const key of ['x','y','width','height','points','text','originalText','fontSize','fontFamily','lineHeight','customData','frameId','groupIds','startBinding','endBinding'])if(Object.hasOwn(e,key))assert.deepEqual(actual[key],JSON.parse(JSON.stringify(e[key])),`${e.id}/${key}`);}
 assert.deepEqual(exported.files,after.document.files);assert.deepEqual(errors,[]);

 if(process.env.VISUALIZATION_TEXT_CANDIDATE_EDIT==='1'){assert.ok(fullScene);await checkHumanCandidateText({page,call,path,persist,directory,until});}
 if(process.env.VISUALIZATION_CANDIDATE_NODE==='1'){assert.ok(fullScene);await checkHumanCandidateNode({page,call,path,persist,directory,until});}
 if(process.env.VISUALIZATION_CANDIDATE_DRAFTS==='1'){assert.ok(fullScene);await checkHumanCandidateDrafts({page,call,path,persist,directory,until});}
 if(process.env.VISUALIZATION_CANDIDATE_CHILDREN==='1'){assert.ok(process.env.VISUALIZATION_TEXT_CANDIDATE_EDIT==='1');await checkCandidateChildPagination({page,call,project,persist,directory,until});}
 if(process.env.VISUALIZATION_CANDIDATE_RELATION==='1')await checkHumanCandidateRelation({page,call,project,persist,directory,until});
 if(process.env.VISUALIZATION_CANDIDATE_EVIDENCE==='1')await checkHumanCandidateEvidence({page,call,project,persist,directory,until});
 if(process.env.VISUALIZATION_RESTORE_CANDIDATE==='1'){assert.ok(fullScene);await checkHumanRestoreCandidate({page,call,path,persist,directory,until});}
 if(process.env.VISUALIZATION_CANDIDATE_ARTWORK==='1'){assert.ok(fullScene);await checkHumanCandidateArtwork({page,call,path,project,persist,directory,until});}
 if(process.env.VISUALIZATION_CANDIDATE_LABEL_POSITION==='1'){assert.ok(fullScene);await checkHumanLabelPosition({page,call,path,persist,directory,until});}
 if(process.env.VISUALIZATION_AGENT_FEEDBACK_DRAFTS==='1'){assert.ok(fullScene);await checkHumanFeedbackDrafts({page,call,path,persist,directory,until});}
 if(process.env.VISUALIZATION_CANDIDATE_IMAGE==='1'){assert.ok(fullScene);await checkHumanCandidateImage({page,call,path,persist,directory,until});}
 if(process.env.VISUALIZATION_ANNOTATION_HANDOFF==='1')await checkAnnotationHandoff({page,call,project,persist,directory,until});
 if(process.env.VISUALIZATION_MARKUP_HANDOFF==='1')await checkMarkupHandoff({page,call,project,persist,directory,until});
 assert.deepEqual(errors,[],'no page errors during optional candidate UI flows');
 await persist('source.json',exported);await persist('proposal.json',after);assert.equal(sent,0);await persist('result.json',{passed:true,model_used:false,historical_markup_verified:process.env.VISUALIZATION_HISTORICAL_MARKUP==='1',markup_handoff_verified:process.env.VISUALIZATION_MARKUP_HANDOFF==='1',annotation_handoff_verified:process.env.VISUALIZATION_ANNOTATION_HANDOFF==='1',candidate_image_verified:process.env.VISUALIZATION_CANDIDATE_IMAGE==='1',agent_feedback_drafts_verified:process.env.VISUALIZATION_AGENT_FEEDBACK_DRAFTS==='1',candidate_view_modes_verified:process.env.VISUALIZATION_CANDIDATE_VIEW_MODES==='1',candidate_label_position_verified:process.env.VISUALIZATION_CANDIDATE_LABEL_POSITION==='1',candidate_artwork_verified:process.env.VISUALIZATION_CANDIDATE_ARTWORK==='1',candidate_restoration_verified:process.env.VISUALIZATION_RESTORE_CANDIDATE==='1',candidate_evidence_verified:process.env.VISUALIZATION_CANDIDATE_EVIDENCE==='1',plan_relation_review_verified:process.env.VISUALIZATION_PLAN_RELATION_REVIEW==='1',candidate_relation_verified:process.env.VISUALIZATION_CANDIDATE_RELATION==='1',candidate_inspector_layout_verified:process.env.VISUALIZATION_CANDIDATE_INSPECTOR==='1',human_candidate_drafts_verified:process.env.VISUALIZATION_CANDIDATE_DRAFTS==='1',human_candidate_node_verified:process.env.VISUALIZATION_CANDIDATE_NODE==='1',human_candidate_edit_verified:process.env.VISUALIZATION_TEXT_CANDIDATE_EDIT==='1',candidate_children_pagination_verified:process.env.VISUALIZATION_CANDIDATE_CHILDREN==='1',scope:process.env.VISUALIZATION_TEXT_CANDIDATE_EDIT==='1'?'explicit saved full scene native reflow plus human candidate wording/layout revisions, preserved parents and unrelated content, PNG/adoption/export and stale/branch guards; optional child navigation uses a separate empty-scene alternatives fixture':fullScene?'explicit saved full scene; one native text reflow with all unrelated objects, source metadata and files preserved; PNG, adoption and export; no scientific text changes':'isolated text fixture derived from explicit saved label, same width/font; new native reflow, PNG, adoption and export; not full scientific scene replay'});console.log(`scientific-wrap: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`scientific-wrap: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
