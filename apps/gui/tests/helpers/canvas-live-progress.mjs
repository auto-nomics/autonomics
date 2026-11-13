// Session history may be persisted only after a complete turn. Observe actual
// SSE metadata independently; never retain text, thinking or tool payloads.
export async function installLiveProgress(page,agent) {
  await page.evaluate(agent=>{
    const state={scope:'evaluation_agent_sse_cumulative',connected:false,replay_incomplete:false,stream_errors:0,counts:{},tool_calls:0,tool_errors:0,last_tools:[],last_seq:0,last_event_at:null,timeline:[],timeline_omitted:0,clock_scope:'browser_monotonic_event_arrival'};
    const started=performance.now();
    window.__visualizationLiveProgress=state;
    const stream=new EventSource('/api/v1/events');
    stream.addEventListener('open',()=>{state.connected=true;});
    stream.addEventListener('error',()=>{state.connected=false;state.stream_errors++;});
    stream.addEventListener('lag',()=>{state.replay_incomplete=true;});
    stream.addEventListener('agent',event=>{
      const sequence=Number(event.lastEventId);
      if(!Number.isSafeInteger(sequence)||sequence<=state.last_seq)return;
      const frame=JSON.parse(event.data);
      if(frame.agent!==agent)return;
      const kind=typeof frame.event==='string'?frame.event:Object.keys(frame.event??{})[0];
      if(!kind)return;
      state.last_seq=sequence;state.last_event_at=new Date().toISOString();
      state.counts[kind]=(state.counts[kind]??0)+1;
      if(['TurnStarted','TurnCompleted','Requesting','StreamStart','StreamDelta','ToolCall','ToolResult','Compact'].includes(kind)){
        const entry={kind,sequence,at_ms:performance.now()-started};
        if(kind==='ToolCall'){
          const call=frame.event.ToolCall;
          entry.name=typeof call.name==='string'?call.name.slice(0,100):null;
          entry.action=typeof call.input?.action==='string'?call.input.action.slice(0,100):null;
        }
        if(kind==='ToolResult'){
          entry.ok=frame.event.ToolResult.ok===true;
          entry.text_bytes=typeof frame.event.ToolResult.content==='string'?new TextEncoder().encode(frame.event.ToolResult.content).length:null;
        }
        if(kind==='StreamDelta'){
          const reason=frame.event.StreamDelta.stop_reason;
          entry.stop_reason=['end_turn','tool_use','max_tokens','stop_sequence','pause_turn','refusal'].includes(reason)?reason:null;
        }
        if(state.timeline.length<2000)state.timeline.push(entry);else state.timeline_omitted++;
      }
      if(kind==='ToolCall'){
        const call=frame.event.ToolCall;
        state.tool_calls++;
        state.last_tools.push({name:typeof call.name==='string'?call.name.slice(0,100):null,action:typeof call.input?.action==='string'?call.input.action.slice(0,100):null});
        if(state.last_tools.length>20)state.last_tools.shift();
      }
      if(kind==='ToolResult'&&frame.event.ToolResult.ok===false)state.tool_errors++;
    });
    window.addEventListener('pagehide',()=>stream.close(),{once:true});
  },agent);
}

export async function readLiveProgress(page) {
  try {return await page.evaluate(()=>window.__visualizationLiveProgress??null);}
  catch {return null;}
}
