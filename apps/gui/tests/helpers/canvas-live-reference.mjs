import {readCanvasPreparedContext} from './canvas-prepared-context.mjs';
// Real GUI image upload, selection, vision tool use and native reconstruction.
// The synthetic picture is test input, not a model response or a scientific claim.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {currentCandidateLeaf} from './canvas-live-scientific.mjs';
import {checkPortableSceneExport} from './canvas-live-source-review.mjs';

export function checkReferenceScene(original,candidate) {
  assert.equal(new Set(candidate.elements.map(e=>e.id)).size,candidate.elements.length,'unique native identities');
  const settings=document=>Object.fromEntries(Object.entries(document).filter(([key])=>key!=='elements'&&key!=='files'));
  assert.deepEqual(settings(candidate),settings(original),'existing scene settings unchanged');
  const source=original.elements.find(e=>e.type==='image'&&!e.isDeleted);
  assert.ok(source,'one uploaded reference');
  assert.deepEqual(candidate.elements.find(e=>e.id===source.id),source,'reference object unchanged');
  assert.deepEqual(candidate.files,original.files,'reference bytes unchanged; no raster replacement');
  const originalIds=new Set(original.elements.map(e=>e.id));
  for(const element of original.elements)assert.deepEqual(candidate.elements.find(e=>e.id===element.id),element);
  const added=candidate.elements.filter(e=>!originalIds.has(e.id)&&!e.isDeleted);
  for(const element of added){
    for(const key of ['x','y','width','height'])assert.ok(Number.isFinite(element[key]),`finite ${element.id}.${key}`);
    assert.ok(element.opacity>0,'visible native object');
    if(element.type==='arrow')assert.ok(element.width>=0&&element.height>=0&&element.width+element.height>0,'nonzero native connector');
    else assert.ok(element.width>0&&element.height>0,'positive native node/text dimensions');
  }
  const labels=added.filter(e=>e.type==='text');
  assert.deepEqual(labels.map(e=>(e.originalText??e.text).trim()).sort(),['Collect','Image','Review']);
  const names=new Map();
  for(const label of labels){
    const node=added.find(e=>e.id===label.containerId);
    assert.ok(node&&['rectangle','ellipse','diamond'].includes(node.type),'each label belongs to an editable native shape');
    assert.ok(!names.has(node.id),'different native node per label');
    assert.ok(node.boundElements?.some(e=>e.id===label.id&&e.type==='text'),'reciprocal native text binding');
    assert.equal(label.text.trim(),(label.originalText??label.text).trim(),'visible label equals complete reference label');
    assert.ok(label.fontSize>=16,'readable native text');
    assert.ok(node.y>=source.y+source.height+20,'new diagram below preserved reference');
    names.set(node.id,(label.originalText??label.text).trim());
  }
  const arrows=added.filter(e=>e.type==='arrow');
  assert.equal(arrows.length,2);
  assert.deepEqual(arrows.map(e=>`${names.get(e.startBinding?.elementId)}>${names.get(e.endBinding?.elementId)}`).sort(),['Collect>Image','Image>Review']);
  assert.ok(arrows.every(e=>e.startArrowhead===null&&e.endArrowhead==='arrow'),'original forward direction');
  for(const arrow of arrows)for(const binding of [arrow.startBinding,arrow.endBinding])assert.ok(added.find(e=>e.id===binding.elementId)?.boundElements?.some(e=>e.id===arrow.id&&e.type==='arrow'),'reciprocal native arrow binding');
  assert.equal(added.length,8,'only three native shapes/texts and two arrows');
}

export function checkReferenceNodeResize(previous,next,nodeId,bounds){
  const strip=(value,keys)=>Object.fromEntries(Object.entries(value).filter(([key])=>!keys.includes(key))),book=['version','versionNonce','updated'];
  assert.deepEqual(strip(next,['elements']),strip(previous,['elements']));assert.equal(next.elements.length,previous.elements.length);assert.equal(new Set(next.elements.map(e=>e.id)).size,next.elements.length);
  const oldNode=previous.elements.find(e=>e.id===nodeId),node=next.elements.find(e=>e.id===nodeId);assert.ok(oldNode&&node);
  for(const [i,key] of ['x','y','width','height'].entries())assert.ok(Number.isFinite(node[key])&&Math.abs(node[key]-bounds[i])<1e-6,`requested node ${key}`);
  const labels=previous.elements.filter(e=>e.type==='text'&&!e.isDeleted&&e.containerId===nodeId);assert.equal(labels.length,1);
  const arrows=previous.elements.filter(e=>e.type==='arrow'&&!e.isDeleted&&(e.startBinding?.elementId===nodeId||e.endBinding?.elementId===nodeId)),arrowIds=new Set(arrows.map(e=>e.id));assert.ok(arrows.length);
  const captionIds=new Set(previous.elements.filter(e=>e.type==='text'&&arrowIds.has(e.containerId)).map(e=>e.id));
  for(const old of previous.elements){const actual=next.elements.find(e=>e.id===old.id);assert.ok(actual);const allowed=old.id===nodeId?[...book,'x','y','width','height']:old.id===labels[0].id?[...book,'x','y','width','height','text','autoResize']:arrowIds.has(old.id)?[...book,'x','y','width','height','points']:captionIds.has(old.id)?[...book,'x','y']:[];assert.deepEqual(strip(actual,allowed),strip(old,allowed),`unrelated data unchanged: ${old.id}`);}
  const label=next.elements.find(e=>e.id===labels[0].id);assert.equal(label.text.replace(/\s/g,''),(labels[0].originalText??labels[0].text).replace(/\s/g,''));assert.ok([label.x,label.y,label.width,label.height].every(Number.isFinite)&&label.width>0&&label.height>0);assert.ok(label.x>=node.x-1e-6&&label.y>=node.y-1e-6&&label.x+label.width<=node.x+node.width+1e-6&&label.y+label.height<=node.y+node.height+1e-6);
  const world=e=>{assert.ok([e.x,e.y,e.width,e.height].every(Number.isFinite)&&e.width>=0&&e.height>=0&&e.width+e.height>0);assert.ok(Array.isArray(e.points)&&e.points.length>=2&&e.points.length<=1000);return e.points.map(p=>{assert.ok(p.length===2&&p.every(Number.isFinite));return[p[0]+e.x,p[1]+e.y];});};
  for(const old of arrows){const actual=next.elements.find(e=>e.id===old.id),before=world(old),after=world(actual),start=old.startBinding?.elementId===nodeId;assert.ok(!(start&&old.endBinding?.elementId===nodeId));const endpoint=start?after[0]:after.at(-1),far=start?after.at(-1):after[0],oldFar=start?before.at(-1):before[0],binding=actual[start?'startBinding':'endBinding'];assert.ok(Number.isFinite(binding.gap)&&binding.focus===0);const g=binding.gap,ports=[[node.x-g,node.y+node.height/2],[node.x+node.width+g,node.y+node.height/2],[node.x+node.width/2,node.y-g],[node.x+node.width/2,node.y+node.height+g]];assert.ok(ports.some(p=>Math.abs(p[0]-endpoint[0])<1e-6&&Math.abs(p[1]-endpoint[1])<1e-6),'resized node has a coherent centered port');for(let axis=0;axis<2;axis++)assert.ok(Math.abs(far[axis]-oldFar[axis])<1e-6,'far endpoint stays fixed');for(const [axis,key]of[[0,'width'],[1,'height']])assert.ok(Math.abs(actual[key]-(Math.max(...after.map(p=>p[axis]))-Math.min(...after.map(p=>p[axis]))))<1e-6,'canonical connector dimensions');}
}

export async function runLiveReference({page,call,path,handoff,send,persist,directory,turns,until}) {
  const png=await page.evaluate(()=>{
    const canvas=document.createElement('canvas');canvas.width=840;canvas.height=180;
    const c=canvas.getContext('2d');c.fillStyle='#ffffff';c.fillRect(0,0,840,180);
    const labels=['Collect','Image','Review'],colors=['#dbeafe','#dcfce7','#ffedd5'];
    c.font='26px sans-serif';c.textAlign='center';c.textBaseline='middle';
    for(let i=0;i<3;i++){
      const x=30+i*280;c.fillStyle=colors[i];c.fillRect(x,40,220,100);c.strokeStyle='#1f2937';c.lineWidth=2;c.strokeRect(x,40,220,100);c.fillStyle='#111827';c.fillText(labels[i],x+110,90);
      if(i<2){c.beginPath();c.moveTo(x+225,90);c.lineTo(x+270,90);c.moveTo(x+260,83);c.lineTo(x+270,90);c.lineTo(x+260,97);c.stroke();}
    }
    return canvas.toDataURL('image/png').split(',')[1];
  });
  await writeFile(join(directory,'uploaded-reference.png'),Buffer.from(png,'base64'));
  const chooser=page.waitForEvent('filechooser');
  await page.getByTestId('toolbar-image').locator('..').click();
  await(await chooser).setFiles({name:'concept-reference.png',mimeType:'image/png',buffer:Buffer.from(png,'base64')});
  const bounds=await page.locator('[data-canvas-surface]').boundingBox();assert.ok(bounds);
  await page.mouse.click(bounds.x+bounds.width/2,bounds.y+220);
  await page.getByRole('button',{name:'保存画布',exact:true}).click();
  let formal;
  await until(async()=>{formal=await call('GET',path);return formal.scene.document.elements.filter(e=>e.type==='image'&&!e.isDeleted).length===1;},'uploaded reference persisted');
  const image=formal.scene.document.elements.find(e=>e.type==='image'&&!e.isDeleted);
  assert.ok(formal.scene.document.files[image.fileId]);
  await handoff('选中的图片是一个纯概念流程参考，没有实验参数。请先用 read_image 实际查看参考图，再在参考图下方重建其中三个英文节点和两条有向关系，保留图中的英文原文与方向。只新增三个原生形状、三个绑定文字和两条绑定箭头，总计八个对象；不要额外添加标题、图注、来源文字或概念范围说明，本轮没有科学机制或实验条件需要声明。文字至少 16 号，节点留出清晰间距；这是明确授权的参考图重建，不需新增科学计划或机制。原图片、位置、尺寸、文件以及其他对象全部保留，不换成另一张位图、不采用。提交待审候选并实际读取候选 PNG。');
  const draft=await page.getByPlaceholder('输入消息…').inputValue();
  const context=await readCanvasPreparedContext(page);
  assert.ok(context.selected_objects.some(e=>e.id===image.id),'actual GUI selection reaches Agent context');
  await persist('reference-handoff.json',{draft,selected_image_id:image.id});
  formal=await call('GET',path);await persist('reference-formal-before.json',formal);
  await send(1);
  let proposal=currentCandidateLeaf(await call('GET',`${path}/proposals`),formal.metadata.revision);
  checkReferenceScene(formal.scene.document,proposal.document);
  assert.deepEqual(await call('GET',path),formal,'no adoption before user review');
  const calls=turns[0].calls.filter(c=>!c.is_error);
  assert.ok(calls.some(c=>c.name==='visualization'&&c.input.action==='read_image'&&c.input.element_id===image.id&&c.input.figure_id===formal.metadata.id&&c.input.project_id===formal.metadata.project_id&&c.input.expected_revision===formal.metadata.revision),'successful scoped image vision tool call');
  assert.ok(calls.some(c=>c.name==='visualization'&&c.input.action==='read_proposal_preview'&&c.input.proposal_id===proposal.id),'actual final candidate PNG read');
  const dialog=page.getByRole('dialog',{name:'Agent 修改预览',exact:true});
  const show=async candidate=>{await page.getByRole('tab',{name:'Live figure',exact:true}).click();await page.getByRole('button',{name:'查看 Agent 提案',exact:true}).click();const items=(await call('GET',`${path}/proposal-index?limit=100`)).items,matching=items.filter(p=>p.summary===candidate.summary&&p.base_revision===formal.metadata.revision&&Boolean(p.parent_proposal_id)===Boolean(candidate.parent_proposal_id)),index=matching.findIndex(p=>p.id===candidate.id);assert.ok(index>=0);await dialog.getByRole('button',{name:`${candidate.summary} · v${formal.metadata.revision}${candidate.parent_proposal_id?' · 修订':''}`,exact:true}).nth(index).click();await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();};
  const resizeMode=process.env.VISUALIZATION_LIVE_REFERENCE_NODE_GEOMETRY==='1';
  if(resizeMode){
    const parent=proposal,label=parent.document.elements.find(e=>e.type==='text'&&!e.isDeleted&&(e.originalText??e.text)==='Collect'),node=parent.document.elements.find(e=>e.id===label?.containerId);assert.ok(node);
    // Expand leftward so the existing forward connector retains horizontal room.
    const bounds=[node.x-30,node.y+5,node.width+30,node.height+20];await persist('reference-before-node-geometry.json',parent);await show(parent);
    const feedback=dialog.locator('details[aria-label="候选修订反馈"]');await feedback.locator('summary').click();await feedback.getByRole('textbox',{name:'候选修订要求',exact:true}).fill(`只调整 Collect 所属原生节点 ${node.id}：x=${bounds[0]}、y=${bounds[1]}、width=${bounds[2]}、height=${bounds[3]}。用 set_node_geometry 操作，使完整绑定文字按新范围重排且字体字号不变，连接线保持原有居中连接、连接间隙和原远端端点，不更改连接策略来挤入空间。Image、Review、原参考图、文件、所有其他对象及历史保持原样，不增加标题图注，不采用。先读取当前候选与 PNG，沿同一候选链修改，最后查看 PNG。`);await feedback.getByRole('button',{name:'准备候选修订草稿',exact:true}).click();let prepared;await until(async()=>{prepared=await page.getByPlaceholder('输入消息…').inputValue();return prepared.includes(parent.id)&&prepared.includes(parent.document_sha256);},'native node geometry feedback scope');await persist('reference-node-geometry-feedback.json',{draft:prepared,node_id:node.id,bounds});await send(2);
    const all=await call('GET',`${path}/proposals`);await persist('reference-node-geometry-candidates.json',all);proposal=currentCandidateLeaf(all,formal.metadata.revision);assert.notEqual(proposal.id,parent.id);checkReferenceNodeResize(parent.document,proposal.document,node.id,bounds);checkReferenceScene(formal.scene.document,proposal.document);assert.deepEqual(await call('GET',path),formal);assert.deepEqual(await call('GET',`${path}/proposals/${parent.id}`),parent);
    const edits=turns[1].calls.filter(c=>!c.is_error&&c.name==='visualization');assert.ok(edits.some(c=>c.input.project_id===formal.metadata.project_id&&c.input.figure_id===formal.metadata.id&&c.input.expected_revision===formal.metadata.revision&&c.input.operations?.some(op=>op.op==='set_node_geometry'&&op.id===node.id&&['x','y','width','height'].every((key,i)=>typeof op[key]==='number'&&Math.abs(op[key]-bounds[i])<1e-6))));for(const id of[parent.id,proposal.id])assert.ok(edits.some(c=>c.input.action==='read_proposal_preview'&&c.input.proposal_id===id));
  }
  await persist('reference-proposal.json',proposal);
  let preview;await until(async()=>{preview=await call('GET',`${path}/proposals/${proposal.id}/preview?expected_revision=${formal.metadata.revision}`);assert.notEqual(preview.render_status,'failed');return preview.render_status==='ready';},'reference reconstruction PNG');
  assert.equal(preview.document_sha256,proposal.document_sha256);await writeFile(join(directory,'reference-candidate.png'),Buffer.from(preview.dataURL.split(',')[1],'base64'));
  await show(proposal);
  await dialog.getByRole('region',{name:'候选画布'}).locator('.excalidraw').waitFor();await page.screenshot({path:join(directory,'reference-review.png'),animations:'disabled'});
  await dialog.getByRole('button',{name:'采用提案',exact:true}).click();await dialog.waitFor({state:'hidden'});
  const final=await call('GET',path);await persist('final.json',final);assert.equal(final.metadata.revision,formal.metadata.revision+1);assert.deepEqual(final.scene.document,proposal.document);
  const download=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const source=JSON.parse(await readFile(await(await download).path(),'utf8'));await persist('reference-source.json',source);checkPortableSceneExport(source,proposal.document);
  return {uploaded_reference_read:true,native_reconstruction:true,native_node_geometry_verified:resizeMode,insert_sketch_used:calls.some(c=>c.name==='visualization'&&c.input.operations?.some(op=>op.op==='insert_sketch')),upload_mode:'native_file_input_fallback_with_file_system_access_picker_disabled_for_automation',scope:'synthetic conceptual reference image uploaded/selected through GUI; actual model reads image and reconstructs native graph; optional explicit native node resize preserves unrelated objects; no biological mechanism claim; OS File System Access picker not evaluated'};
}
