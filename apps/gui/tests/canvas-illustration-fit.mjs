import {rasterizeFixtureImage} from './helpers/canvas-raster-fixture.mjs';
const existingRaster=process.env.VISUALIZATION_FIT_EXISTING_RASTER==='1';
const rasterMode=existingRaster||process.env.VISUALIZATION_LAYOUT_RASTER==='1';
// Native rendering/round-trip of explicit artwork bounds; no model decisions are simulated.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const scaleMode=process.env.VISUALIZATION_FIT_SCALE==='1';
const editGeometry=process.env.VISUALIZATION_FIT_GEOMETRY==='1'||scaleMode;
const manualFit=process.env.VISUALIZATION_FIT_MANUAL==='1',transformedFit=process.env.VISUALIZATION_FIT_TRANSFORMED==='1';
const fontSymbols=process.env.VISUALIZATION_FIT_FONT_SYMBOLS==='1';
const source=process.env.VISUALIZATION_FIT_SCENE||fileURLToPath(new URL('./fixtures/scientific-egfr-mapk-illustrated.json',import.meta.url));
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-illustration-fit-')),origin='http://127.0.0.1:1453';
const native=spawn(binary,[join(directory,'figures.sqlite')],{stdio:['ignore','pipe','pipe']}),vite=spawn(process.execPath,['node_modules/vite/bin/vite.js','--host','127.0.0.1','--port','1453','--strictPort'],{stdio:['ignore','pipe','pipe']});
let output='',viteOutput='',browser,page;const diagnostics={requests:[],console:[]};native.stdout.on('data',b=>output+=b);native.stderr.on('data',b=>output+=b);vite.stdout.on('data',b=>viteOutput+=b);vite.stderr.on('data',b=>viteOutput+=b);
const persist=(name,value)=>writeFile(join(directory,name),JSON.stringify(value,null,2),{mode:0o600});
const until=async(fn,label)=>{const deadline=Date.now()+60000;while(!await fn()){assert.ok(Date.now()<deadline,label);await new Promise(r=>setTimeout(r,50));}};
try {
 await until(()=>{assert.equal(native.exitCode,null,output);return output.includes('VISUALIZATION_NATIVE_ADDR=');},'native readiness');await until(()=>{assert.equal(vite.exitCode,null,viteOutput);return viteOutput.includes(origin);},'Vite readiness');
 const backend=`http://${output.match(/VISUALIZATION_NATIVE_ADDR=([^\s]+)/)[1]}`;
 const call=async(method,path,body)=>{const r=await fetch(backend+path,{method,headers:{'content-type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(30000)});const v=await r.json();assert.ok(r.ok,JSON.stringify(v));return v;};
 browser=await chromium.launch({executablePath:process.env.PDF_TEST_CHROMIUM||(existsSync('/usr/bin/chromium')?'/usr/bin/chromium':undefined),args:['--no-sandbox']});page=await browser.newPage({viewport:{width:1600,height:1000}});const errors=[];page.on('pageerror',e=>errors.push(e.message));page.on('requestfailed',r=>{if(diagnostics.requests.length<100)diagnostics.requests.push({url:r.url(),error:r.failure()?.errorText});});page.on('console',m=>{if(m.type()==='error'&&diagnostics.console.length<100)diagnostics.console.push(m.text().slice(0,1000));});
 await page.route('**/api/session',r=>r.fulfill({json:{}}));
 await page.route('**/api/v1/**',async route=>{const url=new URL(route.request().url()),p=url.pathname.replace('/api/v1','');if(p.startsWith('/visualization/'))return route.fulfill({response:await route.fetch({url:backend+url.pathname+url.search})});if(p==='/events')return new Promise(()=>{});if(p==='/state')return route.fulfill({json:{last_seq:0,agents:[],contexts:{}}});if(p==='/profiles')return route.fulfill({json:[]});if(p==='/model-config')return route.fulfill({json:{models:[],providers:[]}});if(p==='/settings')return route.fulfill({json:{}});if(p==='/storage/agents')return route.fulfill({json:[]});if(p==='/gateway/status')return route.fulfill({json:{running:true}});return route.fulfill({status:404,json:{error:`unrelated fixture ${p}`}});});
 await page.goto(origin,{waitUntil:'domcontentloaded',timeout:60000});await page.getByRole('navigation',{name:'工作台活动栏'}).getByRole('button',{name:'画布',exact:true}).click();await page.getByRole('textbox',{name:'新项目名称'}).fill('Illustration fit');await page.getByRole('button',{name:'创建项目',exact:true}).click();await page.getByRole('button',{name:'新建画布',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const project=await page.getByRole('combobox',{name:'可视化项目'}).inputValue(),figure=(await call('GET',`/api/v1/visualization/projects/${project}/figures`))[0].id,path=`/api/v1/visualization/projects/${project}/figures/${figure}`;
 const input=JSON.parse(await readFile(source,'utf8')),document=input.scene?.document??input.document??input;const receptor=document.elements.find(e=>e.customData?.scientificId===(process.env.VISUALIZATION_FIT_SCIENTIFIC_ID??'egfr'));assert.ok(receptor);const image=document.elements.find(e=>e.id===receptor.customData.visualAssetElementId);assert.ok(image);
 if(fontSymbols){const caption=document.elements.find(e=>e.type==='text'&&!e.containerId&&!e.frameId);assert.ok(caption);const text=caption.originalText??caption.text;caption.text=`① 范围：${text} ②`;caption.originalText=caption.text;}
 if(rasterMode&&!existingRaster)await rasterizeFixtureImage(page,document,image);
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Artwork fit',document});
 if(transformedFit){
  const saved=await call('GET',path),info=await call('GET',`${path}/image-info?element_id=${encodeURIComponent(image.id)}&expected_revision=${saved.metadata.revision}`);
  image.angle=.42;image.scale=[-1,-1];image.crop={x:info.natural_width*.1,y:0,width:info.natural_width*.8,height:info.natural_height,naturalWidth:info.natural_width,naturalHeight:info.natural_height};
  await call('PUT',path,{expected_revision:saved.metadata.revision,title:'Artwork fit',document});
 }
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();const before=await call('GET',path),revision=before.metadata.revision;
 let bounds;
 if(existingRaster){bounds=receptor.customData.visualAnchorFit?.normalizedBounds;assert.ok(Array.isArray(bounds)&&bounds.length===4,'explicit saved reviewed raster bounds');}else if(rasterMode)bounds=[.2,0,.6,1];else{ const parts=await call('GET',`${path}/vector-parts?expected_revision=${revision}&element_id=${encodeURIComponent(image.id)}`);const visible=parts.parts.filter(p=>p.opacity>0&&(p.fill!=='none'||p.stroke!=='none'));assert.ok(visible.length);const l=Math.max(0,Math.min(...visible.map(p=>p.bounds[0]))),t=Math.max(0,Math.min(...visible.map(p=>p.bounds[1]))),r=Math.min(parts.width,Math.max(...visible.map(p=>p.bounds[0]+p.bounds[2]))),b=Math.min(parts.height,Math.max(...visible.map(p=>p.bounds[1]+p.bounds[3])));bounds=[l/parts.width,t/parts.height,(r-l)/parts.width,(b-t)/parts.height];}
 await persist('reviewed-bounds.json',{method:rasterMode?'explicit source-relative fixture rectangle; not molecular identities':'explicit union of visible SVG path stroke bounds; not molecular identities',file_id:image.fileId,normalized_bounds:bounds});
 const parentResult=(await call('POST','/test/visualization',{action:'propose',project_id:project,figure_id:figure,expected_revision:revision,title:'Before visual anchor fit',operations:[]})).content;const parent=await call('GET',`${path}/proposals/${parentResult.id}`);
 const displayImage=editGeometry?{...image,...(scaleMode?{x:image.x-image.width*.05,y:image.y-image.height*.05}:{}),width:image.width*1.1,height:image.height*1.1}:image;
 let child;
 if(manualFit){
  await page.locator('[data-canvas-surface] .excalidraw:visible .excalidraw__canvas.interactive').click({position:{x:30,y:350}});
  await page.keyboard.press('Control+a');
  await page.getByRole('button',{name:'素材编辑',exact:true}).click();
  const palette=page.getByRole('dialog',{name:rasterMode?'图像素材编辑':'矢量素材编辑',exact:true});
  const svg=palette.getByRole('img',{name:'框选插图贴合范围',exact:true});
  await svg.waitFor();
  const submit=palette.getByRole('button',{name:'预览贴合提案',exact:true});
  assert.equal(await submit.isEnabled(),false,'no implicit range');
  await palette.getByRole('button',{name:'使用当前可见范围',exact:true}).click();
  await palette.getByRole('spinbutton',{name:'贴合范围左侧百分比',exact:true}).fill('95');
  assert.equal(await submit.isEnabled(),false,'out-of-root range never submits');
  const left=palette.getByRole('spinbutton',{name:'贴合范围左侧百分比',exact:true});
  await left.fill('');assert.equal(await left.inputValue(),'');assert.equal(await submit.isEnabled(),false,'blank range never submits');
  assert.ok(!diagnostics.console.some(message=>message.includes('NaN')),'clearing a range input never renders NaN');
  if(transformedFit){
   await left.fill('0');await palette.getByRole('spinbutton',{name:'贴合范围宽度百分比',exact:true}).fill('5');
   assert.equal(await submit.isEnabled(),false,'source-root rectangle outside visible crop is rejected');
   await palette.getByText('灰色区域已被画布裁剪，框选须位于虚线标示的可见区域内。',{exact:true}).waitFor();
  }

  const screen=await svg.evaluate((element,bounds)=>{const box=element.viewBox.baseVal,m=element.getScreenCTM();return [[bounds[0]*box.width,bounds[1]*box.height],[(bounds[0]+bounds[2])*box.width,(bounds[1]+bounds[3])*box.height]].map(([x,y])=>{const p=new DOMPoint(x,y).matrixTransform(m);return {x:p.x,y:p.y};});},bounds);
  await page.mouse.move(screen[0].x,screen[0].y);await page.mouse.down();await page.mouse.move(screen[1].x,screen[1].y,{steps:8});await page.mouse.up();
  assert.equal(await submit.isEnabled(),true,'actual pointer selection yields a valid range');
  if(editGeometry){
   await palette.getByRole('checkbox',{name:'调整插图位置和尺寸',exact:true}).check();
   const widthInput=palette.getByRole('spinbutton',{name:'插图几何宽度',exact:true});
   await widthInput.fill('');assert.equal(await submit.isEnabled(),false);
   await widthInput.fill('-1');assert.equal(await submit.isEnabled(),false);
   await widthInput.fill(String(displayImage.width));
   await palette.getByRole('spinbutton',{name:'插图几何高度',exact:true}).fill(String(displayImage.height));
   assert.equal(await submit.isEnabled(),true);
  }
  await page.screenshot({path:join(directory,'manual-range.png'),animations:'disabled'});
  const count=(await call('GET',`${path}/proposals`)).length;
  await submit.click();
  await palette.waitFor({state:'hidden'});
  const proposals=await call('GET',`${path}/proposals`);assert.equal(proposals.length,count+1);
  child=proposals.find(p=>p.summary===(editGeometry?'调整插图布局与连接范围':'调整插图连接范围'));assert.ok(child);
  assert.equal(child.parent_proposal_id??null,null,'manual range starts from the saved formal figure');
 }else{
  const childResult=(await call('POST','/test/visualization',{action:'revise_proposal',project_id:project,figure_id:figure,proposal_id:parent.id,document_sha256:parent.document_sha256,expected_revision:revision,title:'Connect receptor artwork',markup_ids:[],operations:[{op:'fit_illustration_anchor',id:image.id,file_id:image.fileId,normalized_bounds:bounds},...(scaleMode?[{op:'scale_illustration',id:image.id,file_id:image.fileId,factor:1.1}]:[]),...(process.env.VISUALIZATION_FIT_REROUTE==='1'?document.elements.filter(e=>e.type==='arrow'&&e.customData?.crossPanel&&(e.startBinding?.elementId===receptor.id||e.endBinding?.elementId===receptor.id)).map(e=>({op:'reroute',id:e.id})):[])]})).content;child=await call('GET',`${path}/proposals/${childResult.id}`);assert.equal(child.parent_proposal_id,parent.id);assert.equal(child.parent_document_sha256,parent.document_sha256);
 }

 const fitted=child.document.elements.find(e=>e.id===receptor.id);
 if(transformedFit){
  const actual=fitted.customData.visualAnchorFit.normalizedBounds;
  for(let i=0;i<4;i++)assert.ok(Math.abs(actual[i]-bounds[i])<1e-6,'pointer range uses original SVG coordinates');
  const c=displayImage.crop,crop=[c.x/c.naturalWidth,c.y/c.naturalHeight,c.width/c.naturalWidth,c.height/c.naturalHeight];
  const lx=((actual[0]+actual[2]/2-crop[0])/crop[2]-.5)*displayImage.width*displayImage.scale[0],ly=((actual[1]+actual[3]/2-crop[1])/crop[3]-.5)*displayImage.height*displayImage.scale[1];
  const cos=Math.cos(displayImage.angle),sin=Math.sin(displayImage.angle),cx=displayImage.x+displayImage.width/2+cos*lx-sin*ly,cy=displayImage.y+displayImage.height/2+sin*lx+cos*ly;
  const width=Math.abs(cos)*actual[2]/crop[2]*displayImage.width+Math.abs(sin)*actual[3]/crop[3]*displayImage.height,height=Math.abs(sin)*actual[2]/crop[2]*displayImage.width+Math.abs(cos)*actual[3]/crop[3]*displayImage.height;
  for(const [key,value]of Object.entries({x:cx-width/2,y:cy-height/2,width,height}))assert.ok(Math.abs(fitted[key]-value)<1e-6,`native transformed anchor ${key}`);
 }
 if(existingRaster){assert.ok(!displayImage.angle&&!displayImage.crop,'saved raster replay expects unrotated uncropped artwork');const expected={x:displayImage.x+bounds[0]*displayImage.width,y:displayImage.y+bounds[1]*displayImage.height,width:bounds[2]*displayImage.width,height:bounds[3]*displayImage.height};for(const [key,value]of Object.entries(expected))assert.ok(Math.abs(fitted[key]-value)<1e-6,`reviewed raster anchor ${key}`);}else assert.ok(fitted.width<receptor.width/2,'visible art fit contracts the wide placeholder');if(editGeometry){const actualImage=child.document.elements.find(e=>e.id===image.id),expectedImage={...displayImage};if(scaleMode)for(const key of ['x','y','width','height']){assert.ok(Number.isFinite(actualImage[key])&&Math.abs(actualImage[key]-expectedImage[key])<1e-9,`scaled image ${key}`);expectedImage[key]=actualImage[key];}for(const key of ['version','versionNonce','updated'])expectedImage[key]=actualImage[key];assert.deepEqual(actualImage,expectedImage);}else assert.deepEqual(child.document.elements.find(e=>e.id===image.id),image);assert.deepEqual(child.document.files,document.files);
 const related=document.elements.filter(e=>e.type==='arrow'&&(e.startBinding?.elementId===receptor.id||e.endBinding?.elementId===receptor.id));assert.equal(related.length,2);const changed=new Set([...(editGeometry?[image.id]:[]),receptor.id,...related.map(e=>e.id),...document.elements.filter(e=>e.type==='text'&&related.some(a=>a.id===e.containerId)).map(e=>e.id)]);
 if(existingRaster)for(const label of document.elements.filter(e=>e.type==='text'&&related.some(a=>a.id===e.containerId)))assert.deepEqual(child.document.elements.find(e=>e.id===label.id),label,'reviewed connector caption stays exact');
 for(const e of document.elements.filter(e=>!changed.has(e.id)))assert.deepEqual(child.document.elements.find(n=>n.id===e.id),e,`unrelated ${e.id} remains intact`);
 for(const e of related){const after=child.document.elements.find(n=>n.id===e.id);for(const key of ['startBinding','endBinding','startArrowhead','endArrowhead','customData','strokeColor','strokeWidth'])assert.deepEqual(after[key],e[key]);const start=e.startBinding.elementId===receptor.id,point=start?after.points[0]:after.points.at(-1),endpoint=[after.x+point[0],after.y+point[1]],gap=e[start?'startBinding':'endBinding'].gap;const distances=[Math.hypot(endpoint[0]-(fitted.x-gap),endpoint[1]-(fitted.y+fitted.height/2)),Math.hypot(endpoint[0]-(fitted.x+fitted.width+gap),endpoint[1]-(fitted.y+fitted.height/2)),Math.hypot(endpoint[0]-(fitted.x+fitted.width/2),endpoint[1]-(fitted.y-gap)),Math.hypot(endpoint[0]-(fitted.x+fitted.width/2),endpoint[1]-(fitted.y+fitted.height+gap))];assert.ok(Math.min(...distances)<1e-6,'actual endpoint matches the fitted illustration port');}
 // Arrowheads must face their actual components, not run tangentially beside them.
 for(const prior of related){const arrow=child.document.elements.find(e=>e.id===prior.id);for(const [key,start] of [['startBinding',true],['endBinding',false]]){const node=child.document.elements.find(e=>e.id===arrow[key].elementId),gap=arrow[key].gap,p=start?arrow.points[0]:arrow.points.at(-1),n=start?arrow.points[1]:arrow.points.at(-2),port=[arrow.x+p[0],arrow.y+p[1]],outside=[arrow.x+n[0],arrow.y+n[1]];const near=(a,b)=>Math.abs(a-b)<1e-6;assert.ok((near(port[0],node.x-gap)&&outside[0]<port[0]&&near(outside[1],port[1]))||(near(port[0],node.x+node.width+gap)&&outside[0]>port[0]&&near(outside[1],port[1]))||(near(port[1],node.y-gap)&&outside[1]<port[1]&&near(outside[0],port[0]))||(near(port[1],node.y+node.height+gap)&&outside[1]>port[1]&&near(outside[0],port[0])),`${arrow.id}/${key} arrowhead faces the component`);}}
 if(process.env.VISUALIZATION_FIT_REROUTE==='1'){
  const direction=(arrow,key)=>{const p=key==='startBinding'?arrow.points[0]:arrow.points.at(-1),x=arrow.x+p[0],y=arrow.y+p[1],gap=arrow[key].gap;const ports=[[fitted.x-gap,fitted.y+fitted.height/2],[fitted.x+fitted.width+gap,fitted.y+fitted.height/2],[fitted.x+fitted.width/2,fitted.y-gap],[fitted.x+fitted.width/2,fitted.y+fitted.height+gap]];return ports.findIndex(p=>Math.hypot(p[0]-x,p[1]-y)<1e-6);};
  const outgoing=child.document.elements.find(e=>e.type==='arrow'&&e.startBinding?.elementId===receptor.id),incoming=child.document.elements.find(e=>e.type==='arrow'&&e.endBinding?.elementId===receptor.id);
  assert.notEqual(direction(outgoing,'startBinding'),direction(incoming,'endBinding'),'prefer a free port instead of reversing over the ligand connection');
 }
 assert.deepEqual(await call('GET',path),before);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
 for(const [candidate,name] of [[parent,'before.png'],[child,'after.png']]){await call('POST',`${path}/proposals/${candidate.id}/preview-request`,{expected_revision:revision});let preview;await until(async()=>{preview=await call('GET',`${path}/proposals/${candidate.id}/preview?expected_revision=${revision}`);assert.notEqual(preview.render_status,'failed');return preview.render_status==='ready';},'independent candidate render');assert.equal(preview.document_sha256,candidate.document_sha256);await writeFile(join(directory,name),Buffer.from(preview.dataURL.split(',')[1],'base64'));}
 const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});if(manualFit)await dialog.waitFor();else await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();await dialog.getByRole('button',{name:`${child.summary} · v${revision}${manualFit?'':' · 修订'}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await page.screenshot({path:join(directory,'review.png'),animations:'disabled'});await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});const adopted=await call('GET',path);assert.equal(adopted.metadata.revision,revision+1);assert.deepEqual(adopted.scene.document,child.document);
 if(process.env.VISUALIZATION_FIT_REQUIRE_ARROWHEAD_SPAN==='1')for(const prior of related){
  const arrow=child.document.elements.find(e=>e.id===prior.id),points=arrow.points;
  for(const [head,a,b]of [[arrow.startArrowhead,points[0],points[1]],[arrow.endArrowhead,points.at(-1),points.at(-2)]])if(head==='arrow')assert.ok(Math.hypot(a[0]-b[0],a[1]-b[1])>=24,`${arrow.id}: visible native arrowhead terminal span`);
 }
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));await persist('source.json',exported);for(const e of child.document.elements){const restored=exported.elements.find(n=>n.id===e.id);for(const key of ['x','y','width','height','points','startBinding','endBinding','startArrowhead','endArrowhead','customData'])assert.deepEqual(restored[key],e[key],`${e.id}/${key} survives native restore`);}assert.deepEqual(exported.files,child.document.files);assert.deepEqual(errors,[]);
 if(fontSymbols){
  const svgDownload=page.waitForEvent('download');await page.getByRole('button',{name:'导出 SVG',exact:true}).click();
  const svg=await readFile(await(await svgDownload).path(),'utf8');assert.ok(svg.includes('①')&&svg.includes('②'));
  const svgPath=join(directory,'font-symbols.svg');await writeFile(svgPath,svg);
  const audit=spawn('python3',[fileURLToPath(new URL('../../../crates/visualization/scripts/check-exported-font.py',import.meta.url)),svgPath,'--require-chinese'],{stdio:['ignore','pipe','pipe']});
  let auditOutput='';audit.stdout.on('data',b=>auditOutput+=b);audit.stderr.on('data',b=>auditOutput+=b);
  const exit=await new Promise(resolve=>audit.once('exit',resolve));assert.equal(exit,0,auditOutput);await persist('font-symbols-audit.json',{passed:true,output:auditOutput});
 }
 await persist('proposal.json',child);await persist('final.json',adopted);await persist('result.json',{passed:true,model_used:false,test_driver:manualFit?'actual SVG pointer range selection':'explicit registered-tool geometry fixture',font_symbols:fontSymbols,manual_fit:manualFit,raster:rasterMode,illustration_geometry:editGeometry,center_fixed_scale:scaleMode,transformed_fit:transformedFit,rerouted:process.env.VISUALIZATION_FIT_REROUTE==='1',arrowhead_terminal_span_verified:process.env.VISUALIZATION_FIT_REQUIRE_ARROWHEAD_SPAN==='1',scope:'saved native scientific scene; explicit path bounds, real registered tool/HTTP/SQLite/GUI renderer/adoption/source restore'});console.log(`illustration-fit: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`illustration-fit: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
