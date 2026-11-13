import {clickCanvasAction} from './helpers/canvas-action.mjs';
// Frontend regression with a controlled visualization API; no model calls.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {existsSync} from 'node:fs';
import {mkdtemp,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
const directory=await mkdtemp(join(tmpdir(),'autonomics-project-open-'));
const origin='http://127.0.0.1:1466';
const vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1466','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',browser,page;
vite.stdout.on('data',v=>output+=v);vite.stderr.on('data',v=>output+=v);
const until=async(fn,label)=>{const deadline=Date.now()+30000;while(!await fn()){assert.ok(Date.now()<deadline,label);await new Promise(r=>setTimeout(r,50));}};
try{
 await until(()=>{assert.equal(vite.exitCode,null,output);return output.includes(origin);},'Vite ready');
 browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||(existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined),args:['--no-sandbox']});
 page=await browser.newPage({viewport:{width:1600,height:1000}});page.setDefaultTimeout(15000);
 const errors=[],projects=[],figures=new Map(),specialists=new Map(),sent=[];let nextId=0,failNext=false,firstList=true,releaseInitialList,markCaptured;
 const captured=new Promise(resolve=>{markCaptured=resolve;});page.on('pageerror',e=>errors.push(e.message));
 await page.route('**/api/session',r=>r.fulfill({json:{}}));
 await page.route('**/api/v1/**',async route=>{
  const p=new URL(route.request().url()).pathname.replace('/api/v1',''),method=route.request().method();
  const reply=json=>route.fulfill({json});
  if(p==='/visualization/projects'){
   if(method==='GET')return reply(projects);
   const project=route.request().postDataJSON();projects.push(project);figures.set(project.id,[]);return reply(project);
  }
  const list=p.match(/^\/visualization\/projects\/([^/]+)\/figures$/);
  if(list){const records=figures.get(list[1]);assert.ok(records);
   if(method==='GET'){
    const snapshot=records.map(r=>structuredClone(r.metadata));
    if(firstList){firstList=false;markCaptured();await new Promise(resolve=>{releaseInitialList=resolve;});}
    return reply(snapshot);
   }
   await captured;
   if(failNext){failNext=false;return route.fulfill({status:503,json:{error:'Temporary canvas creation failure'}});}
   const body=route.request().postDataJSON(),metadata={id:`figure-${++nextId}`,project_id:list[1],revision:1,title:body.title,updated_at:new Date().toISOString()};
   records.unshift({metadata,scene:{document:body.document}});return reply(metadata);
  }
  const figure=p.match(/^\/visualization\/projects\/([^/]+)\/figures\/([^/]+)(.*)$/);
  if(figure){const record=figures.get(figure[1])?.find(r=>r.metadata.id===figure[2]);assert.ok(record);
   if(!figure[3])return reply(record);
   if(figure[3]==='/proposal-summary')return reply({current_revision:1,total:0,current:0,stale:0,latest:null});
   if(figure[3]==='/proposal-index')return reply({items:[],next_before:null});
   if(figure[3]==='/markups'&&new URL(route.request().url()).searchParams.get('page')==='true')return reply({items:[],next_before:null});
   if(figure[3].endsWith('/preview'))return reply(method==='POST'?{revision:1}:null);
   return reply([]);
  }
  if(p==='/agents'&&method==='POST'){
   const body=route.request().postDataJSON();assert.equal(body.profile,'illustrator');const path=`${body.parent_path}/${body.name}`;
   assert.ok(!specialists.has(path),'one specialist per figure');specialists.set(path,body);return reply({path});
  }
  if(p.startsWith('/agents/')&&p.endsWith('/messages')&&method==='POST'){sent.push({path:p,text:route.request().postDataJSON().text});return reply({});}
  if(p.startsWith('/agents/')&&p.endsWith('/context'))return reply({agent_id:`id-${p}`,context:null});
  if(p.endsWith('/history'))return reply([]);
  if(p==='/events')return new Promise(()=>{});
  if(p==='/state')return reply({last_seq:0,agents:[],contexts:{},display_settings:{collapse_thinking:false,collapse_tool_calls:false,collapse_tool_results:false}});
  if(p==='/profiles')return reply(['researcher','illustrator']);
  if(p==='/model-config')return reply({models:[],providers:[]});
  if(p==='/storage/agents')return reply([]);
  if(p==='/gateway/status')return reply({running:true});
  return reply({});
 });
 await page.goto(origin,{waitUntil:'domcontentloaded'});await page.getByRole('navigation',{name:'工作台活动栏'}).getByRole('button',{name:'画布',exact:true}).click();
 const sidebar=page.locator('section[aria-label="画布列表"]');
 await page.getByRole('textbox',{name:'新项目名称'}).fill('Project opens canvas');await page.getByRole('button',{name:'创建项目',exact:true}).click();
 await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const project=await page.getByRole('combobox',{name:'可视化项目'}).inputValue();assert.equal(figures.get(project).length,1);
 const staleResponse=page.waitForResponse(r=>r.url().endsWith(`/projects/${project}/figures`)&&r.request().method()==='GET');releaseInitialList();await staleResponse;
 await page.getByRole('tab',{name:'未命名画布',exact:true}).waitFor();await sidebar.getByRole('button',{name:'未命名画布',exact:true}).waitFor();
 assert.equal(await page.getByRole('textbox',{name:'新项目名称'}).count(),0,'project creation form collapses after creation');assert.equal(await page.getByRole('button',{name:'将视图移到底部面板',exact:true}).isVisible(),false);
 await page.getByRole('button',{name:'视图位置',exact:true}).first().click();await page.getByRole('menuitemradio',{name:'底部面板',exact:true}).waitFor();await page.keyboard.press('Escape');
 await page.getByRole('button',{name:'关闭 未命名画布',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor({state:'hidden'});
 await sidebar.getByRole('button',{name:'未命名画布',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 failNext=true;await page.getByRole('button',{name:'新建项目',exact:true}).click();await page.getByRole('textbox',{name:'新项目名称'}).fill('Recoverable project');await page.getByRole('button',{name:'创建项目',exact:true}).click();
 await page.getByRole('alert').filter({hasText:'项目已创建，但首张画布创建失败'}).waitFor();
 const second=await page.getByRole('combobox',{name:'可视化项目'}).inputValue();assert.notEqual(second,project);assert.equal(projects.length,2);assert.equal(figures.get(second).length,0);
 await page.getByRole('button',{name:'新建画布',exact:true}).click();await until(async()=>await page.getByRole('tab',{name:'未命名画布',exact:true}).count()===2,'retry opens second tab');assert.equal(figures.get(second).length,1);
 await page.getByRole('button',{name:'新建画布',exact:true}).click();await until(async()=>await page.getByRole('tab',{name:'未命名画布',exact:true}).count()===3,'manual add opens another tab');assert.equal(figures.get(second).length,2);assert.equal(figures.get(project).length,1);
 await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();assert.deepEqual(errors,[]);
 await until(async()=>await page.getByRole('button',{name:'新建画布',exact:true}).isEnabled(),'creation has settled');await page.locator('[data-slot=workbench-sidebar]').screenshot({path:join(directory,'sidebar-dark.png')});await page.evaluate(async()=>{const theme=await import('/src/lib/theme.ts');theme.setThemeMode('light');await new Promise(requestAnimationFrame);await new Promise(requestAnimationFrame);});await page.locator('[data-slot=workbench-sidebar]').screenshot({path:join(directory,'sidebar-light.png')});
 await page.setViewportSize({width:1000,height:820});
 const header=page.getByRole('group',{name:'画布工具栏',exact:true});assert.ok((await header.boundingBox()).height<=48,'canvas name and save stay compact inside footer');assert.equal(await header.getByRole('button').count(),1,'footer title group contains save');
 await page.getByRole('toolbar',{name:'画布快捷操作',exact:true}).waitFor();
 assert.equal(await page.getByRole('toolbar',{name:'画布快捷操作',exact:true}).getByRole('button',{name:'素材',exact:true}).count(),0,'materials use the native browser entry only');
 await clickCanvasAction(page,'历史版本');const history=page.getByRole('dialog',{name:'画布历史',exact:true});await history.waitFor();await history.getByRole('button',{name:'关闭历史',exact:true}).click();
 const download=page.waitForEvent('download');await clickCanvasAction(page,'导出 源文件');await download;
 assert.equal(await page.getByRole('button',{name:'Agent 绘图',exact:true}).count(),0);
 assert.equal(await page.getByRole('complementary',{name:'画布 Agent',exact:true}).count(),0);
 await page.getByRole('button',{name:'审阅批注',exact:true}).click();await page.getByRole('button',{name:'框选批注',exact:true}).waitFor();await page.getByRole('button',{name:'审阅批注',exact:true}).press('Escape');assert.equal(await page.getByRole('button',{name:'审阅批注',exact:true}).getAttribute('aria-expanded'),'false');
 await page.getByRole('region',{name:'画布编辑器',exact:true}).screenshot({path:join(directory,'canvas-actions.png')});
 await page.setViewportSize({width:640,height:820});
 const activeSurface=page.locator('[data-canvas-surface]:visible');
 await until(async()=>await activeSurface.locator('.App-toolbar-content [data-canvas-action-dock]').count()===1,'narrow footer adapts without Agent sidebar');
 assert.equal(await activeSurface.locator('.App-toolbar-content [data-canvas-action-dock]').count(),1,'narrow canvas uses the native bottom bar');
 await clickCanvasAction(page,'历史版本');await history.waitFor();await page.setViewportSize({width:1600,height:900});
 await until(async()=>await activeSurface.locator('.layer-ui__wrapper__footer [data-canvas-action-dock]').count()===1,'wide canvas uses public Footer slot');await history.waitFor();await history.getByRole('button',{name:'关闭历史',exact:true}).click();
 assert.equal(await activeSurface.locator('[data-canvas-action-dock]').count(),1,'layout changes preserve a single action host');
 await page.getByRole('region',{name:'画布编辑器',exact:true}).screenshot({path:join(directory,'native-footer-desktop.png')});
 await clickCanvasAction(page,'BioArt 素材');const library=activeSurface.locator('.canvas-material-browser');await library.waitFor();await page.getByRole('textbox',{name:'BioArt 搜索词',exact:true}).fill('cell');
 assert.equal(await page.locator('[data-canvas-panels] aside[aria-label="BioArt 素材"]').count(),0,'asset browser is inside the canvas, not a second app panel');
 assert.equal(await library.getByRole('combobox',{name:'素材库来源'}).inputValue(),'bioart','dropdown follows native browser selection');
 await library.getByRole('combobox',{name:'素材库来源'}).selectOption('bioicons');await page.getByRole('textbox',{name:'BioIcons 搜索词',exact:true}).fill('receptor');await library.getByRole('combobox',{name:'素材库来源'}).selectOption('bioart');assert.equal(await page.getByRole('textbox',{name:'BioArt 搜索词',exact:true}).inputValue(),'cell');
 await library.locator('.sidebar__close').click();await clickCanvasAction(page,'BioArt 素材');assert.equal(await page.getByRole('textbox',{name:'BioArt 搜索词',exact:true}).inputValue(),'cell','closing native sidebar retains search state');
 await page.setViewportSize({width:640,height:820});await until(async()=>await activeSurface.locator('.App-toolbar-content [data-canvas-action-dock]').count()===1,'native footer stays integrated after narrowing with library open');
 await page.getByRole('textbox',{name:'BioArt 搜索词',exact:true}).waitFor();
 await writeFile(join(directory,'library-hit-test.json'),JSON.stringify(await page.getByRole('textbox',{name:'BioArt 搜索词',exact:true}).evaluate(e=>{const r=e.getBoundingClientRect();return document.elementsFromPoint(r.x+10,r.y+10).slice(0,12).map(n=>({tag:n.tagName,cls:n.className,style:{z:getComputedStyle(n).zIndex,opacity:getComputedStyle(n).opacity},label:n.getAttribute('aria-label')}));}),null,2));
 await page.getByRole('textbox',{name:'BioArt 搜索词',exact:true}).click();
 await writeFile(join(directory,'library-layout.json'),JSON.stringify(await page.getByRole('textbox',{name:'BioArt 搜索词',exact:true}).evaluate(e=>{const a=[];for(let i=0;e&&i<9;i++,e=e.parentElement){const r=e.getBoundingClientRect(),s=getComputedStyle(e);a.push({tag:e.tagName,cls:e.className,rect:{x:r.x,y:r.y,w:r.width,h:r.height},display:s.display,visibility:s.visibility,opacity:s.opacity,color:s.color,position:s.position});}return a;}),null,2));
 await page.getByRole('region',{name:'画布编辑器',exact:true}).screenshot({path:join(directory,'native-material-browser.png')});assert.deepEqual(errors,[]);
 const binding=await page.evaluate(async()=>{
  const store=await import('/src/app/workbench/workbench-store.ts');
  const editors=store.allGroups(store.getWorkbenchState().groups).flatMap(([,group])=>group.editors);
  const canvas=editors.filter(e=>e.kind==='canvas').at(-1);return {id:canvas.id,path:canvas.state.illustratorPath,figure:canvas.params.canvasId,project:canvas.params.projectId};
 });
 assert.ok(binding.path?.startsWith('/root/illustrator_'));
 assert.equal(specialists.size,3,'reopening a canvas reuses its specialist');
 await activeSurface.getByTestId('main-menu-trigger').click();await page.getByText('打开插画专家对话',{exact:true}).click();
 const composer=page.locator('textarea[placeholder="输入消息…"]:visible');await composer.waitFor();
 assert.equal(await page.getByRole('combobox',{name:'Agent 上下文目标',exact:true}).inputValue(),binding.id);
 await composer.fill('请调整这张图的布局');await page.getByRole('button',{name:'发送',exact:true}).click();
 await until(()=>sent.length===1,'native Agent chat sends request');
 assert.ok(decodeURIComponent(sent[0].path).includes(binding.path));assert.ok(sent[0].text.includes('请调整这张图的布局'));
 assert.ok(sent[0].text.includes(binding.figure));assert.ok(sent[0].text.includes(binding.project));
 assert.ok(sent[0].text.includes('selectedObjectIds'));assert.ok(sent[0].text.includes('figureContract'));
 await page.screenshot({path:join(directory,'native-illustrator-chat.png')});assert.deepEqual(errors,[]);
 await writeFile(join(directory,'result.json'),JSON.stringify({passed:true,backend_fixture:true,model_used:false,project_opens_initial_canvas:true,late_empty_list_preserves_new_canvas:true,sidebar_reopens_canvas:true,creation_failure_preserves_project_and_retry:true,manual_add_preserved:true,compact_title_bar:true,in_canvas_action_menus:true,menu_dialog_and_source_export:true,native_footer_responsive_dialog_persistence:true,native_material_tabs_and_search_persistence:true},null,2));
 console.log(`project-open: passed; artifacts=${directory}`);
}catch(error){if(page)await writeFile(join(directory,'diagnostics.json'),JSON.stringify(await page.evaluate(()=>({canvas:[...document.querySelectorAll('[data-canvas-surface]')].map(e=>({width:e.getBoundingClientRect().width,footer:e.querySelectorAll('.layer-ui__wrapper__footer').length,mobile:e.querySelectorAll('.App-toolbar-content').length,center:e.querySelectorAll('.footer-center').length})),hosts:[...document.querySelectorAll('.canvas-native-footer-host')].map(e=>({parent:e.parentElement?.outerHTML.slice(0,350)})),catalogLayout:[...document.querySelectorAll('input[aria-label="BioArt 搜索词"]')].map(e=>{const a=[];for(let i=0;e&&i<9;i++,e=e.parentElement){const r=e.getBoundingClientRect(),s=getComputedStyle(e);a.push({tag:e.tagName,cls:e.className,rect:{x:r.x,y:r.y,w:r.width,h:r.height},display:s.display,visibility:s.visibility,overflow:s.overflow});}return a;}),materials:[...document.querySelectorAll('.canvas-material-browser')].map(e=>e.outerHTML.slice(-7000)),slots:[...document.querySelectorAll('[data-canvas-catalog-slot]')].map(e=>({slot:e.getAttribute('data-canvas-catalog-slot'),html:e.innerHTML.slice(0,500)})),viewport:innerWidth})),null,2)).catch(()=>{});if(page)await page.screenshot({path:join(directory,'failure.png')}).catch(()=>{});console.error(`project-open: failed; artifacts=${directory}`);throw error;
}finally{await page?.unrouteAll({behavior:'ignoreErrors'});await browser?.close();if(vite.exitCode===null){vite.kill();await new Promise(resolve=>vite.once('exit',resolve));}}
