import {rasterizeFixtureImage} from './helpers/canvas-raster-fixture.mjs';
// Native rendering/round-trip of explicit artwork bounds; no model decisions are simulated.
import assert from 'node:assert/strict';
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import {mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
const bioartMode=process.env.VISUALIZATION_LAYOUT_BIOART==='1';
const rasterMode=process.env.VISUALIZATION_LAYOUT_RASTER==='1';
const editGeometry=process.env.VISUALIZATION_FIT_GEOMETRY==='1';
const manualFit=process.env.VISUALIZATION_FIT_MANUAL==='1',transformedFit=process.env.VISUALIZATION_FIT_TRANSFORMED==='1';
const fontSymbols=process.env.VISUALIZATION_FIT_FONT_SYMBOLS==='1';
const source=process.env.VISUALIZATION_FIT_SCENE||fileURLToPath(new URL('./fixtures/scientific-egfr-mapk-illustrated.json',import.meta.url));
const binary=process.env.VISUALIZATION_NATIVE_BIN||fileURLToPath(new URL('../../../target/debug/examples/visualization_native',import.meta.url));
const directory=await mkdtemp(join(tmpdir(),'autonomics-illustration-label-layout-')),origin='http://127.0.0.1:1453';
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
 const input=JSON.parse(await readFile(source,'utf8'));let document=input.scene?.document??input.document??input;const receptor=document.elements.find(e=>e.customData?.scientificId==='egfr');assert.ok(receptor);let image=document.elements.find(e=>e.id===receptor.customData.visualAssetElementId);assert.ok(image);
 if(fontSymbols){const caption=document.elements.find(e=>e.type==='text'&&!e.containerId&&!e.frameId);assert.ok(caption);const text=caption.originalText??caption.text;caption.text=`① 范围：${text} ②`;caption.originalText=caption.text;}
 if(rasterMode)await rasterizeFixtureImage(page,document,image);
 if(process.env.VISUALIZATION_LABEL_COLLISION==='1'){const label=document.elements.find(e=>e.type==='text'&&receptor.customData.visualLabelIds?.includes(e.id));assert.ok(label);label.x=image.x-label.width/2;label.y=image.y;}
 const initial=await call('GET',path);await call('PUT',path,{expected_revision:initial.metadata.revision,title:'Artwork fit',document});
 if(bioartMode){
  const search=await call('POST','/test/bioart',{action:'search',query:'receptor'});const assets=search.content.assets;assert.ok(assets?.length,'real NIH receptor search has results');
  await persist('nih-search.json',assets);
  const asset=assets.find(a=>a.licenses?.length&&a.creators?.length);assert.ok(asset,'choose a catalogue item with explicit creator and license');
  const preview=await call('POST','/test/bioart',{action:'preview',asset_id:asset.id});assert.equal(preview.is_error??false,false);assert.ok(JSON.stringify(preview).includes('image/png'));await persist('nih-reviewed-asset.json',asset);
  const saved=await call('GET',path);
  const replacement=await call('POST','/test/bioart',{action:'replace',project_id:project,figure_id:figure,element_id:receptor.id,asset_id:asset.id,expected_revision:saved.metadata.revision,markup_ids:[]});
  const proposal=await call('GET',`${path}/proposals/${replacement.content.proposal_id}`);assert.deepEqual(await call('GET',path),saved,'real NIH replacement waits for adoption');
  await call('POST',`${path}/proposals/${proposal.id}/accept`);
  document=proposal.document;image=document.elements.find(e=>e.type==='image'&&e.customData?.semanticAnchorId===receptor.id);assert.ok(image);
  assert.equal(image.customData.asset.id,asset.id);assert.ok(document.files[image.fileId].dataURL.startsWith('data:image/png;base64,'));
  await persist('nih-replacement.json',proposal);
 }
 if(transformedFit){
  const saved=await call('GET',path),info=await call('GET',`${path}/image-info?element_id=${encodeURIComponent(image.id)}&expected_revision=${saved.metadata.revision}`);
  image.angle=.42;image.scale=[-1,-1];image.crop={x:info.natural_width*.1,y:0,width:info.natural_width*.8,height:info.natural_height,naturalWidth:info.natural_width,naturalHeight:info.natural_height};
  await call('PUT',path,{expected_revision:saved.metadata.revision,title:'Artwork fit',document});
 }
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();const before=await call('GET',path),revision=before.metadata.revision;
 const label=document.elements.find(e=>e.type==='text'&&receptor.customData.visualLabelIds?.includes(e.id));assert.ok(label);
 await page.locator('[data-canvas-surface] .excalidraw:visible .excalidraw__canvas.interactive').click({position:{x:30,y:350}});await page.keyboard.press('Control+a');
 await page.getByRole('button',{name:'素材编辑',exact:true}).click();
 const palette=page.getByRole('dialog',{name:rasterMode||bioartMode?'图像素材编辑':'矢量素材编辑',exact:true});
 if(rasterMode||bioartMode){assert.equal(await palette.getByRole('button',{name:'预览配色提案',exact:true}).count(),0);await palette.getByText('位图保留原始像素和来源，可调整绑定插图的布局、连接范围和标签。独立图片可直接使用画布变换控件。',{exact:true}).waitFor();}
 const layout=palette.getByRole('article',{name:`标签布局 ${label.id}`,exact:true});await layout.waitFor();
 const submit=layout.getByRole('button',{name:'预览标签布局提案',exact:true});assert.equal(await submit.isEnabled(),false);
 const width=layout.getByRole('spinbutton',{name:'标签换行宽度',exact:true});await width.fill('');assert.equal(await submit.isEnabled(),false);
 await width.fill(String(label.width*1.2));
 const font=layout.getByRole('spinbutton',{name:'标签字号',exact:true});await font.fill('0');assert.equal(await submit.isEnabled(),false);await font.fill('18');
 await layout.getByRole('spinbutton',{name:'标签X',exact:true}).fill(String(label.x+3));await layout.getByRole('spinbutton',{name:'标签Y',exact:true}).fill(String(label.y+2));
 await submit.click();await palette.waitFor({state:'hidden'});
 const candidates=(await call('GET',`${path}/proposals`)).filter(p=>p.status==='pending');assert.equal(candidates.length,1);const child=candidates[0];
 const actual=child.document.elements.find(e=>e.id===label.id);assert.ok(actual);
 assert.equal(actual.x,label.x+3);assert.equal(actual.y,label.y+2);assert.equal(actual.fontSize,18);assert.equal(actual.originalText,label.originalText??label.text);
 assert.equal(actual.text.replace(/\s/g,''),(label.originalText??label.text).replace(/\s/g,''));assert.ok(actual.width<=label.width*1.2+1e-6);
 const omit=(e,fields)=>Object.fromEntries(Object.entries(e).filter(([key])=>!fields.includes(key)));
 const changed=['x','y','width','height','text','originalText','fontSize','autoResize','version','versionNonce','updated'];
 assert.deepEqual(omit(actual,changed),omit(label,changed));
 for(const old of document.elements)if(old.id!==label.id)assert.deepEqual(child.document.elements.find(e=>e.id===old.id),old,`unrelated ${old.id}`);
 assert.deepEqual(child.document.files,document.files);assert.deepEqual(await call('GET',path),before);
 const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.waitFor();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 if(process.env.VISUALIZATION_LABEL_COLLISION==='1'){
  const collision=dialog.locator('details[aria-label="插图标签重叠检查"]');await collision.locator('summary').click();
  await collision.getByRole('button',{name:`定位插图标签 ${label.id}`,exact:true}).click();
  await until(async()=>(await dialog.locator('[data-preview-focus]').evaluateAll(nodes=>nodes.map(n=>n.dataset.previewFocus))).every(id=>id===label.id),'both previews focus conflicting label');
  assert.deepEqual(await call('GET',path),before,'review warnings do not adopt or change the scene');
 }
 await page.screenshot({path:join(directory,'label-review.png'),animations:'disabled'});
 await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
 const adopted=await call('GET',path);assert.equal(adopted.metadata.revision,revision+1);assert.deepEqual(adopted.scene.document,child.document);
 page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));
 assert.equal(exported.elements.find(e=>e.id===label.id).originalText,actual.originalText);assert.deepEqual(exported.files,child.document.files);if(bioartMode){const restored=exported.elements.find(e=>e.id===image.id);assert.deepEqual(restored.customData.asset,image.customData.asset);assert.ok(restored.customData.asset.source_url.startsWith('https://bioart.niaid.nih.gov/'));assert.ok(restored.customData.asset.creators.length&&restored.customData.asset.licenses.length);}assert.deepEqual(errors,[]);
 await persist('proposal.json',child);await persist('source.json',exported);await persist('result.json',{passed:true,model_used:false,raster:rasterMode,bioart:bioartMode,scope:'real HTTP/SQLite/native canvas label layout, exact scientific text and unrelated objects, invalid numeric inputs, GUI adoption and export'});console.log(`illustration-label-layout: passed; artifacts=${directory}`);
} catch(error){await persist('failure.json',{error:String(error),stack:error.stack,diagnostics,native_output:output,vite_output:viteOutput});if(page)await page.screenshot({path:join(directory,'failure.png'),animations:'disabled'}).catch(()=>{});console.error(`illustration-label-layout: failed; artifacts=${directory}`);throw error;}finally{if(browser)await browser.close();for(const p of [vite,native]){if(p.exitCode===null&&p.signalCode===null){p.kill();await new Promise(resolve=>p.once('exit',resolve));}}}
