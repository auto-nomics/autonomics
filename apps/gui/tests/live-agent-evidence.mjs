import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { observeLiveAgent } from './helpers/live-agent-observer.mjs';
import { retainAgentEvidence, retainFigureEvidence } from './helpers/live-agent-evidence.mjs';
const artifacts = new Map(), requests = [];
let deny = false, denyCandidates = false, status = 'ERROR';
const figurePath='/api/v1/visualization/projects/p/figures/f';
const server = createServer(async (req, res) => {
  requests.push(req.method+' '+req.url);
  res.setHeader('content-type','application/json');
  if(req.url==='/api/session'){
    let body='';for await(const chunk of req)body+=chunk;
    assert.equal(JSON.parse(body).token,'test-token');
    if(deny){res.writeHead(401);res.end('{}');return;}
    res.setHeader('set-cookie','autonomics_session=test-cookie; HttpOnly');res.end('{}');return;
  }
  assert.equal(req.headers.cookie,'autonomics_session=test-cookie');
  if(req.url===figurePath)res.end(JSON.stringify({metadata:{revision:2},scene:{document:{elements:[{id:'actual-note',text:'Scientific source scope'}],files:{}}}}));
  else if(req.url===figurePath+'/planning')res.end(JSON.stringify({version:3,status:'proposed'}));
  else if(req.url===figurePath+'/proposals'){if(denyCandidates){res.writeHead(500);res.end('{}');}else res.end(JSON.stringify([{id:'candidate',parent_proposal_id:'parent',document:{elements:[{id:'actual-note',text:'Candidate scope'}],files:{}}}]));}
  else if(req.url==='/api/v1/state')res.end(JSON.stringify({agents:[{path:'/root/figure',agent_id:'a1',status}],contexts:{'/root/figure':{id:'s1'}}}));
  else if(req.url==='/api/v1/agents/a1/sessions/s1/history')res.end(JSON.stringify([{role:'assistant',content:[{type:'image',data:'private-image-bytes'},{type:'text',text:'partial output'}]}]));
  else {res.writeHead(404);res.end('{}');}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
try {
  const options={origin:`http://127.0.0.1:${server.address().port}`,token:'test-token',agent:'/root/figure',persist:async(name,value)=>artifacts.set(name,value)};
  assert.equal(await retainAgentEvidence(options),'ERROR');
  assert.equal(artifacts.get('failure-agent-state.json').status,'ERROR');
  assert.equal(artifacts.get('failure-session-history.json')[0].content[0].data,'[IMAGE BYTES OMITTED]');
  assert.equal(artifacts.get('failure-session-history.json')[0].content[1].text,'partial output');
  assert.ok(!JSON.stringify([...artifacts]).includes('test-cookie'));
  status='STREAMING';let observed=0;
  await observeLiveAgent({poll:async()=>{const current=await retainAgentEvidence(options);observed++;if(observed===1)status='IDLE';return current==='IDLE';},isAlive:()=>true,windowMs:1000,intervalMs:1,onWindow:()=>assert.fail('unexpected window')});
  assert.equal(observed,2);
  assert.equal(artifacts.get('failure-agent-state.json').status,'IDLE');
  const report=await retainFigureEvidence({...options,figurePath});assert.deepEqual(report.saved,['figure','planning','candidates']);assert.deepEqual(report.errors,[]);
  assert.equal(artifacts.get('failure-candidates.json')[0].document.elements[0].text,'Candidate scope');
  assert.ok(!requests.some(request=>request.startsWith('PUT ')||request.startsWith('DELETE ')));
  denyCandidates=true;const partial=await retainFigureEvidence({...options,figurePath});assert.deepEqual(partial.saved,['figure','planning']);assert.equal(partial.errors[0].resource,'candidates');
  await assert.rejects(retainFigureEvidence({...options,figurePath:'/api/v1/state'}),/exact project figure path/);
  assert.ok(!JSON.stringify([...artifacts]).includes('test-cookie'));
  deny=true;const before=requests.length;
  await assert.rejects(retainAgentEvidence(options),/login HTTP 401/);
  assert.equal(requests.length,before+1);
  console.log('live-agent-evidence: passed (real HTTP, session authentication, terminal evidence, image redaction, scoped figure/candidate snapshots, partial retrieval, failed login)');
} finally {await new Promise(resolve=>server.close(resolve));}
