// Verify actual tool-delivered metrics against the exact immutable candidate.
// Evaluation figures here are axis-aligned; this does not certify visual quality.
import assert from 'node:assert/strict';
export function checkCandidateGeometryReviews(history,candidate,{projectId}={}){
 const elements=candidate.document.elements.filter(e=>e.isDeleted!==true);
 const positions=elements.flatMap(e=>{
  assert.ok(!e.angle,'geometry evidence replay expects axis-aligned evaluation objects');
  assert.ok(Number.isFinite(e.x)&&Number.isFinite(e.y));
  if(['arrow','line','freedraw'].includes(e.type))return e.points.map(([x,y])=>[e.x+x,e.y+y]);
  assert.ok(Number.isFinite(e.width)&&Number.isFinite(e.height));return [[e.x,e.y],[e.x+e.width,e.y+e.height]];
 });
 assert.ok(positions.length&&positions.every(p=>p.every(Number.isFinite)));
 const x=Math.min(...positions.map(p=>p[0])),y=Math.min(...positions.map(p=>p[1]));
 const expected={x,y,width:Math.max(...positions.map(p=>p[0]))-x,height:Math.max(...positions.map(p=>p[1]))-y};
 const fonts=elements.filter(e=>e.type==='text').map(e=>e.fontSize);assert.ok(fonts.every(size=>Number.isFinite(size)&&size>0));
 const blocks=history.flatMap(message=>message.content),calls=new Map(blocks.filter(b=>b.type==='tool_use').map(b=>[b.id,b]));
 const reviews=[];
 for(const block of blocks){
  const call=calls.get(block.tool_use_id);
  if(block.type!=='tool_result'||block.is_error||!call||!(/^(visualization(?:_|$))/.test(call.name)||['bioart','bioicons'].includes(call.name)))continue;
  if(call.input?.figure_id!==candidate.visualization_id||projectId&&call.input?.project_id!==projectId)continue;
  let value;try{value=typeof block.content==='string'?JSON.parse(block.content):block.content;}catch{continue;}value=value?.content??value;
  const proposal=value?.proposal??value;if(!proposal||!value?.scene_geometry_review)continue;
  if((proposal.id??proposal.proposal_id)!==candidate.id||proposal.document_sha256!==candidate.document_sha256)continue;
  if((proposal.visualization_id??proposal.figure_id)!==candidate.visualization_id||proposal.base_revision!==candidate.base_revision)continue;
  const r=value.scene_geometry_review;assert.equal(r.status,'available');
  assert.equal(r.skipped_element_count,0);assert.equal(r.omitted_element_count,0);assert.equal(r.unknown_text_font_size_count,0);
  assert.equal(r.checked_element_count,elements.length);assert.equal(r.live_element_count,elements.length);
  for(const [key,value]of Object.entries(expected))assert.ok(Number.isFinite(r.bounds?.[key])&&Math.abs(r.bounds[key]-value)<1e-6,`actual candidate ${key} metric`);
  const ratio=expected.width>0&&expected.height>0?expected.width/expected.height:null;
  if(ratio===null)assert.equal(r.aspect_ratio,null);else assert.ok(Number.isFinite(r.aspect_ratio)&&Math.abs(r.aspect_ratio-ratio)<1e-9);
  assert.equal(r.min_text_font_size,fonts.length?Math.min(...fonts):null);
  reviews.push({tool_name:call.name,tool_use_id:block.tool_use_id,proposal_id:candidate.id,document_sha256:candidate.document_sha256,review:r});
 }
 assert.ok(reviews.length,'actual registered tool delivered full-scene geometry for this exact candidate scope and hash');return reviews;
}
