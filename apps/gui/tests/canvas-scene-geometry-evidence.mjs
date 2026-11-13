import assert from 'node:assert/strict';
import {checkCandidateGeometryReviews} from './helpers/canvas-scene-geometry-evidence.mjs';
const candidate={id:'child',visualization_id:'figure',base_revision:2,document_sha256:'actual-hash',document:{elements:[{type:'frame',x:0,y:0,width:740,height:1280},{type:'text',x:40,y:1310,width:700,height:220,fontSize:16}]}};
const review={status:'available',bounds:{x:0,y:0,width:740,height:1530},aspect_ratio:740/1530,live_element_count:2,checked_element_count:2,skipped_element_count:0,omitted_element_count:0,min_text_font_size:16,unknown_text_font_size_count:0};
const history=[{content:[{type:'tool_use',id:'read',name:'visualization',input:{action:'read_proposal',project_id:'project',figure_id:'figure'}},{type:'tool_result',tool_use_id:'read',content:{proposal:candidate,scene_geometry_review:review}}]}];
assert.equal(checkCandidateGeometryReviews(history,candidate,{projectId:'project'}).length,1);
const mutation=structuredClone(history);mutation[0].content[0].name='visualization_add_text';mutation[0].content[1].content={proposal_id:candidate.id,figure_id:candidate.visualization_id,base_revision:2,document_sha256:candidate.document_sha256,scene_geometry_review:review};assert.equal(checkCandidateGeometryReviews(mutation,candidate,{projectId:'project'}).length,1);
for(const mutate of [h=>h[0].content[0].input.figure_id='other',h=>h[0].content[0].input.project_id='other',h=>h[0].content[1].is_error=true,h=>h[0].content[1].content.proposal.document_sha256='stale',h=>h[0].content[1].content.scene_geometry_review.status='partial',h=>h[0].content[1].content.scene_geometry_review.bounds.height=1280,h=>h[0].content[1].content.scene_geometry_review.checked_element_count=1]){
 const invalid=structuredClone(history);mutate(invalid);assert.throws(()=>checkCandidateGeometryReviews(invalid,candidate,{projectId:'project'}));
}
console.log('actual geometry evidence: exact candidate scope/hash and full caption bounds accepted; wrong scope, failed reads, stale hash, partial coverage and stale plan metrics rejected');
