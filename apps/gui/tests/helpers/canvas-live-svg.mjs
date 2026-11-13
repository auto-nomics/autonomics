import {readCandidatePreparedPrompt} from './canvas-prepared-context.mjs';
// Unscripted configured Agent: refine pending SVG through actual candidate feedback.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
import {previewMetadata} from './canvas-context-artwork.mjs';
import {createHash} from 'node:crypto';
const byId=(doc,id)=>doc.elements.find(e=>e.id===id);
const content=elements=>elements.map(e=>Object.fromEntries(Object.entries(e).filter(([k])=>!['index','version','versionNonce','updated'].includes(k)&&!(k==='lastCommittedPoint'&&e[k]===null))));
export async function runLiveBioartSvgAcquisition(options) {
  const {page,call,path,send,persist,directory,turns}=options;
  let formal=await call('GET',path);
  assert.equal(formal.scene.document.elements.length,0,'online acquisition starts from an empty figure');
  await call('PUT',path,{expected_revision:formal.metadata.revision,title:'Live figure',document:formal.scene.document});
  page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
  formal=await call('GET',path);const revision=formal.metadata.revision;
  const parent=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'Acquire editable BioArt SVG',operations:[]});
  await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
  await dialog.getByRole('button',{name:`${parent.summary} · v${revision}`,exact:true}).click();
  await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
  const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
  await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill('请为这个空画布准备两个相同的、可继续编辑内部路径的 NIH BioArt 通用线粒体 SVG 实例。先 bioart search Mitochondria，选择普通 Mitochondria，不选 Damaged Mitochondria，实际 preview 且明确 format:svg，核对图像、作者和许可；不要猜素材ID。如果没有可用 SVG 则明确说明，不用 PNG 代替。随后连续两次 bioart insert，均明确 format:svg；第一次 x=0,y=0，第二次 x=300,y=0，保留默认尺寸、比例、方向和所有来源信息。沿当前候选及每次返回的新叶候选ID/哈希串行继续，markup_ids:[]，不要 replace，不另起根候选，不增删文字，不赋予 semanticAnchorId/scientificId，不推断分子结构。最后读取候选和 PNG，确认两个独立图片，留待我审阅，不采用。');
  await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();
  await persist('svg-acquisition-feedback.json',{draft:await readCandidatePreparedPrompt(page)});
  const start=turns.length;await send(start+1);
  const calls=turns[start].calls.filter(c=>!c.is_error),inserts=calls.filter(c=>c.name==='bioart'&&c.input.action==='insert');
  assert.equal(inserts.length,2);assert.ok(calls.some(c=>c.name==='bioart'&&c.input.action==='search'));
  assert.equal(inserts[0].input.asset_id,inserts[1].input.asset_id);
  for(const c of inserts){assert.equal(c.input.format,'svg');assert.equal(c.input.expected_revision,revision);assert.deepEqual(c.input.markup_ids,[]);}
  assert.equal(inserts[0].input.proposal_id,parent.id);assert.equal(inserts[0].input.document_sha256,parent.document_sha256);
  const previews=calls.filter(c=>c.name==='bioart'&&c.input.action==='preview'&&c.input.asset_id===inserts[0].input.asset_id);assert.ok(previews.some(c=>c.input.format==='svg'));
  const all=await call('GET',`${path}/proposals`),child=currentCandidateLeaf(all,revision),images=child.document.elements;
  assert.equal(images.length,2);assert.notEqual(images[0].id,images[1].id);assert.equal(images[0].fileId,images[1].fileId);
  assert.equal(Object.keys(child.document.files).length,1);
  const insertionParent=all.find(p=>p.id===inserts[1].input.proposal_id);assert.ok(insertionParent);assert.equal(insertionParent.parent_proposal_id,parent.id);assert.equal(inserts[1].input.document_sha256,insertionParent.document_sha256);assert.equal(child.parent_proposal_id,insertionParent.id);
  const history=JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')),uses=new Map(history.flatMap(m=>m.content).filter(b=>b.type==='tool_use').map(b=>[b.id,b]));
  const metadata=history.flatMap(m=>m.content).filter(b=>b.type==='tool_result'&&!b.is_error&&uses.get(b.tool_use_id)?.name==='bioart'&&uses.get(b.tool_use_id)?.input.action==='preview').map(b=>previewMetadata(b.content));
  for(let i=0;i<images.length;i++){
    const image=images[i],file=child.document.files[image.fileId];assert.equal(image.type,'image');assert.equal(file.mimeType,'image/svg+xml');
    assert.equal(image.x,i*300);assert.equal(image.y,0);assert.equal(image.customData.asset.title,'Mitochondria');assert.equal(image.customData.source,'NIH BioArt');
    assert.ok(image.customData.asset.creators.length&&image.customData.asset.licenses.length);assert.ok(image.customData.source_sha256);
    assert.equal(image.customData.semanticAnchorId,undefined);assert.equal(image.customData.scientificId,undefined);
    assert.deepEqual(image.groupIds,[]);assert.equal(image.frameId,null);assert.deepEqual(image.scale,[1,1]);assert.equal(image.crop,null);
    const hash=createHash('sha256').update(Buffer.from(file.dataURL.split(',')[1],'base64')).digest('hex');assert.equal(hash,image.customData.sha256);
    const preview=metadata.find(v=>v.format==='svg'&&v.sha256===hash&&v.source_sha256===image.customData.source_sha256);assert.ok(preview,'actual SVG preview and inserted bytes match');
    const scale=Math.min(1,240/Math.max(preview.width,preview.height));assert.equal(image.width,preview.width*scale);assert.equal(image.height,preview.height*scale);
  }
  assert.ok(calls.some(c=>c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===child.id));
  assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
  await persist('svg-acquired-candidate.json',child);
  await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();
  const index=(await call('GET',`${path}/proposal-index?limit=20`)).items,matching=index.filter(p=>p.summary===child.summary&&p.base_revision===revision&&p.parent_proposal_id);
  const position=matching.findIndex(p=>p.id===child.id);assert.ok(position>=0,'exact acquisition candidate is present in the GUI index');
  await dialog.getByRole('button',{name:`${child.summary} · v${revision} · 修订`,exact:true}).nth(position).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
  await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
  const acquired=await call('GET',path);assert.equal(acquired.metadata.revision,revision+1);assert.deepEqual(acquired.scene.document,child.document);await persist('svg-acquired-formal.json',acquired);
  const refinement=await runLiveSvgRefinement({...options,source:undefined,bioart:true});
  const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出素材来源',exact:true}).click();
  const manifest=JSON.parse(await readFile(await(await download).path(),'utf8')),final=await call('GET',path);
  for(const image of final.scene.document.elements){const entry=manifest.assets.find(a=>a.element_id===image.id);assert.ok(entry);assert.equal(entry.hash_matches_recorded,true);assert.equal(entry.actual_sha256,image.customData.sha256);assert.deepEqual(entry.authors,image.customData.asset.creators);assert.deepEqual(entry.licenses,image.customData.asset.licenses);assert.ok(entry.source_urls.includes(image.customData.asset.source_url));}
  await persist('svg-online-source-manifest.json',manifest);
  return {...refinement,online_acquisition:true,source_fixture_imported:false,source_manifest_verified:true,acquired_asset_id:inserts[0].input.asset_id,scope:'same Agent acquires BioArt SVG online into an empty figure, user adopts, then same Agent recolors and accents a path; no scientific discovery claim'};
}
export async function runLiveSvgRefinement({page,call,path,send,persist,directory,turns,until,source,bioart=false}) {
  const head=await call('GET',path);
  const input=source?JSON.parse(await readFile(source,'utf8')):head,document=input.scene?.document??input.document??input;
  const anchor=document.elements.find(e=>e.customData?.scientificId==='egfr');
  const image=bioart?document.elements.filter(e=>e.type==='image'&&!e.isDeleted&&e.customData?.source==='NIH BioArt').at(-1):byId(document,anchor?.customData?.visualAssetElementId);assert.equal(image?.type,'image');
  if(bioart){assert.equal(document.files[image.fileId].mimeType,'image/svg+xml');assert.ok(image.customData.source_sha256);assert.ok(document.elements.some(e=>e.type==='image'&&e.id!==image.id),'explicit saved two-instance BioArt scene is required');}
  if(source){await call('PUT',path,{expected_revision:head.metadata.revision,title:'Live figure',document});
  page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();}
  const formal=await call('GET',path),revision=formal.metadata.revision;
  const originalPalette=await call('GET',`${path}/vector-palette?element_id=${image.id}&expected_revision=${revision}`);
  let colors=Object.fromEntries(originalPalette.colors.filter(c=>c!=='#228866').map(c=>[c,'#228866']));
  if(bioart){
    const originalParts=await call('GET',`${path}/vector-parts?element_id=${image.id}&expected_revision=${revision}`);
    const strokes=new Set(originalParts.parts.map(p=>p.stroke));
    const counts=new Map();
    for(const part of originalParts.parts)if(/^#[0-9a-f]{6}$/i.test(part.fill)&&!strokes.has(part.fill)&&part.fill!=='#228866')counts.set(part.fill,(counts.get(part.fill)??0)+1);
    const selected=[...counts].filter(([,count])=>count>=2).sort((a,b)=>b[1]-a[1])[0];
    assert.ok(selected,'BioArt example needs a repeated fill distinct from outlines for local accent verification');
    colors={[selected[0]]:'#228866'};
    await persist('svg-parent-color-selection.json',{colors,original_palette:originalPalette.colors,reason:'change one repeated fill; preserve other fills and outlines'});
  }
  assert.ok(Object.keys(colors).length);
  const parent=await call('POST',`${path}/vector-colors`,{element_id:image.id,expected_revision:revision,colors});
  await persist('svg-parent.json',parent);await persist('svg-formal-before.json',formal);
  await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
  await dialog.getByRole('button',{name:`${parent.summary} · v${revision}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
  const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
  await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(`请精化这份待审候选中的${bioart?' BioArt 通用线粒体 SVG（只修改已经配成绿色的图片实例，保留另一实例）':' EGFR 通用受体 SVG'}。先查看候选 PNG 和该候选素材的 palette、parts，核对实际颜色。第一步用 recolor 把候选中绿色 #228866 改成深蓝 #2457A7，创建子候选。第二步重新读取这份子候选的 parts，自主选一条当前可见且填充深蓝的路径，用 edit_parts 将这一条的填充改成橙色 #E89B38，留下局部视觉强调。不要把路径解释为特异分子域，不改几何、不替换素材、不增删文字。保留所有科学标签、关系与证据、其他对象以及来源许可；每次编辑沿用最新候选引用和 markup_ids:[]。最终重新读取配色及候选 PNG 核对，留待我审阅，不采用，不在正式图另建提案。`);
  await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();const composer=page.getByPlaceholder('输入消息…');await composer.waitFor();const draft=await readCandidatePreparedPrompt(page);assert.ok(draft.includes(parent.id)&&draft.includes(parent.document_sha256));await persist('svg-feedback.json',{draft});
  const start=turns.length;assert.deepEqual(await call('GET',path),formal);await send(start+1);
  const calls=turns[start].calls,mutations=calls.filter(c=>c.name==='bioicons'&&['recolor','edit_parts'].includes(c.input.action));
  assert.ok(mutations.some(c=>c.input.action==='recolor')&&mutations.some(c=>c.input.action==='edit_parts'),'real Agent performs both refinement stages');
  for(const c of mutations){assert.ok(c.input.proposal_id&&c.input.document_sha256);assert.deepEqual(c.input.markup_ids,[]);assert.equal(c.input.expected_revision,revision);}
  const first=mutations.find(c=>c.input.action==='recolor');assert.equal(first.input.proposal_id,parent.id);assert.equal(first.input.document_sha256,parent.document_sha256);
  assert.ok(calls.slice(0,calls.indexOf(first)).some(c=>c.name==='bioicons'&&c.input.action==='palette'&&c.input.proposal_id===parent.id));
  const local=mutations.find(c=>c.input.action==='edit_parts');assert.notEqual(local.input.proposal_id,parent.id);
  assert.equal(local.input.edits.length,1,'only one graphical path is accented');assert.equal(local.input.edits[0].fill.toLowerCase(),'#e89b38');
  for(const key of ['stroke','opacity','translate','points'])assert.ok(local.input.edits[0][key]==null,`unrequested ${key} remains unchanged`);
  assert.ok(calls.slice(calls.indexOf(first)+1,calls.indexOf(local)).some(c=>c.name==='bioicons'&&c.input.action==='parts'&&c.input.proposal_id===local.input.proposal_id&&c.input.document_sha256===local.input.document_sha256),'parts refreshed from the recolored candidate');
  const child=currentCandidateLeaf(await call('GET',`${path}/proposals`),revision);await persist('svg-child.json',child);assert.notEqual(child.id,parent.id);
  const finalImage=byId(child.document,image.id);assert.notEqual(finalImage.fileId,image.fileId);
  for(const e of parent.document.elements.filter(e=>e.id!==image.id))assert.deepEqual(byId(child.document,e.id),e,`unrelated ${e.id} preserved`);
  assert.equal(child.document.elements.length,parent.document.elements.length);
  for(const key of ['x','y','width','height','angle','crop','frameId'])assert.deepEqual(finalImage[key],image[key]);
  for(const [id,file] of Object.entries(parent.document.files))assert.deepEqual(child.document.files[id],file);
  for(const [key,value] of Object.entries(byId(parent.document,image.id).customData))if(!['modifications','sha256','colors'].includes(key))assert.deepEqual(finalImage.customData[key],value);
  const query=`element_id=${image.id}&expected_revision=${revision}&proposal_id=${child.id}&document_sha256=${child.document_sha256}`;
  const palette=await call('GET',`${path}/vector-palette?${query}`);assert.ok(palette.colors.includes('#e89b38'));assert.ok(palette.colors.includes('#2457a7'));assert.ok(!palette.colors.includes('#228866'));
  assert.ok(calls.some(c=>c.name==='bioicons'&&c.input.action==='palette'&&c.input.proposal_id===child.id),'Agent rechecks final palette');
  assert.ok(calls.some(c=>c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===child.id),'Agent sees final PNG');
  assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
  let png;await until(async()=>{png=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'final SVG candidate PNG');assert.equal(png.document_sha256,child.document_sha256);await writeFile(join(directory,'svg-candidate.png'),Buffer.from(png.dataURL.split(',')[1],'base64'));
  await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();await dialog.getByRole('button',{name:`${child.summary} · v${revision} · 修订`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await page.screenshot({path:join(directory,'svg-review.png'),animations:'disabled'});
  await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});const final=await call('GET',path);assert.equal(final.metadata.revision,revision+1);assert.deepEqual(final.scene.document,child.document);
  const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));await persist('svg-source.json',exported);assert.deepEqual(content(exported.elements),content(child.document.elements));assert.deepEqual(exported.files,child.document.files);await persist('final.json',final);
  return {asset_source:bioart?'NIH BioArt':'BioIcons',target_element_id:image.id,parent_proposal_id:parent.id,child_proposal_id:child.id,scope:'one configured Agent recolors a pending SVG then accents one path; existing declared scientific content preserved, no scientific discovery claim'};
}
