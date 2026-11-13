import assert from 'node:assert/strict';
import {emptySvgPathState,svgPathSnapshot,svgPathTransition,svgPathReadInputs} from '../src/features/canvas/canvas-svg-path-context.ts';
const input={project_id:'p',figure_id:'f',element_id:'image',expected_revision:2,sha256:'a'.repeat(64),part_id:'opaque-path-id',part_index:3,private_note:'PRIVATE',dataURL:'PRIVATE-BYTES'};
const formal=svgPathSnapshot(input);assert.equal(formal.scope,'saved_revision');assert.equal(formal.panel_visible,true);assert.ok(!JSON.stringify(formal).includes('PRIVATE'));
let state=svgPathTransition(emptySvgPathState,{kind:'inspect',owner:'view-a',snapshot:input});
state=svgPathTransition(state,{kind:'hide',owner:'older-owner'});assert.equal(state.snapshot.panel_visible,true);
state=svgPathTransition(state,{kind:'hide',owner:'view-a'});assert.equal(state.snapshot.panel_visible,false);assert.equal(state.snapshot.part_id,input.part_id);
assert.equal(svgPathTransition(state,{kind:'scene',revision:2,dirty:false,selectionChanged:false,selectedIds:[]}),state);
assert.equal(svgPathTransition(state,{kind:'head_observed',revision:1}),state,'older cached head must not invalidate a newer local snapshot');
assert.equal(svgPathTransition(state,{kind:'head_observed',revision:3}).snapshot,null);
for(const event of [{kind:'clear'},{kind:'scene',revision:3,dirty:false,selectionChanged:false,selectedIds:['image']},{kind:'scene',revision:2,dirty:true,selectionChanged:false,selectedIds:['image']},{kind:'scene',revision:2,dirty:false,selectionChanged:true,selectedIds:['other']}])assert.equal(svgPathTransition(state,event).snapshot,null);
const candidate={...input,proposal_id:'candidate',document_sha256:'b'.repeat(64),markup_ids:['review']};
state=svgPathTransition(state,{kind:'inspect',owner:'candidate-panel',snapshot:candidate});assert.equal(state.snapshot.scope,'proposal_candidate');assert.deepEqual(state.snapshot.markup_ids,['review']);
assert.equal(svgPathTransition(state,{kind:'hide',owner:'view-a'}),state,'late cleanup must not hide newer owner');
assert.equal(svgPathTransition(state,{kind:'candidate_decided',id:'other'}),state);
assert.equal(svgPathTransition(state,{kind:'candidate_decided',id:'candidate'}).snapshot,null);
assert.equal(svgPathTransition(state,{kind:'candidate_focused',id:'different'}).snapshot,null);
assert.equal(svgPathTransition(emptySvgPathState,{kind:'hide',owner:'candidate-panel'}).snapshot,null,'another editor view has independent state');
for(const invalid of [{...input,expected_revision:0},{...input,part_index:257},{...input,part_id:'x'.repeat(513)},{...input,sha256:'not-hash'},{...candidate,document_sha256:undefined},{...candidate,markup_ids:['x','x']},{...input,markup_ids:['review']},{...candidate,markup_ids:Array.from({length:50},(_,i)=>String(i).padEnd(512,'x'))}])assert.equal(svgPathSnapshot(invalid),null);

const formalReads=svgPathReadInputs(input);assert.deepEqual(formalReads.read.element_ids,['image']);assert.equal(formalReads.read_preview,undefined,'do not promise an uncaptured formal preview');assert.equal(svgPathReadInputs(input,true).read_preview.expected_revision,2);assert.equal(formalReads.parts.proposal_id,undefined);assert.equal(formalReads.geometry.sha256,input.sha256);
const candidateReads=svgPathReadInputs(candidate);assert.equal(candidateReads.read,undefined);assert.equal(candidateReads.read_image,undefined);assert.equal(candidateReads.read_proposal.proposal_id,'candidate');assert.equal(candidateReads.parts.document_sha256,candidate.document_sha256);assert.equal(candidateReads.geometry.part_id,candidate.part_id);
for(const args of Object.values(candidateReads)){assert.equal(args.markup_ids,undefined,'review selection never belongs to read-only input');assert.equal(args.expected_revision,2);assert.equal(args.project_id,'p');assert.equal(args.figure_id,'f');}
assert.deepEqual(svgPathSnapshot(candidate).follow_up_inputs,candidateReads);
const longIds={...input,project_id:'p'.repeat(512),figure_id:'f'.repeat(512),element_id:'e'.repeat(512),part_id:'i'.repeat(512)};const bounded=svgPathSnapshot(longIds);assert.equal(bounded.follow_up_inputs_omitted,'size_limit');assert.equal(bounded.part_id,longIds.part_id);assert.equal(bounded.follow_up_inputs,undefined);assert.ok(new TextEncoder().encode(JSON.stringify(bounded)).length<=4096);

console.log('svg path context: identity, lifecycle, read-only arguments and bounded template fallback passed');
