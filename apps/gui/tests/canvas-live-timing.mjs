import assert from 'node:assert/strict';
import {installLiveProgress,readLiveProgress} from './helpers/canvas-live-progress.mjs';
import {summarizeLiveTiming} from './helpers/canvas-live-timing.mjs';
const listeners={};
const previous={window:globalThis.window,EventSource:globalThis.EventSource};
try{
 globalThis.window={addEventListener(){}};
 globalThis.EventSource=class {addEventListener(kind,fn){listeners[kind]=fn;}close(){}};
 const page={evaluate:async(fn,arg)=>fn(arg)};
 await installLiveProgress(page,'/test');
 let sequence=0;
 const emit=(event,agent='/test')=>listeners.agent({lastEventId:String(++sequence),data:JSON.stringify({agent,event})});
 emit({ToolCall:{name:'ignored',input:{action:'ignored'}}},'/other');
 emit({TurnStarted:{turn_id:'private-id'}});emit('Requesting');emit({StreamStart:{message:{text:'DO_NOT_RETAIN'}}});
 emit({ThinkingDelta:'SECRET_THINKING'});emit({StreamDelta:{stop_reason:'tool_use'}});
 emit({ToolCall:{name:'visualization',input:{action:'read_proposal',secret:'PRIVATE_ARGUMENT'}}});
 emit({ToolCall:{name:'visualization',input:{action:'read_proposal_preview'}}});
 emit({ToolResult:{ok:true,content:'秘密'}});emit({ToolResult:{ok:false,content:'PRIVATE_RESULT'}});
 emit('Requesting');emit({StreamStart:{message:{}}});emit({StreamDelta:{stop_reason:'end_turn'}});emit({TurnCompleted:{}});
 const progress=await readLiveProgress(page),serialized=JSON.stringify(progress);
 for(const secret of ['DO_NOT_RETAIN','SECRET_THINKING','PRIVATE_ARGUMENT','PRIVATE_RESULT','private-id'])assert.ok(!serialized.includes(secret));
 assert.equal(progress.tool_calls,2);assert.equal(progress.tool_errors,1);
 const report=summarizeLiveTiming(progress);assert.equal(report.complete,true);assert.equal(report.requests.length,2);assert.equal(report.tool_batches.length,1);
 assert.equal(report.tool_batches[0].calls.length,2);assert.equal(report.tool_batches[0].errors,1);assert.equal(report.tool_batches[0].text_bytes,Buffer.byteLength('秘密PRIVATE_RESULT'));
 assert.ok(report.requests.every(r=>r.request_to_stream_start_ms>=0&&r.stream_to_response_boundary_ms>=0));
 const withoutStreamDelta={...progress,timeline:progress.timeline.filter(e=>e.kind!=='StreamDelta')};
 const boundaryReport=summarizeLiveTiming(withoutStreamDelta);assert.equal(boundaryReport.complete,true);assert.equal(boundaryReport.requests[0].response_boundary,'ToolCall');assert.equal(boundaryReport.requests[1].response_boundary,'TurnCompleted');
 assert.equal(summarizeLiveTiming(progress,{afterSequence:sequence}).complete,false);
 assert.equal(summarizeLiveTiming({...progress,replay_incomplete:true}).complete,false);
 for(let i=0;i<2010;i++)emit('Requesting');
 assert.equal(window.__visualizationLiveProgress.timeline.length,2000);assert.ok(window.__visualizationLiveProgress.timeline_omitted>0);
 assert.equal(summarizeLiveTiming(await readLiveProgress(page)).complete,false);
 console.log('canvas-live-timing: metadata privacy, batched tool windows, turn scoping and explicit incomplete coverage passed');
}finally{
 for(const [key,value] of Object.entries(previous)){if(value===undefined)delete globalThis[key];else globalThis[key]=value;}
}
