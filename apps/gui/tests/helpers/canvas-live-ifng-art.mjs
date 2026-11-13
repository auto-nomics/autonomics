import {readCandidatePreparedPrompt} from './canvas-prepared-context.mjs';
import {checkCandidateGeometryReviews} from "./canvas-scene-geometry-evidence.mjs";
import {illustrationConnectorLabelOverlaps} from "../../src/features/canvas/canvas-connector-label-overlaps.ts";
import {checkIllustratedScene} from './canvas-illustrated-scene-contract.mjs';
import {reviewedFit} from './canvas-reviewed-fit.mjs';
import {checkIllustrationResize,reviewedResizeCall} from './canvas-illustration-resize-contract.mjs';
// Actual Agent illustrates a previously reviewed source-backed IFNG figure.
// Geometry changes are local to the target, attached connector/labels and notes.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
import {checkPortableSceneExport} from './canvas-live-source-review.mjs';
const byId=(doc,id)=>doc.elements.find(e=>e.id===id);
export async function runLiveIfngArt({page,call,path,send,persist,directory,turns,until,source,targetId,noteIds,targetName='IFNGR'}) {
 const assetProvider=process.env.VISUALIZATION_LIVE_ART_PROVIDER??'bioicons';
 assert.ok(['bioicons','bioart'].includes(assetProvider));
 const nih=assetProvider==='bioart';
 const translateInstruction=process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_TRANSLATE==='1'?'visualization_translate 工具（必须使用此入口）':'translate 操作或 visualization_translate 工具';
 const fitInstruction=process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_FIT==='1'?'visualization_fit（必须使用此入口，x/y/width/height 为四个数值）':'visualization_fit（四个必填数值 x/y/width/height）或 fit_illustration_anchor';
 const resizeContinuation=process.env.VISUALIZATION_LIVE_ILLUSTRATION_RESIZE==='1';
 assert.ok(!resizeContinuation||process.env.VISUALIZATION_LIVE_IFNG_CONTINUATION==='1');
 const startTurn=turns.length;
 const head=await call('GET',path);
 const input=source?JSON.parse(await readFile(source,'utf8')):head;
 const document=input.scene?.document??input.document??input;
 const target=targetId?byId(document,targetId):document.elements.find(e=>e.customData?.scientificId==='n_receptor_complex');
 assert.ok(target);assert.ok(!document.elements.some(e=>e.type==='image'));
 const notes=[...(noteIds??['note_scope_heading','note_scope_body']),'generic-receptor-note'];
 assert.ok(notes.length>=2&&new Set(notes).size===notes.length);
 for(const id of notes.slice(0,-1))assert.ok(byId(document,id)?.type==='text');
 if(source){
  await call('PUT',path,{expected_revision:head.metadata.revision,title:'Live figure',document});
  page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
 }
 const formal=await call('GET',path),revision=formal.metadata.revision;
 assert.deepEqual(formal.scene.document,document);
 const note=`Artwork is a generic membrane receptor illustration, not an ${targetName}-specific molecular structure.`;
 const bottom=Math.max(...document.elements.filter(e=>!e.isDeleted).map(e=>e.y+e.height));
 const parent=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'Illustrate reviewed IFN-γ figure',operations:[{op:'add_wrapped_text',id:'generic-receptor-note',text:note,x:0,y:bottom+36,max_width:1180,font_size:16}]});
 await persist('ifng-art-formal.json',formal);await persist('ifng-art-parent.json',parent);
 await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.getByRole('button',{name:`${parent.summary} · v${revision}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
 const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
 await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(`请在当前待审候选上，把受体复合物节点 ${target.id} 的占位外观替换成一份 ${nih?'NIH BioArt 通用膜受体 PNG':'BioIcons 通用膜受体 SVG'}。先检索、预览同一素材，核对作者和许可；图标不代表 ${targetName} 特异结构，保留已有通用图示说明。使用 ${assetProvider} replace，延续本候选的 proposal_id/document_sha256 和 markup_ids:[]。替换后${nih?'读取新候选与候选 PNG，结合已预览的原始 PNG 尺寸和可见轮廓，明确选择源图归一化连接范围；不要调用 SVG parts、palette 或 geometry':'读取新候选的 parts 及 PNG，按实际 SVG 几何范围'}，用 ${fitInstruction} 贴合 ${target.id}，让它相连的箭头靠近可见插图；不要猜测生物域，不改科学标签或引用。注意替换可能把原标签放在插图旁边或下方，先读取候选核对其实际位置。可仅调整受体自有标签的位置与换行宽度以避开连线和其他文字，完整原文、字号、字体和引用不变；本轮还必须将 ${notes.join("、")} 这些说明整体等距向下移动至少 24 个画布单位，以增加图形与说明间距；即使原本没有重叠也执行这项明确的布局要求。使用 ${translateInstruction}（element_ids 为这些现有 ID，dx:0、dy:数值）整体等距向下移动，保留原 ID，绝不能 duplicate 再 remove。文字、字号、字体和宽度不变。其他节点与连线不移动；贴合仅允许调整该受体的连接端点及其关系标签，保留端点身份、方向和依据。不要重建草图或另起根候选，不采用父候选。读取最终子候选 PNG，确认标签、插图、连线与说明无碰撞后留待我审阅，不采用。如果找不到合适素材，明确说明，不捏造来源。`);
 await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();await page.getByPlaceholder('输入消息…').waitFor();await persist('ifng-art-feedback.json',{draft:await readCandidatePreparedPrompt(page)});await send(startTurn+1);
 const all=await call('GET',`${path}/proposals`);await persist('ifng-art-all-candidates.json',all);let child=currentCandidateLeaf(all,revision);await persist('ifng-art-child.json',child);const earlyPng=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);if(earlyPng.render_status==='ready')await writeFile(join(directory,'ifng-art-candidate.png'),Buffer.from(earlyPng.dataURL.split(',')[1],'base64'));let cursor=child;const seen=new Set();while(cursor.id!==parent.id){assert.ok(!seen.has(cursor.id));seen.add(cursor.id);cursor=all.find(p=>p.id===cursor.parent_proposal_id);assert.ok(cursor,'all edits continue from the reviewed source figure');}
 if(process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_TRANSLATE==='1')assert.ok(turns[startTurn].calls.some(c=>!c.is_error&&c.name==='visualization_translate'),'scalar translation evaluation requires the dedicated entry');
 if(process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_FIT==='1')assert.ok(turns[startTurn].calls.some(c=>!c.is_error&&c.name==='visualization_fit'),'scalar-fit evaluation requires a successful call to the new entry');
 const calls=turns[startTurn].calls.filter(c=>!c.is_error),replacements=calls.filter(c=>c.name===assetProvider&&c.input.action==='replace');assert.ok(replacements.length);const replace=replacements.at(-1);for(const replacement of replacements){const base=all.find(p=>p.id===replacement.input.proposal_id);assert.ok(base&&(base.id===parent.id||seen.has(base.id)));assert.equal(replacement.input.document_sha256,base.document_sha256);assert.equal(replacement.input.element_id,target.id);assert.deepEqual(replacement.input.markup_ids??[],[]);const earlier=calls.slice(0,calls.indexOf(replacement));assert.ok(earlier.some(c=>c.name===assetProvider&&c.input.action==='search'));assert.ok(earlier.some(c=>c.name===assetProvider&&c.input.action==='preview'&&c.input.asset_id===replacement.input.asset_id));}if(!nih)assert.ok(calls.some(c=>c.name==='bioicons'&&c.input.action==='parts'&&seen.has(c.input.proposal_id)));else assert.ok(!calls.some(c=>c.name==='bioicons'&&['parts','palette','geometry'].includes(c.input.action)),'PNG uses no SVG-only inspection');
 assert.ok(calls.some(c=>seen.has(c.input.proposal_id)&&c.input.project_id===formal.metadata.project_id&&c.input.figure_id===formal.metadata.id&&c.input.expected_revision===revision&&reviewedFit(c,all,target.id)),'reviewed source geometry is fitted on the exact owned component');
 const image=checkIllustratedScene({child,parent,target,notes,assetProvider,assetId:replace.input.asset_id});
 assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);assert.ok(calls.some(c=>c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===child.id));
 if(process.env.VISUALIZATION_LIVE_IFNG_CONTINUATION==='1') {
  const previous=child;
  await persist('ifng-art-first-child.json',previous);
  await page.getByRole('tab',{name:'Live figure',exact:true}).click();
  await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();
  const index=(await call('GET',`${path}/proposal-index?limit=20`)).items;
  const matches=index.filter(p=>p.summary===previous.summary&&p.base_revision===revision&&p.parent_proposal_id);
  await dialog.getByRole('button',{name:`${previous.summary} · v${revision} · 修订`,exact:true}).nth(matches.findIndex(p=>p.id===previous.id)).click();
  await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
  if(!await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).isVisible())await feedback.locator('summary').click();
  await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(resizeContinuation ? `保留当前科学内容、素材文件、标签和说明文字。请以当前受体插图 ${image.id} 的中心不变，将图片宽度和高度各增加 10%。先读取当前候选，${process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_SCALE==='1'?'使用 visualization_scale_illustration，element_id 为当前插图或锚点、file_id 为当前图片文件，factor 使用 JSON 数值 1.1，复用当前已审阅贴合范围。':'使用 visualization revise_proposal 的 set_illustration_geometry 操作，id 为当前插图或锚点、file_id 为当前图片文件，x/y/width/height 使用数值；normalized_bounds 必须沿用当前锚点 visualAnchorFit 中已经审阅的范围，不扩大到整个源文件。'}不要更改裁剪、旋转、镜像或颜色；连接端点与关系标签由操作同步，其他对象（包括插图所属标签和全部说明）保持原样。不采用父候选，不创建新根候选。读取最终候选 PNG 后留待审阅。` : `保留刚才的受体插图、贴合和所有科学内容。请把 ${notes.join('、')} 这些现有说明再整体向下移动 24 个画布单位，横坐标不变，文字和样式不变。使用 visualization_translate，在当前候选上继续创建子候选，不复制、不重建、不修改其他对象，不采用。读取新的候选 PNG 后留待我审阅。`);
  await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();
  await page.getByPlaceholder('输入消息…').waitFor();
  await persist('ifng-art-continuation-feedback.json',{draft:await readCandidatePreparedPrompt(page)});
  await send(startTurn+2);
  if(resizeContinuation&&process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_SCALE==='1')assert.ok(turns.at(-1).calls.some(c=>c.name==='visualization_scale_illustration'&&!c.is_error),'registered scalar scale must succeed');
  const continued=await call('GET',`${path}/proposals`);
  await persist('ifng-art-continuation-candidates.json',continued);
  child=currentCandidateLeaf(continued,revision);
  assert.notEqual(child.id,previous.id);
  let ancestor=child;const lineage=new Set();
  while(ancestor.id!==previous.id){assert.ok(!lineage.has(ancestor.id));lineage.add(ancestor.id);ancestor=continued.find(p=>p.id===ancestor.parent_proposal_id);assert.ok(ancestor,'second turn must continue the first-turn candidate');}
  assert.deepEqual(child.review_markups,previous.review_markups);
  assert.deepEqual(child.document.files,previous.document.files,'second turn preserves exact illustration bytes');
  assert.equal(child.document.elements.length,previous.document.elements.length);
  if(resizeContinuation) checkIllustrationResize(previous.document,child.document,target.id);
  else {
  for(const old of previous.document.elements){const actual=byId(child.document,old.id);assert.ok(actual);if(notes.includes(old.id)){assert.equal(actual.y,old.y+24);const rest=Object.fromEntries(Object.entries(actual).filter(([k])=>!['y','version','versionNonce','updated'].includes(k)));const expected=Object.fromEntries(Object.entries(old).filter(([k])=>!['y','version','versionNonce','updated'].includes(k)));assert.deepEqual(rest,expected);}else assert.deepEqual(actual,old,`second turn preserves ${old.id}`);}
  }
  assert.deepEqual(await call('GET',path),formal);
  assert.deepEqual(await call('GET',`${path}/proposals/${previous.id}`),previous);
  if(resizeContinuation) assert.ok(reviewedResizeCall(turns[startTurn+1].calls,previous,target.id,{projectId:formal.metadata.project_id,figureId:formal.metadata.id}),'resize uses reviewed current candidate, owned file and exact scope');
  else assert.ok(turns[startTurn+1].calls.some(c=>!c.is_error&&c.name==='visualization_translate'&&c.input.proposal_id===previous.id&&c.input.document_sha256===previous.document_sha256),'second turn uses the current candidate hash');
  assert.ok(turns[startTurn+1].calls.some(c=>!c.is_error&&c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===child.id));
 }
 if(process.env.VISUALIZATION_LIVE_REQUIRE_LABEL_CLEARANCE==='1'){
  const clearance=illustrationConnectorLabelOverlaps(child.document);await persist('ifng-art-connector-label-clearance.json',clearance);
  assert.ok(clearance.checked>0,'requested caption clearance checks actual owned connector/label pairs');assert.equal(clearance.skipped,0,'requested caption clearance has supported geometry');assert.equal(clearance.omitted,0,'requested caption clearance covers all owned connector/label pairs');
  assert.equal(clearance.count,0,'requested illustration label clearance excludes connector centerline crossings');
 }
 if(process.env.VISUALIZATION_LIVE_REQUIRE_GEOMETRY_REVIEW==='1'){const reviews=checkCandidateGeometryReviews(JSON.parse(await readFile(join(directory,'session-history.json'),'utf8')),child,{projectId:formal.metadata.project_id});await persist('ifng-art-scene-geometry-review.json',reviews);}
 let png;await until(async()=>{png=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'source-backed IFNG artwork PNG');assert.equal(png.document_sha256,child.document_sha256);await writeFile(join(directory,'ifng-art-candidate.png'),Buffer.from(png.dataURL.split(',')[1],'base64'));await persist('ifng-art-child.json',child);
 await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const items=(await call('GET',`${path}/proposal-index?limit=20`)).items,matching=items.filter(p=>p.summary===child.summary&&p.base_revision===revision&&p.parent_proposal_id);await dialog.getByRole('button',{name:`${child.summary} · v${revision} · 修订`,exact:true}).nth(matching.findIndex(p=>p.id===child.id)).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await page.screenshot({path:join(directory,'ifng-art-review.png'),animations:'disabled'});await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});const adopted=await call('GET',path);assert.equal(adopted.metadata.revision,revision+1);assert.deepEqual(adopted.scene.document,child.document);
 const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));await persist('ifng-art-source.json',exported);await persist('final.json',adopted);checkPortableSceneExport(exported,child.document);return{full_scene_geometry_review_verified:process.env.VISUALIZATION_LIVE_REQUIRE_GEOMETRY_REVIEW==='1',asset_provider:assetProvider,label_clearance_required:process.env.VISUALIZATION_LIVE_REQUIRE_LABEL_CLEARANCE==='1',scalar_scale_required:process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_SCALE==='1',scalar_scale_used:turns[startTurn+1]?.calls.some(c=>!c.is_error&&c.name==='visualization_scale_illustration')??false,illustration_resize_verified:resizeContinuation,continuation_required:process.env.VISUALIZATION_LIVE_IFNG_CONTINUATION==='1',continuation_verified:process.env.VISUALIZATION_LIVE_IFNG_CONTINUATION==='1'&&turns.length===startTurn+2,scalar_translate_required:process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_TRANSLATE==='1',scalar_translate_used:calls.some(c=>c.name==='visualization_translate'),scalar_fit_required:process.env.VISUALIZATION_LIVE_REQUIRE_SCALAR_FIT==='1',scalar_fit_used:calls.some(c=>c.name==='visualization_fit'),asset_id:image.customData.asset.id,asset_author:nih?image.customData.asset.creators:image.customData.asset.author,asset_license:nih?image.customData.asset.licenses:image.customData.asset.license,scope:source?'one generic receptor illustration on a supplied source-reviewed IFNG figure; graph/evidence preserved, local fit and caption clearance reviewed; artwork does not verify molecular structure':'generic receptor illustration on the same-session source-backed sketch; graph/evidence preserved; artwork does not verify molecular structure'};
}
