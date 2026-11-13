// Audit actual delivered tool results, not request arguments or estimated tokens.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
const [directory,baselineDirectory]=process.argv.slice(2);
assert.ok(directory,'supply an actual scoped-source-review evaluation directory');
const load=async(dir,name)=>JSON.parse(await readFile(join(dir,name),'utf8'));
function reads(history){
 const uses=new Map(history.flatMap(m=>m.content).filter(b=>b.type==='tool_use').map(b=>[b.id,b]));
 return history.flatMap(m=>m.content).filter(b=>b.type==='tool_result'&&!b.is_error).flatMap(b=>{
  const use=uses.get(b.tool_use_id);if(use?.name!=='visualization'||use.input.action!=='read_proposal')return [];
  assert.equal(typeof b.content,'string');return [{input:use.input,value:JSON.parse(b.content),bytes:Buffer.byteLength(b.content)}];
 });
}
const result=await load(directory,'result.json');assert.equal(result.passed,true);assert.equal(result.scoped_reads_required,true);
const {parent}=await load(directory,'source-review-seed.json');
if(result.scoped_read_origin==='gui_focused_object'){
 const feedback=await load(directory,'source-review-feedback.json');
 assert.ok(!feedback.request.includes('element_ids'),'test request must not manually instruct the local-read API');
 const context=JSON.parse(feedback.draft.slice(feedback.draft.lastIndexOf('\n\n{')+2));
 assert.equal(context.focused_object_id,result.corrected_element_id);
 assert.deepEqual(context.follow_up_inputs.read_proposal.element_ids,[result.corrected_element_id]);
 assert.equal(context.follow_up_inputs.read_proposal.proposal_id,parent.id);
}

const history=await load(directory,'session-history.json');
const actual=reads(history);assert.ok(actual.length>0);
for(const read of actual){
 assert.deepEqual(read.input.element_ids,[result.corrected_element_id]);
 assert.equal(read.value.scope.hash_scope,'complete_immutable_proposal');
 assert.equal(read.value.scope.document_omitted,true);assert.ok(!('document' in read.value.proposal));
 assert.equal(read.value.geometry_context.non_geometry_fields_omitted,true);
 if(read.input.proposal_id===parent.id){
  assert.equal(read.value.proposal.document_sha256,parent.document_sha256);
  assert.deepEqual(read.value.selected_elements,[parent.document.elements.find(e=>e.id===result.corrected_element_id)]);
 }
}
const finalAssistant=history.filter(m=>m.role==='assistant').at(-1);
const report={passed:true,scoped_read_origin:result.scoped_read_origin??'explicit_request',final_reply_characters:(finalAssistant?.content??[]).filter(b=>b.type==='text').reduce((n,b)=>n+b.text.length,0),scope:'actual read_proposal UTF-8 text response bytes; excludes image blocks, tokens and provider cache; no causal latency claim',reads:actual.map(r=>({proposal_id:r.input.proposal_id,bytes:r.bytes,selected_count:r.value.selected_elements.length,context_count:r.value.geometry_context.elements.length,context_omitted:r.value.geometry_context.omitted_count})),turn_elapsed_ms:(await load(directory,'turn-1.json')).elapsed_ms};
if(baselineDirectory){
 const baseline=reads(await load(baselineDirectory,'session-history.json')).find(r=>r.value.proposal?.document_sha256===parent.document_sha256&&r.value.proposal?.document);
 assert.ok(baseline,'baseline must contain a full read of the same exact document hash');
 const scoped=actual.find(r=>r.input.proposal_id===parent.id);assert.ok(scoped);
 assert.deepEqual(scoped.value.scene_geometry_review,baseline.value.scene_geometry_review);
 report.same_document_comparison={document_sha256:parent.document_sha256,full_response_bytes:baseline.bytes,scoped_response_bytes:scoped.bytes,reduction_fraction:1-scoped.bytes/baseline.bytes,baseline_turn_elapsed_ms:(await load(baselineDirectory,'turn-1.json')).elapsed_ms};
}
await writeFile(join(directory,'scoped-read-evidence.json'),JSON.stringify(report,null,2)+'\n',{mode:0o600});
console.log(JSON.stringify(report));
