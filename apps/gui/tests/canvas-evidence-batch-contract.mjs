import assert from 'node:assert/strict';
import {sourceRecords,successfulRecordBatches,recordBatchSummary} from './helpers/canvas-live-evidence.mjs';
const first={requested_id:'R-HSA-1',status:'ok',source_url:'https://reactome.org/content/detail/R-HSA-1',record:{stId:'R-HSA-1'}};
const second={requested_id:'R-HSA-2',status:'error',source_url:'https://reactome.org/content/detail/R-HSA-2',record:{stId:'R-HSA-2'}};
const unrelated={...first,requested_id:'R-HSA-3'};
const history=[{content:[
 {type:'tool_use',id:'batch',name:'figure_evidence',input:{action:'record_batch',ids:['R-HSA-1','R-HSA-2']}},
 {type:'tool_result',tool_use_id:'batch',content:JSON.stringify({content:{results:[first,second,unrelated],all_requested_records_retrieved:false}})},
]}];
assert.deepEqual(sourceRecords(history),[first],'only successful explicitly requested batch entries count as read sources');
const failed=structuredClone(history);failed[0].content[1].is_error=true;assert.deepEqual(sourceRecords(failed),[]);
const navigation=structuredClone(history);navigation[0].content[0].input.action='events';assert.deepEqual(sourceRecords(navigation),[]);
const missing=structuredClone(history);delete missing[0].content[0].input.ids;assert.deepEqual(sourceRecords(missing),[]);
console.log('batch evidence: per-record status, requested scope and successful tool result are required; navigation/failed entries never become read evidence');

assert.deepEqual(successfulRecordBatches(history),[],'one successful source is not a successful multi-record read');
const two=structuredClone(history);two[0].content[1].content=JSON.stringify({results:[first,{...second,status:'ok'}]});
assert.deepEqual(successfulRecordBatches(two)[0].read_ids,['R-HSA-1','R-HSA-2']);
const duplicate=structuredClone(two);duplicate[0].content[1].content=JSON.stringify({results:[first,{...second,status:'ok',source_url:first.source_url}]});assert.deepEqual(successfulRecordBatches(duplicate),[]);

assert.equal(recordBatchSummary(history).incomplete_count_metadata_batches,1);
const counted=structuredClone(history);counted[0].content[1].content=JSON.stringify({results:[first,second],requested_count:2,success_count:1,failure_count:1});
assert.deepEqual(recordBatchSummary(counted),{returned_batches:1,reported_requested_items:2,reported_successful_items:1,reported_failed_or_deferred_items:1,incomplete_count_metadata_batches:0});
