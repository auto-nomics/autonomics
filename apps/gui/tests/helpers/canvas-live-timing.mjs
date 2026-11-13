// Arrival-time observations, not server spans or per-tool execution timers.
export function summarizeLiveTiming(progress, {afterSequence=0}={}) {
 const events=(progress?.timeline??[]).filter(e=>e.sequence>afterSequence);
 const requests=[],batches=[],warnings=[];
 let request=null,batch=null;
 const finishRequest=(event)=>{
  if(request){request.stream_stopped_at_ms=event.at_ms;request.response_boundary=event.kind;request=null;}
 };
 for(const event of events){
  if(event.kind==='ToolCall'||event.kind==='TurnCompleted')finishRequest(event);
  if(event.kind==='Requesting'){
   if(request)warnings.push('request_without_stream_stop');
   request={requested_at_ms:event.at_ms,stream_started_at_ms:null,stream_stopped_at_ms:null};requests.push(request);
  }
  if(event.kind==='StreamStart'){
   if(request&&request.stream_started_at_ms===null)request.stream_started_at_ms=event.at_ms;
   else warnings.push('stream_start_without_request');
  }
  if(event.kind==='StreamDelta'){
   if(request){request.stop_reason=event.stop_reason;finishRequest(event);}
   else warnings.push('stream_stop_without_request');
  }
  if(event.kind==='ToolCall'){
   if(!batch){batch={started_at_ms:event.at_ms,completed_at_ms:null,calls:[],results:0,text_bytes:0,errors:0};batches.push(batch);}
   batch.calls.push({name:event.name,action:event.action});
  }
  if(event.kind==='ToolResult'){
   if(!batch)warnings.push('tool_result_without_batch');
   else {batch.results++;batch.text_bytes+=event.text_bytes??0;if(!event.ok)batch.errors++;if(batch.results===batch.calls.length){batch.completed_at_ms=event.at_ms;batch=null;}}
  }
 }
 if(progress?.replay_incomplete)warnings.push('sse_replay_incomplete');
 if(progress?.timeline_omitted)warnings.push('timeline_limit_reached');
 if(progress?.stream_errors)warnings.push('sse_transport_errors');
 if(request)warnings.push('unfinished_request');
 if(batch)warnings.push('unfinished_tool_batch');
 const total=events.findLast(e=>e.kind==='TurnCompleted')?.at_ms;
 const start=events.find(e=>e.kind==='TurnStarted')?.at_ms;
 for(const r of requests){
  r.request_to_stream_start_ms=r.stream_started_at_ms===null?null:r.stream_started_at_ms-r.requested_at_ms;
  r.stream_to_response_boundary_ms=r.stream_started_at_ms===null||r.stream_stopped_at_ms===null?null:r.stream_stopped_at_ms-r.stream_started_at_ms;
 }
 for(const b of batches)b.window_ms=b.completed_at_ms===null?null:b.completed_at_ms-b.started_at_ms;
 return {scope:'browser SSE event arrival windows; request wait includes transport and queue; stream-to-boundary includes response processing and persistence, not pure generation; tool results may be batch-delivered; no per-tool/render/provider-only duration inference',complete:warnings.length===0&&start!==undefined&&total!==undefined,warnings,turn_window_ms:start===undefined||total===undefined?null:total-start,requests,tool_batches:batches};
}
