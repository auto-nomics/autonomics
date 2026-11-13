import {readCandidatePreparedPrompt} from './canvas-prepared-context.mjs';
// Explicit reviewer-directed routing correction on a saved scientific figure.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
import {checkPortableSceneExport} from './canvas-live-source-review.mjs';
const omit=(value,keys)=>Object.fromEntries(Object.entries(value).filter(([key])=>!keys.includes(key)));
const bookkeeping=['version','versionNonce','updated'];
export function crossesBox(a,b,box){
  if(Math.hypot(b[0]-a[0],b[1]-a[1])<1e-6)return false;
  let low=0,high=1;
  for(let axis=0;axis<2;axis++){
    const start=axis===0?box.x:box.y,size=axis===0?box.width:box.height;
    const min=start+1e-6,max=start+size-1e-6,delta=b[axis]-a[axis];
    if(Math.abs(delta)<1e-12){if(a[axis]<min||a[axis]>max)return false;continue;}
    const t1=(min-a[axis])/delta,t2=(max-a[axis])/delta;
    low=Math.max(low,Math.min(t1,t2));high=Math.min(high,Math.max(t1,t2));
    if(high<=low)return false;
  }
  return high-low>1e-9;
}
export function crossedNodes(document,arrowId){
  const arrow=document.elements.find(e=>e.id===arrowId&&!e.isDeleted);assert.ok(arrow?.type==='arrow');
  assert.ok([arrow.x,arrow.y,arrow.width,arrow.height].every(Number.isFinite)&&arrow.width>=0&&arrow.height>=0);
  assert.equal(arrow.angle??0,0);assert.equal(arrow.roundness??null,null);assert.ok(!arrow.elbowed);
  assert.ok(Array.isArray(arrow.points)&&arrow.points.length>=2&&arrow.points.length<=1000);
  const points=arrow.points.map(p=>{assert.ok(p.length===2&&p.every(Number.isFinite));return[p[0]+arrow.x,p[1]+arrow.y];});
  const endpoints=[arrow.startBinding?.elementId,arrow.endBinding?.elementId];
  return document.elements.filter(e=>!e.isDeleted&&typeof e.customData?.scientificId==='string'&&!endpoints.includes(e.id)).filter(e=>{
    assert.equal(e.angle??0,0);assert.ok([e.x,e.y,e.width,e.height].every(Number.isFinite)&&e.width>0&&e.height>0);
    return points.slice(1).some((p,i)=>crossesBox(points[i],p,e));
  }).map(e=>e.id);
}
export function checkRouteReview(previous,next,arrowId){
  assert.deepEqual(omit(next,['elements']),omit(previous,['elements']));
  assert.equal(next.elements.length,previous.elements.length);
  assert.equal(new Set(next.elements.map(e=>e.id)).size,next.elements.length);
  const labels=new Set(previous.elements.filter(e=>e.type==='text'&&e.containerId===arrowId).map(e=>e.id));
  for(const old of previous.elements){
    const actual=next.elements.find(e=>e.id===old.id);assert.ok(actual);
    const allowed=old.id===arrowId?[...bookkeeping,'x','y','width','height','points']:labels.has(old.id)?[...bookkeeping,'x','y']:[];
    assert.deepEqual(omit(actual,allowed),omit(old,allowed),`only reviewed arrow and caption position may change: ${old.id}`);
  }
  assert.notDeepEqual(next.elements.find(e=>e.id===arrowId).points,previous.elements.find(e=>e.id===arrowId).points);
  assert.deepEqual(crossedNodes(next,arrowId),[],'reviewed connector avoids all non-endpoint native scientific node boxes');
  const boxes=previous.elements.filter(e=>!e.isDeleted&&['rectangle','ellipse','diamond','text','image','frame'].includes(e.type));
  for(const box of boxes)assert.ok([box.x,box.y,box.width,box.height].every(Number.isFinite)&&(box.angle??0)===0);
  const bounds=[Math.min(...boxes.map(e=>e.x)),Math.min(...boxes.map(e=>e.y)),Math.max(...boxes.map(e=>e.x+e.width)),Math.max(...boxes.map(e=>e.y+e.height))];
  const arrow=next.elements.find(e=>e.id===arrowId);
  for(const point of arrow.points){const x=arrow.x+point[0],y=arrow.y+point[1];assert.ok(x>=bounds[0]-1e-6&&y>=bounds[1]-1e-6&&x<=bounds[2]+1e-6&&y<=bounds[3]+1e-6,'route stays inside the existing figure footprint instead of an outer loop');}
}

export async function runLiveRouteReview({page,call,path,send,persist,directory,turns,until,source}){
  const input=JSON.parse(await readFile(source,'utf8')),document=input.scene?.document??input.document??input;
  const arrowId='edge-4',obstacleId='node-3';assert.ok(crossedNodes(document,arrowId).includes(obstacleId),'explicit saved RAS→RAF crossing case');
  const head=await call('GET',path);await call('PUT',path,{expected_revision:head.metadata.revision,title:'Live figure',document});
  page.once('dialog',d=>d.accept());await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const formal=await call('GET',path),revision=formal.metadata.revision;assert.deepEqual(formal.scene.document,document);await persist('route-review-before.json',formal);
  const created=await call('POST',`${path}/proposals`,{expected_revision:revision,summary:'Review one cross-panel connector',operations:[]});
  const parent=await call('GET',`${path}/proposals/${created.id}`);await persist('route-review-parent.json',parent);
  await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});await dialog.getByRole('button',{name:`${parent.summary} · v${revision}`,exact:true}).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();
  const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();
  const request=`明确审阅意见：RAS→RAF 的跨分面连线 ${arrowId} 目前穿过 GRB2–SOS1 节点 ${obstacleId}。先读取当前候选和 PNG，仅使用 reroute 操作重新布置这条已生成的跨分面连线，使其避开非端点节点，保持在现有整图外框内，不绕到整幅图的外侧；允许它的原有绑定说明随路径移动，完整文字/字号/宽高不变。端点身份、箭头方向、证据、样式、其他对象、图片/文件、分面、历史计划和所有科学内容保持原样。不加标题/图注，不重建或采用。沿单一父子候选链修订，最后实际查看 PNG。`;
  await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(request);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();
  let draft;await until(async()=>{draft=await readCandidatePreparedPrompt(page);return draft.includes(parent.id)&&draft.includes(parent.document_sha256);},'route review draft scope');await persist('route-review-feedback.json',{request,draft});await send(1);
  const all=await call('GET',`${path}/proposals`);await persist('route-review-candidates.json',all);const child=currentCandidateLeaf(all,revision);assert.notEqual(child.id,parent.id);checkRouteReview(parent.document,child.document,arrowId);await persist('route-review-child.json',child);
  assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
  const calls=turns[0].calls.filter(c=>!c.is_error&&c.name==='visualization');assert.ok(calls.some(c=>c.input.operations?.some(op=>op.op==='reroute'&&op.id===arrowId)));for(const id of [parent.id,child.id])assert.ok(calls.some(c=>c.input.action==='read_proposal_preview'&&c.input.proposal_id===id));
  let png;await until(async()=>{png=await call('GET',`${path}/proposals/${child.id}/preview?expected_revision=${revision}`);assert.notEqual(png.render_status,'failed');return png.render_status==='ready';},'route review PNG');assert.equal(png.document_sha256,child.document_sha256);await writeFile(join(directory,'route-review-candidate.png'),Buffer.from(png.dataURL.split(',')[1],'base64'));
  await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const items=(await call('GET',`${path}/proposal-index?limit=100`)).items,matching=items.filter(p=>p.summary===child.summary&&p.base_revision===revision&&p.parent_proposal_id),index=matching.findIndex(p=>p.id===child.id);assert.ok(index>=0);await dialog.getByRole('button',{name:`${child.summary} · v${revision} · 修订`,exact:true}).nth(index).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await page.screenshot({path:join(directory,'route-review-adoption.png'),animations:'disabled'});await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
  const final=await call('GET',path);await persist('final.json',final);assert.equal(final.metadata.revision,revision+1);assert.deepEqual(final.scene.document,child.document);
  const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const exported=JSON.parse(await readFile(await(await download).path(),'utf8'));await persist('route-review-source.json',exported);checkPortableSceneExport(exported,child.document);
  return{within_original_figure_footprint_verified:true,nonendpoint_node_boxes_clear:true,scope:'explicit reviewer-directed correction of a known connector/node-box crossing on a saved EGFR figure within its original footprint; all other objects/files/scientific metadata preserved; not autonomous layout review or scientific correctness',reviewed_arrow_id:arrowId};
}
