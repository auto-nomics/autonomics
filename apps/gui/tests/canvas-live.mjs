import {finalizeFreshFigure} from './helpers/canvas-live-full-finalize.mjs';
import {runLiveApoptosisMaterialize} from './helpers/canvas-live-apoptosis-materialize.mjs';
import {runLiveCropRepair} from './helpers/canvas-live-crop-repair.mjs';
import {runLiveStyleAuthor} from './helpers/canvas-live-style-author.mjs';
import {runLiveFramePack} from './helpers/canvas-live-frame-pack.mjs';
import {runLiveCheckpointResume} from './helpers/canvas-live-checkpoint-resume.mjs';
import {runLiveSvgRefit} from './helpers/canvas-live-svg-refit.mjs';
import {runLiveStrokeWidth} from './helpers/canvas-live-stroke-width.mjs';
import {runLiveCaptionPosition} from './helpers/canvas-live-caption-position.mjs';
import {runLiveBatchSearch} from './helpers/canvas-live-batch-search.mjs';
import {runLiveFramePosition} from './helpers/canvas-live-frame-position.mjs';
import {runLivePaintedReview} from './helpers/canvas-live-painted-review.mjs';
import {runLiveMaterializationRoleRepair} from './helpers/canvas-live-materialization-role-repair.mjs';
import {runLiveWholeMaterialization} from './helpers/canvas-live-whole-materialization.mjs';
import {tgfbScenario} from './helpers/canvas-tgfb-evidence.mjs';
import {runLiveCircleAnnotation} from './helpers/canvas-live-circle-annotation.mjs';
import {runLiveGenericProtein} from './helpers/canvas-live-generic-protein.mjs';
import {runLiveRasSearch} from './helpers/canvas-live-ras-search.mjs';
import {runLiveCaptionColumn} from './helpers/canvas-live-caption-column.mjs';
import {runLiveFrameSwap} from './helpers/canvas-live-frame-swap.mjs';
import {runLiveCaptionWidth} from './helpers/canvas-live-caption-width.mjs';
import {runLiveFrameFit} from './helpers/canvas-live-frame-fit.mjs';
import {runLiveResidueReview} from './helpers/canvas-live-residue-review.mjs';
import {runLiveCandidatePosition} from './helpers/canvas-live-candidate-position.mjs';
import {readCanvasPreparedContext} from './helpers/canvas-prepared-context.mjs';
import {runLiveMarkupPosition} from './helpers/canvas-live-markup-position.mjs';
import {runLiveRelationReview} from './helpers/canvas-live-relation-review.mjs';
import {runLivePlanReview} from "./helpers/canvas-live-plan-review.mjs";
import { runLiveContextArtwork } from './helpers/canvas-live-context-artwork.mjs';
import { apoptosisScenario } from './helpers/canvas-apoptosis-evidence.mjs';
import { egfrScenario } from './helpers/canvas-egfr-evidence.mjs';
import { runLiveFormalSourceReview } from './helpers/canvas-live-formal-source-review.mjs';
import { summarizeLiveTiming } from './helpers/canvas-live-timing.mjs';
import { evaluationConfig } from './helpers/canvas-live-config.mjs';
import { runLiveRouteReview } from "./helpers/canvas-live-route-review.mjs";
import { runLiveReference } from "./helpers/canvas-live-reference.mjs";
import { installLiveProgress, readLiveProgress } from './helpers/canvas-live-progress.mjs';
import { runLiveReviewedRepair } from "./helpers/canvas-live-reviewed-repair.mjs";
import { runLiveEvidenceWorkflow } from "./helpers/canvas-live-evidence-workflow.mjs";
import { retainAgentEvidence, retainFigureEvidence } from './helpers/live-agent-evidence.mjs';
import { observeLiveAgent } from './helpers/live-agent-observer.mjs';
import { runLiveIfngArt } from "./helpers/canvas-live-ifng-art.mjs";
import { runLiveSourceReview, retrievalScopeReview } from "./helpers/canvas-live-source-review.mjs";
import { runLiveEvidencePlanning } from './helpers/canvas-live-evidence.mjs';
import { runLiveComposition } from './helpers/canvas-live-compose.mjs';
// Opt-in real Agent + configured provider + tools + SQLite + GUI integration.
// Default mode fixtures only the surrounding workbench APIs; gateway mode uses real HTTP/login/SSE.
// Model responses are never scripted.
import { chromium } from 'playwright';
import { runCandidateRevision } from './helpers/canvas-live-candidate.mjs';
import { runLiveSvgVisibleRepair } from './helpers/canvas-live-svg-visible.mjs';
import { runLiveSvgRefinement, runLiveBioartSvgAcquisition } from './helpers/canvas-live-svg.mjs';
import { runLiveIllustrationFit } from './helpers/canvas-live-fit.mjs';
import { runCandidateMaterialization } from './helpers/canvas-live-materialize.mjs';
import { runScientificCase } from './helpers/canvas-live-scientific.mjs';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, mkdir, writeFile, readFile } from 'node:fs/promises';
import { tmpdir, homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const evaluationSettings=evaluationConfig(process.env);
const planReviewSource=process.env.VISUALIZATION_LIVE_PLAN_REVIEW_SOURCE;
assert.ok(process.env.VISUALIZATION_LIVE_PLAN_REVIEW_BATCH!=='1'||(process.env.VISUALIZATION_LIVE_PLAN_REVIEW_TYPED==='1'&&process.env.VISUALIZATION_LIVE_PLAN_REVIEW_FOCUS==='1'&&process.env.VISUALIZATION_LIVE_PLAN_REVIEW==='1'&&planReviewSource),'batch plan review requires typed focused plan-review mode and saved source');
assert.ok(process.env.VISUALIZATION_LIVE_PLAN_REVIEW_TYPED!=='1'||(process.env.VISUALIZATION_LIVE_PLAN_REVIEW_FOCUS==='1'&&process.env.VISUALIZATION_LIVE_PLAN_REVIEW==='1'&&planReviewSource),'typed plan review requires focused plan-review mode and saved source');
assert.ok(process.env.VISUALIZATION_LIVE_PLAN_REVIEW_FOCUS!=='1'||(process.env.VISUALIZATION_LIVE_PLAN_REVIEW==='1'&&planReviewSource),'focused plan review requires explicit plan-review mode and saved source');
assert.ok(process.env.VISUALIZATION_LIVE_PLAN_REVIEW!=='1'||planReviewSource,'plan review requires VISUALIZATION_LIVE_PLAN_REVIEW_SOURCE pointing to an explicit saved plan');
assert.ok(process.env.VISUALIZATION_LIVE_PLAN_REVIEW_EDIT_READONLY!=='1'||(planReviewSource&&process.env.VISUALIZATION_LIVE_PLAN_REVIEW==='1'),'edited read-only plan evaluation requires an explicit saved plan review');
const ifngArtSource=process.env.VISUALIZATION_LIVE_IFNG_ART_SCENE;
const sourceReviewSource=process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE;
assert.ok(process.env.VISUALIZATION_LIVE_MARKUP_HISTORY!=='1'||(sourceReviewSource&&process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='markup-position'),'historical markup live evaluation requires saved-source markup-position mode');
assert.ok(process.env.VISUALIZATION_LIVE_MARKUP_EDIT_DRAFT!=='1'||(sourceReviewSource&&process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='markup-position'),'editing the prepared markup draft requires saved-source markup-position mode');
const sourceReviewSpecPath=process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_SPEC;
assert.ok(!sourceReviewSpecPath||(sourceReviewSource&&!process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE),'explicit review spec requires a saved scene and the generic source-review mode');
const sourceReviewSpec=sourceReviewSpecPath?JSON.parse(await readFile(sourceReviewSpecPath,'utf8')):null;
if(sourceReviewSpec){
 for(const flag of ['scopedRead','guiScoped'])if(flag in sourceReviewSpec)assert.equal(typeof sourceReviewSpec[flag],'boolean');
 assert.ok(!sourceReviewSpec.guiScoped||sourceReviewSpec.scopedRead,'GUI focus review requires scoped-read verification');
 assert.equal(typeof sourceReviewSpec.elementId,'string');assert.ok(sourceReviewSpec.elementId.trim());
 assert.ok(Array.isArray(sourceReviewSpec.corrections)&&sourceReviewSpec.corrections.length>0&&sourceReviewSpec.corrections.length<=20);
 for(const pair of sourceReviewSpec.corrections){assert.ok(Array.isArray(pair)&&pair.length===2&&pair.every(value=>typeof value==='string'));assert.ok(pair[0].length>0&&pair[0]!==pair[1]);}
}

assert.ok(!['apoptosis-materialize','crop-repair','style-author','frame-pack','checkpoint-resume','svg-refit','stroke-width','caption-position','batch-search','frame-position','painted-review','materialization-role-repair','whole-materialization','circle-annotation','generic-protein','ras-search','caption-column','frame-swap','caption-width','frame-fit','egfr-residue-labels','candidate-position','markup-position','relation-readonly','retrieval-scope','scoped-retrieval-scope','gui-scoped-retrieval-scope','formal-scoped-retrieval-scope','apoptosis-context-art'].includes(process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE)||sourceReviewSource,'selected source-review case requires VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE');
assert.ok(process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE!=='whole-materialization'||process.env.VISUALIZATION_LIVE_WHOLE_EVIDENCE_RECORDS,'whole-materialization requires explicit read evidence records for source export verification');
assert.ok(process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE!=='connector-routing'||sourceReviewSource,'connector-routing requires VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE pointing to the explicit saved EGFR crossing case');
assert.ok(process.env.VISUALIZATION_LIVE_CONTEXT_ART_IMAGE_INFO!=='1'||(sourceReviewSource&&process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='apoptosis-context-art'),'candidate image info evaluation requires explicit source and contextual artwork mode');
assert.ok(process.env.VISUALIZATION_LIVE_CONTEXT_ART_COMPACT!=='1'||(sourceReviewSource&&process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='apoptosis-context-art'),'aspect-aware artwork requires an explicit saved source and contextual artwork mode');
const liveEvidenceWorkflow=process.env.VISUALIZATION_LIVE_EVIDENCE_WORKFLOW==='1';
const liveEvidence=process.env.VISUALIZATION_LIVE_EVIDENCE==='1'||liveEvidenceWorkflow;
const evidenceCase=process.env.VISUALIZATION_LIVE_EVIDENCE_CASE??'ifng';
if(liveEvidence)assert.ok(['ifng','egfr-mapk','intrinsic-apoptosis','tgfb-smad'].includes(evidenceCase),'unsupported evidence case');
assert.ok(!(liveEvidenceWorkflow&&['intrinsic-apoptosis','tgfb-smad'].includes(evidenceCase)),'apoptosis currently uses the source-to-sketch evaluation, not the receptor-materialization workflow');
const liveCompose=process.env.VISUALIZATION_LIVE_COMPOSE==='1';
const liveReference=process.env.VISUALIZATION_LIVE_REFERENCE==='1';
assert.ok(process.env.VISUALIZATION_LIVE_REFERENCE_NODE_GEOMETRY!=='1'||liveReference,'reference node geometry requires the separate reference-image evaluation');
const liveSvg=process.env.VISUALIZATION_LIVE_SVG==='1';
const bioartSvg=process.env.VISUALIZATION_LIVE_SVG_BIOART==='1';
const onlineSvg=process.env.VISUALIZATION_LIVE_SVG_ONLINE==='1';
const visibleSvg=process.env.VISUALIZATION_LIVE_SVG_VISIBLE==='1';
const pathHandoff=evaluationSettings.svg_settings.path_handoff;
const candidatePathHandoff=evaluationSettings.svg_settings.candidate_path_handoff;
const sharedPathContext=evaluationSettings.svg_settings.shared_path_context;
assert.ok(!onlineSvg||(liveSvg&&bioartSvg&&!process.env.VISUALIZATION_LIVE_FIT_SCENE),'online BioArt SVG requires SVG/BioArt mode and no imported scene');
assert.ok(!bioartSvg||(liveSvg&&(onlineSvg||process.env.VISUALIZATION_LIVE_FIT_SCENE)),'BioArt SVG refinement requires SVG mode and an explicit saved scene or online acquisition');
const liveFit=process.env.VISUALIZATION_LIVE_FIT==='1';
const fitSource=process.env.VISUALIZATION_LIVE_FIT_SCENE||fileURLToPath(new URL('./fixtures/scientific-egfr-mapk-illustrated.json',import.meta.url));
const materializationSource=process.env.VISUALIZATION_LIVE_MATERIALIZE_SCENE;
assert.ok(process.env.VISUALIZATION_LIVE_MATERIALIZE!=='1'||materializationSource,'materialization requires VISUALIZATION_LIVE_MATERIALIZE_SCENE pointing to an explicit saved scientific scene');
const scientificCase=process.env.VISUALIZATION_LIVE_SCIENTIFIC;
const candidateMode=process.env.VISUALIZATION_LIVE_CANDIDATE==='1';
const gatewayMode=process.env.VISUALIZATION_LIVE_GATEWAY==='1';
const reviewSource=process.env.VISUALIZATION_LIVE_REVIEW_SCENE, prepareOnly=process.env.VISUALIZATION_LIVE_PREPARE_ONLY==='1';
assert.ok(!liveReference || (gatewayMode&&!ifngArtSource&&!sourceReviewSource&&!liveEvidence&&!liveCompose&&!liveSvg&&!liveFit&&!materializationSource&&!scientificCase&&!reviewSource&&!candidateMode&&!prepareOnly),'reference image requires a separate Gateway evaluation');
assert.ok(!ifngArtSource || (gatewayMode&&!sourceReviewSource&&!liveEvidence&&!liveCompose&&!liveSvg&&!liveFit&&!materializationSource&&!scientificCase&&!reviewSource&&!candidateMode&&!prepareOnly),'IFNG art requires an explicit reviewed scene and a separate Gateway evaluation');
assert.ok(!sourceReviewSource || (gatewayMode&&!liveEvidence&&!liveCompose&&!liveSvg&&!liveFit&&!materializationSource&&!scientificCase&&!reviewSource&&!candidateMode&&!prepareOnly),'source review requires an explicit saved scene and a separate Gateway evaluation');
assert.ok(!liveEvidence || (gatewayMode&&!liveCompose&&!liveSvg&&!liveFit&&!materializationSource&&!scientificCase&&!reviewSource&&!candidateMode&&!prepareOnly),'live evidence requires a separate Gateway evaluation');
assert.ok(!liveCompose || (gatewayMode&&!liveSvg&&!liveFit&&!materializationSource&&!scientificCase&&!reviewSource&&!candidateMode&&!prepareOnly),'live composition requires a separate Gateway evaluation');
assert.ok(!liveSvg || (gatewayMode&&!liveFit&&!materializationSource&&!scientificCase&&!reviewSource&&!candidateMode&&!prepareOnly),'live SVG requires a separate Gateway evaluation');
assert.ok(!liveFit || (gatewayMode&&!materializationSource&&!scientificCase&&!reviewSource&&!candidateMode&&!prepareOnly),'live fit requires a separate Gateway evaluation');
assert.ok(!materializationSource || (gatewayMode&&!scientificCase&&!reviewSource&&!candidateMode&&!prepareOnly),'materialization requires a separate Gateway evaluation');
assert.ok(!scientificCase || (gatewayMode&&!reviewSource&&!candidateMode&&!prepareOnly),'scientific case requires an empty Gateway evaluation');
assert.ok(!candidateMode || (gatewayMode&&reviewSource&&!prepareOnly),'candidate case requires Gateway mode and an explicit saved illustrated scene');
assert.ok(!gatewayMode || !prepareOnly, 'gateway mode uses a real configured model');
assert.ok(!prepareOnly || reviewSource, 'prepare-only requires an explicit saved review scene');
assert.ok(!planReviewSource||(gatewayMode&&!liveReference&&!ifngArtSource&&!sourceReviewSource&&!liveEvidence&&!liveCompose&&!liveSvg&&!liveFit&&!materializationSource&&!scientificCase&&!reviewSource&&!candidateMode&&!prepareOnly),'plan review requires a separate Gateway evaluation');
const provider = process.env.VISUALIZATION_LIVE_PROVIDER, model = process.env.VISUALIZATION_LIVE_MODEL;
const turnTimeout=Number(process.env.VISUALIZATION_LIVE_TURN_TIMEOUT_MS||((planReviewSource||liveReference||liveFit||liveSvg||liveCompose||liveEvidence||sourceReviewSource||ifngArtSource)?'900000':'250000'));assert.ok(Number.isSafeInteger(turnTimeout)&&turnTimeout>=1000,'turn observation duration must be a positive integer >=1000 ms');
assert.ok(prepareOnly || (provider && model), 'set VISUALIZATION_LIVE_PROVIDER and VISUALIZATION_LIVE_MODEL to an existing configured vision model');
const config = process.env.VISUALIZATION_LIVE_CONFIG || join(homedir(), '.autonomics/config.db');
const binary = gatewayMode ? (process.env.VISUALIZATION_LIVE_GATEWAY_BIN || fileURLToPath(new URL('../../../target/debug/examples/visualization_gateway_live',import.meta.url))) : prepareOnly ? (process.env.VISUALIZATION_NATIVE_BIN || fileURLToPath(new URL('../../../target/debug/examples/visualization_native', import.meta.url))) : process.env.VISUALIZATION_LIVE_BIN || fileURLToPath(new URL('../../../target/debug/examples/visualization_live', import.meta.url));
assert.ok(existsSync(binary) && (prepareOnly || existsSync(config)), 'build the selected test host; live mode also requires a config database');
const directory = process.env.VISUALIZATION_LIVE_OUTPUT || await mkdtemp(join(tmpdir(), 'autonomics-canvas-live-'));
await mkdir(directory, { recursive: true });
assert.ok(!existsSync(join(directory,"figures.sqlite")), "use a fresh output directory; existing evaluation databases are never reused");
const native = spawn(binary, gatewayMode ? [directory,config,provider,model] : prepareOnly ? [join(directory,'figures.sqlite')] : [config, provider, model, join(directory, 'figures.sqlite'), join(directory, 'trace.jsonl')], { stdio: ['ignore', 'pipe', 'pipe'] });
const port = Number(process.env.VISUALIZATION_LIVE_PORT || '1449');
assert.ok(Number.isInteger(port) && port > 1024 && port < 65536, 'choose an unprivileged live test port');
let origin = `http://127.0.0.1:${port}`, gatewayReady, figureEvidencePath;let sseResponses=0;const seenGatewayTools=new Set();
const vite = gatewayMode ? null : spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let output = '', viteOutput = '', browser, page; const turns = [], errors = [], checkpoints = [];
native.stdout.on('data', chunk => { output += chunk; process.stdout.write(chunk); }); native.stderr.on('data', chunk => output += chunk);
vite?.stdout.on('data', chunk => viteOutput += chunk); vite?.stderr.on('data', chunk => viteOutput += chunk);
const until = async (predicate, message, duration = 60000) => { const deadline = Date.now() + duration; while (!await predicate()) { if (Date.now() > deadline) throw new Error(message); await new Promise(resolve => setTimeout(resolve, 100)); } };
const persist = async (name, value) => writeFile(join(directory, name), JSON.stringify(name==='result.json'?{...value,evaluation_profile:evaluationSettings.profile,evaluation_settings:evaluationSettings.settings}:value, null, 2), { mode: 0o600 });
try {
  if(gatewayMode){
    await until(async()=>{if(native.exitCode!==null||native.signalCode!==null)throw new Error('gateway host stopped');try{gatewayReady=JSON.parse(await readFile(join(directory,'ready.json'),'utf8'));return true;}catch{return false;}},'production gateway readiness');origin=gatewayReady.origin;
  }else{
    await until(() => { if (native.exitCode !== null || native.signalCode !== null) throw new Error(output); return /VISUALIZATION_(?:LIVE|NATIVE)_ADDR=/.test(output); }, 'live host readiness');
    await until(() => { if (vite.exitCode !== null || vite.signalCode !== null) throw new Error(viteOutput); return viteOutput.includes(origin); }, 'Vite readiness');
  }
  await persist('evaluation-config.json',evaluationSettings);
  const backend = gatewayMode ? origin : `http://${output.match(/VISUALIZATION_(?:LIVE|NATIVE)_ADDR=([^\s]+)/)[1]}`;
  const call = async (method, path, body, timeout = 30000) => {
    if(gatewayMode){const response=await page.context().request.fetch(`${backend}${path}`,{method,...(body===undefined?{}:{data:body}),timeout});const value=await response.json();assert.ok(response.ok(),`HTTP ${response.status()} ${path}`);return value;}
    const response = await fetch(`${backend}${path}`, { method, headers: body === undefined ? {} : { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(timeout) });
    const value = await response.json(); assert.ok(response.ok, `${response.status} ${JSON.stringify(value)}`); return value;
  };
  browser = await chromium.launch({ executablePath: process.env.PDF_TEST_CHROMIUM || (existsSync('/usr/bin/chromium') ? '/usr/bin/chromium' : undefined), args: ['--no-sandbox'] });
  page = await browser.newPage({ viewport: { width: 1600, height: 1000 } }); page.setDefaultTimeout(30000); page.on('pageerror', error => errors.push(error.message));
  if(!gatewayMode){
  await page.route('**/api/session', route => route.fulfill({ json: {} }));
  await page.route('**/api/v1/**', async route => {
    const url = new URL(route.request().url()), path = url.pathname.replace('/api/v1', '');
    if (path.startsWith('/visualization/')) return route.fulfill({ response: await route.fetch({ url: `${backend}${url.pathname}${url.search}` }) });
    if (path === '/events') return new Promise(() => {});
    if (path === '/state') return route.fulfill({ json: { last_seq: 0, agents: [{ path: '/root/alpha', agent_id: 'a1', status: 'Idle' }], contexts: { '/root/alpha': { id: 's1', title: 'Live figure discussion', created_at: 1, last_active: 1 } }, display_settings: { collapse_thinking: false, collapse_tool_calls: false, collapse_tool_results: false } } });
    if (path.endsWith('/history')) return route.fulfill({ json: { messages: [] } });
    if (path.endsWith('/messages') && route.request().method() === 'POST') {
      try { const result = await call('POST', '/test/turn', { text: route.request().postDataJSON().text }, 250000); turns.push(result); await persist(`turn-${turns.length}.json`, result); return route.fulfill({ json: {} }); }
      catch (error) { errors.push(String(error)); return route.fulfill({ status: 502, json: { error: String(error) } }); }
    }
    if (path === '/profiles') return route.fulfill({ json: ['researcher'] });
    if (path === '/storage/agents') return route.fulfill({ json: [] });
    if (path === '/model-config') return route.fulfill({ json: { models: [], providers: [] } });
    if (path === '/settings') return route.fulfill({ json: {} });
    if (path === '/gateway/status') return route.fulfill({ json: { running: true } });
    return route.fulfill({ status: 404, json: { error: `Unrelated fixture: ${path}` } });
  });
  }else{
    assert.equal((await fetch(`${origin}/api/v1/state`)).status,401);
    await page.context().grantPermissions(['clipboard-read','clipboard-write'],{origin});
    const login=await page.context().request.post(`${origin}/api/session`,{data:{token:gatewayReady.token}});assert.ok(login.ok());
    assert.ok((await page.context().cookies()).some(cookie=>cookie.name==='autonomics_session'&&cookie.httpOnly));
    page.on('response',response=>{if(new URL(response.url()).pathname==='/api/v1/events'&&response.status()===200)sseResponses++;});
  }
  // Playwright filechooser automates native <input type=file>, not the OS-level
  // File System Access picker. Exercise Excalidraw's real browser fallback.
  if(liveReference)await page.addInitScript(()=>{delete window.showOpenFilePicker;});
  await page.goto(origin, { waitUntil: 'domcontentloaded', timeout: 60000 });
  if(gatewayMode)await installLiveProgress(page,gatewayReady.agent);
  await page.getByRole('navigation', { name: '工作台活动栏' }).getByRole('button', { name: '画布', exact: true }).click();
  await page.getByRole('textbox', { name: '新项目名称' }).fill('Live figure evaluation'); await page.getByRole('button', { name: '创建项目', exact: true }).click();
  await page.getByRole('button', { name: '新建画布', exact: true }).click(); await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  await page.getByRole('textbox', { name: '画布名称' }).fill('Live figure');
  const project = await page.getByRole('combobox', { name: '可视化项目' }).inputValue(); const figure = (await call('GET', `/api/v1/visualization/projects/${project}/figures`))[0].id;
  const path = `/api/v1/visualization/projects/${project}/figures/${figure}`;figureEvidencePath=path;
  let reasoningConfigured=false;
  const send = async (number, message) => {
    if(process.env.VISUALIZATION_LIVE_REASONING_EFFORT==='low'&&!reasoningConfigured){
      await page.getByRole('button',{name:'推理力度',exact:true}).click();await page.locator('[data-composer-popover]').getByRole('button').filter({has:page.getByText('低',{exact:true})}).click();
      await until(async()=>{const model=await call('GET',`/api/v1/agents/${encodeURIComponent(gatewayReady.agent)}/model`);return model.reasoning_effort==='low';},'isolated evaluation reasoning setting');reasoningConfigured=true;await persist('reasoning-setting.json',{reasoning_effort:'low',scope:'isolated evaluation agent through GUI',global_model_config_changed:false,output_limit_changed:false});
    }
    if (message !== undefined) await page.getByPlaceholder('输入消息…').fill(message);
    let oldIds,oldToolIds;
    const history=async()=>{const state=await call('GET','/api/v1/state'),actor=state.agents.find(a=>a.path===gatewayReady.agent);assert.ok(actor);const session=state.contexts[gatewayReady.agent]?.id;const messages=session?await call('GET',`/api/v1/agents/${actor.agent_id}/sessions/${session}/history`):[];for(const block of messages.flatMap(message=>message.content)){if(block.type==='tool_use'&&!seenGatewayTools.has(block.id)){seenGatewayTools.add(block.id);console.log(`LIVE_GATEWAY_TOOL_CALL=${block.name} action=${block.input?.action??''}`);}}return {actor,messages};};
    if(gatewayMode){const before=await history();oldIds=new Set(before.messages.map(m=>m.id));oldToolIds=new Set(before.messages.flatMap(m=>m.content).filter(b=>b.type==='tool_use').map(b=>b.id));}
    const timingSequence=gatewayMode?(await readLiveProgress(page))?.last_seq??0:0;
    const sentAt=Date.now();
    if(gatewayMode) await persist(`turn-${number}-started.json`,{sent_at:new Date(sentAt).toISOString(),provider,model,agent:gatewayReady.agent,host_pid:native.pid,observer_pid:process.pid});
    await page.getByRole('button', { name: '发送', exact: true }).click();
    if(gatewayMode){let finished,observedBusy=false,lastProgressAt=0;await observeLiveAgent({poll:async()=>{if(Date.now()-lastProgressAt>=30000){await persist(`turn-${number}-live-progress.json`,{elapsed_ms:Date.now()-sentAt,terminal:false,stream_progress:await readLiveProgress(page)});lastProgressAt=Date.now();}const current=await history();const status=String(current.actor.status).toUpperCase();if(['ERROR','CANCELLED'].includes(status)) {await persist('session-history.json',JSON.parse(JSON.stringify(current.messages,(key,value)=>key==='data'&&typeof value==='string'?'[IMAGE BYTES OMITTED]':value)));await persist(`turn-${number}-failure.json`,{status,terminal_stop_reason:current.messages.filter(m=>m.role==='assistant').at(-1)?.stop_reason??null,visible_api_errors:await page.getByText(/^ApiClient request error:/).evaluateAll(nodes=>nodes.filter(n=>n.getClientRects().length>0).map(n=>(n.textContent??'').slice(0,4000)))});throw new Error(`production agent turn ${number} ended with ${status}; terminal history retained`);}if(status!=='IDLE'){observedBusy=true;return false;}if(!observedBusy&&!current.messages.some(m=>!oldIds.has(m.id)&&m.role==='assistant'))return false;finished=current.messages;return true;},isAlive:()=>native.exitCode===null&&native.signalCode===null,windowMs:turnTimeout,intervalMs:1000,onWindow:async evidence=>{console.log(`LIVE_OBSERVATION_RENEWED turn=${number} elapsed_ms=${evidence.elapsed_ms}`);await persist(`turn-${number}-observation.json`,{...evidence,terminal:false,stream_progress:await readLiveProgress(page)});},onRetry:async evidence=>{await persist(`turn-${number}-transport-retry.json`,evidence);}});
      // Preserve terminal evidence even when the model returns no final text.
      const redactedHistory=JSON.parse(JSON.stringify(finished,(key,value)=>key==='data'&&typeof value==='string'?'[IMAGE BYTES OMITTED]':value));
      await persist('session-history.json',redactedHistory);
      const finalAssistant=finished.filter(m=>!oldIds.has(m.id)&&m.role==='assistant').at(-1),expectedReply=(finalAssistant?.content??[]).filter(b=>b.type==='text').map(b=>b.text).join('\n');
      assert.ok(expectedReply.trim(),`production agent ended without final text; stop_reason=${finalAssistant?.stop_reason??'missing'}; terminal blocks: ${(finalAssistant?.content??[]).map(b=>b.type).join(',')}`);
      const displayed=page.locator('[data-agent-workspace]:visible').getByRole('button',{name:'复制助手回复',exact:true}).last();
      await until(async()=>{if(!await displayed.count())return false;await displayed.click();return await page.evaluate(()=>navigator.clipboard.readText())===expectedReply;},'actual final assistant reply matches session history through production SSE');
      const toolErrors=new Map(finished.flatMap(m=>m.content).filter(b=>b.type==='tool_result').map(b=>[b.tool_use_id,Boolean(b.is_error)]));const calls=finished.flatMap(m=>m.content).filter(b=>b.type==='tool_use'&&!oldToolIds.has(b.id)).map(b=>({name:b.name,input:b.input,is_error:toolErrors.get(b.id)??false}));assert.ok(calls.length,'actual session records registered tool calls');const timing={sent_at:new Date(sentAt).toISOString(),completed_at:new Date().toISOString(),elapsed_ms:Date.now()-sentAt};const progress=await readLiveProgress(page);await persist(`turn-${number}-stream-progress.json`,progress);await persist(`turn-${number}-timing.json`,summarizeLiveTiming(progress,{afterSequence:timingSequence}));turns.push({calls,...timing});await persist(`turn-${number}.json`,{calls,...timing,transport:'production_gateway'});
      const redacted=JSON.parse(JSON.stringify(finished,(key,value)=>key==='data'&&typeof value==='string'?'[IMAGE BYTES OMITTED]':value));await persist('session-history.json',redacted);
    }
    await until(() => turns.length >= number || errors.length, `live turn ${number} completion`, turnTimeout); assert.deepEqual(errors, []);
    console.log(`LIVE_CHECKPOINT turn=${number}`);
  };
  const handoff = async request => {
    await page.getByRole('button', { name: 'Agent 绘图', exact: true }).click();
    if(process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE){const preference=process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE;assert.ok(['auto','compact','horizontal','vertical'].includes(preference));await page.getByRole('combobox',{name:'构图偏好',exact:true}).selectOption(preference);}
    const compositionRequest=process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE==='compact'?`${request} 本次紧凑构图目标：使用明确的视觉分面，整图宽高比在 0.5 到 2.5 之间，文字字号至少 16，不通过删改科学标签、关系或来源来压缩。科学范围明确后再完成布局计划。`:request;
    await page.getByRole('textbox', { name: '绘图需求' }).fill(compositionRequest);
    await page.getByRole('button', { name: '交给 Agent', exact: true }).click(); await page.getByPlaceholder('输入消息…').waitFor();
    if(process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE){const draft=await page.getByPlaceholder('输入消息…').inputValue();const context=await readCanvasPreparedContext(page);assert.equal(context.composition_preferences.layout_preference,process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE);await persist('layout-handoff.json',{requested:process.env.VISUALIZATION_LIVE_LAYOUT_PREFERENCE,draft});}
  };
  const normalized = text => text.replace(/\s/g, '');
  const preview = page.getByRole('dialog', { name: 'Agent 修改预览' });
  if(planReviewSource){
    const evidence=await runLivePlanReview({page,call,path,send,persist,directory,turns,until,source:planReviewSource});assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:'live_plan_label_review',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});console.log(`canvas-live: plan label review passed; artifacts=${directory}`);
  }else if(liveReference){
    const evidence=await runLiveReference({page,call,path,handoff,send,persist,directory,turns,until});assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:'live_reference_reconstruction',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});console.log(`canvas-live: reference reconstruction passed; artifacts=${directory}`);
  }else if(ifngArtSource){
    const evidence=await runLiveIfngArt({page,call,path,send,persist,directory,turns,until,source:ifngArtSource});assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:'live_ifng_art',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});console.log(`canvas-live: Agent IFNG illustration passed; artifacts=${directory}`);
  }else if(sourceReviewSource){
    const evidence=await (process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='apoptosis-materialize'?runLiveApoptosisMaterialize:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='crop-repair'?runLiveCropRepair:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='style-author'?runLiveStyleAuthor:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='frame-pack'?runLiveFramePack:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='checkpoint-resume'?runLiveCheckpointResume:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='svg-refit'?runLiveSvgRefit:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='stroke-width'?runLiveStrokeWidth:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='caption-position'?runLiveCaptionPosition:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='batch-search'?runLiveBatchSearch:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='frame-position'?runLiveFramePosition:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='painted-review'?runLivePaintedReview:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='materialization-role-repair'?runLiveMaterializationRoleRepair:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='whole-materialization'?runLiveWholeMaterialization:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='circle-annotation'?runLiveCircleAnnotation:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='caption-width'?runLiveCaptionWidth:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='generic-protein'?runLiveGenericProtein:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='ras-search'?runLiveRasSearch:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='caption-column'?runLiveCaptionColumn:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='frame-swap'?runLiveFrameSwap:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='frame-fit'?runLiveFrameFit:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='egfr-residue-labels'?runLiveResidueReview:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='candidate-position'?runLiveCandidatePosition:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='markup-position'?runLiveMarkupPosition:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='relation-readonly'?runLiveRelationReview:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='apoptosis-context-art'?runLiveContextArtwork:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='formal-scoped-retrieval-scope'?runLiveFormalSourceReview:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='connector-routing'?runLiveRouteReview:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='egfr-conflict'?runLiveReviewedRepair:runLiveSourceReview)({page,call,path,send,persist,directory,turns,until,source:sourceReviewSource,handoff,...(sourceReviewSpec?{reviewSpec:sourceReviewSpec}:{}),...(['retrieval-scope','scoped-retrieval-scope','gui-scoped-retrieval-scope'].includes(process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE)?{reviewSpec:{...retrievalScopeReview,scopedRead:['scoped-retrieval-scope','gui-scoped-retrieval-scope'].includes(process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE),guiScoped:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='gui-scoped-retrieval-scope'}}:{})});assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='egfr-residue-labels'?'live_reviewed_residue_labels':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='candidate-position'?'live_gui_candidate_attachment_position':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='markup-position'?'live_gui_markup_position':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='relation-readonly'?'live_historical_relation_readonly_review':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='connector-routing'?'live_connector_route_review':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='apoptosis-context-art'?'live_context_artwork':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='formal-scoped-retrieval-scope'?'live_formal_scoped_caption_review':'live_source_caption_review',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});console.log(`canvas-live: Agent ${process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='circle-annotation'?'GUI circle object annotation':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='generic-protein'?'explicit generic protein concept':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='ras-search'?'read-only RAS catalogue search':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='caption-column'?'compact caption column':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='frame-swap'?'reviewed frame swap':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='caption-width'?'reviewed arrow caption width':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='frame-fit'?'reviewed frame fit':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='egfr-residue-labels'?'reviewed residue labels':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='candidate-position'?'GUI candidate attachment position':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='markup-position'?'GUI markup position':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='connector-routing'?'connector-route review':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='apoptosis-context-art'?'contextual artwork':'source-caption review'} passed; artifacts=${directory}`);
  }else if(liveEvidence){
    const evidence=await (liveEvidenceWorkflow?runLiveEvidenceWorkflow:runLiveEvidencePlanning)({page,call,path,handoff,send,persist,directory,turns,until,...(!liveEvidenceWorkflow?{scenario:evidenceCase==='tgfb-smad'?tgfbScenario:evidenceCase==='intrinsic-apoptosis'?apoptosisScenario:evidenceCase==='egfr-mapk'?egfrScenario:undefined}:{})});assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    let contextualArtwork;
    if(process.env.VISUALIZATION_LIVE_CONTEXT_ART==='1'){await persist('fresh-evidence-stage.json',evidence);contextualArtwork=await runLiveContextArtwork({page,call,path,send,persist,directory,turns,until,source:null});assert.deepEqual(errors,[]);}
    let fullFigure;
    if(process.env.VISUALIZATION_LIVE_FULL_MATERIALIZATION==='1'){
      await persist('fresh-evidence-stage.json',evidence);
      const materialization=await runLiveApoptosisMaterialize({page,call,path,send,persist,directory,turns,until,source:null,current:true});assert.equal(materialization.external_scene_imported,false);await persist('fresh-materialization-stage.json',materialization);
      const review=await finalizeFreshFigure({page,call,path,send,persist,directory,turns,materialization});assert.deepEqual(errors,[]);fullFigure={external_scene_imported:false,materialization,review};
      await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    }
    await persist('result.json',{passed:true,case:fullFigure?'live_fresh_apoptosis_full_figure':contextualArtwork?'live_fresh_apoptosis_context_art':liveEvidenceWorkflow?'live_evidence_workflow':'live_evidence_planning',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence,...(contextualArtwork?{contextual_artwork:contextualArtwork}:{} ),...(fullFigure?{fresh_full_figure:fullFigure}:{})});console.log(`canvas-live: Agent ${liveEvidenceWorkflow ? "continuous evidence workflow" : "source planning"} passed; artifacts=${directory}`);
  }else if(liveCompose){
    const evidence=await runLiveComposition({page,call,path,send,persist,directory,turns,until});assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:'live_composition',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});console.log(`canvas-live: Agent composition passed; artifacts=${directory}`);
  }else if(liveSvg){
    const options={page,call,path,send,persist,directory,turns,until,source:fitSource,bioart:bioartSvg,pathHandoff,candidateHandoff:candidatePathHandoff,sharedContext:sharedPathContext};
    const evidence=visibleSvg?await runLiveSvgVisibleRepair(options):onlineSvg?await runLiveBioartSvgAcquisition(options):await runLiveSvgRefinement(options);assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:visibleSvg?(pathHandoff?(candidatePathHandoff?(sharedPathContext?'live_shared_svg_path_context':'live_candidate_svg_path_handoff'):'live_svg_path_handoff'):'live_svg_visible_repair'):onlineSvg?'live_bioart_svg_online':'live_svg_refinement',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});console.log(`canvas-live: Agent SVG refinement passed; artifacts=${directory}`);
  }else if(liveFit){
    assert.ok(Number.isSafeInteger(gatewayReady.max_output_tokens)&&gatewayReady.max_output_tokens>0,'rebuild the configured-model host; output-limit metadata is required');
    await persist('fit-model-settings.json',{provider,model,max_output_tokens:gatewayReady.max_output_tokens});
    const evidence=await runLiveIllustrationFit({page,call,path,send,persist,directory,turns,until,source:fitSource});assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:'live_illustration_fit',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});console.log(`canvas-live: Agent illustration fit passed; artifacts=${directory}`);
  }else if(materializationSource){
    const evidence=await runCandidateMaterialization({page,call,path,handoff,send,persist,directory,turns,until,source:materializationSource});assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:'candidate_materialization',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});console.log(`canvas-live: materialization passed; artifacts=${directory}`);
  }else if(scientificCase){
    const spec=JSON.parse(await readFile(scientificCase,'utf8'));await persist('scientific-case.json',spec);
    const evidence=await runScientificCase({page,call,path,handoff,send,persist,directory,turns,until,spec});assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:'scientific_case',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});console.log(`canvas-live: scientific case passed; artifacts=${directory}`);
  }else{
  let image,receptorLabel,receptor;
  if (!reviewSource) {
  await handoff('我想画一张受体信号传导示意图，但还没有指定受众、具体受体和需要表达的终点。请先和我澄清图意并保存待讨论的绘图计划。这一轮先提问，不要擅自选择具体受体、分子或通路，也不要先画图。');
  await send(1);
  const first = await call('GET', `${path}/planning`); await persist('plan-stage-1.json', first);
  assert.ok(first && first.status === 'needs_clarification'); assert.ok(first.document.questions.length > 0, 'real model saves focused questions');
  assert.ok(first.document.questions.every(question => !question.answer.trim()), 'model does not answer its own clarification questions');
  assert.equal((await call('GET', `${path}/proposals`)).length, 0); const original = await call('GET', path); assert.equal(original.scene.document.elements.length, 0);
  checkpoints.push('saved unanswered clarification without drawing');
  await send(2, '受众为免疫学研究者。只展示三个抽象组件：“配体 L”“受体 R”“响应 X”，关系为 L→R（结合）和 R→X（激活），水平排列。这是概念示意，不代表已验证的真实通路，不要补入其他分子。请把这些答案记录进已有计划，保留问题和答案历史，并根据这些信息生成待审阅草图，不要直接改正式图稿。');
  const ready = await call('GET', `${path}/planning`); await persist('plan-stage-2.json', ready);
  assert.equal(ready.issues.length, 0); assert.ok(ready.version > first.version);
  for (const question of first.document.questions) assert.ok(ready.document.questions.some(q => q.id === question.id && q.answer.trim()), `preserve and answer ${question.id}`);

  assert.deepEqual(ready.document.nodes.map(node => normalized(node.label)).sort(), ['受体R', '响应X', '配体L'].sort());
  const labels = Object.fromEntries(ready.document.nodes.map(node => [node.id, normalized(node.label)]));
  assert.deepEqual(ready.document.edges.map(edge => `${labels[edge.from]}→${labels[edge.to]}`).sort(), ['受体R→响应X', '配体L→受体R'].sort());
  assert.ok(ready.document.assumptions.some(item => /概念|conceptual|未验证|not.*validated/i.test(item)), 'record the conceptual status');
  const proposals = (await call('GET', `${path}/proposals`)).filter(p => p.status === 'pending'); assert.equal(proposals.length, 1);
  assert.equal((await call('GET', path)).metadata.revision, original.metadata.revision, 'real model does not directly commit artwork');
  const proposal = proposals[0]; await persist('proposal-stage-2.json', proposal);
  assert.equal(proposal.document.elements.filter(e => e.type === 'arrow').length, 2); assert.equal(proposal.document.elements.filter(e => e.type === 'text').length, 5);
  checkpoints.push('absorbed supplied answers and proposed exactly the supplied nodes and relations');
  await page.getByRole('tab', { name: 'Live figure', exact: true }).click(); await page.getByRole('button', { name: '绘图计划', exact: true }).click();
  const planning = page.getByRole('dialog', { name: '科研绘图计划' }); await planning.getByRole('textbox', { name: '核心图意' }).waitFor();
  await page.screenshot({ path: join(directory, 'planning.png'), animations: 'disabled' }); await planning.getByRole('button', { name: '生成草图提案', exact: true }).click();
  await preview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
  await page.screenshot({ path: join(directory, 'proposal.png'), animations: 'disabled' }); await preview.getByRole('button', { name: '采用提案', exact: true }).click(); await preview.waitFor({ state: 'hidden' });
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const beforeEdit = await call('GET', path); await persist('adopted-stage-2.json', beforeEdit);
  await handoff('请先查看当前同版本整图预览，把“响应 X”标签改为“细胞响应 X”。只改这一处文字，保留其他所有组件、文字、连线和位置。请提交局部修改提案，不要直接提交正式版本。');
  await send(3);
  assert.ok(turns[2].calls.some(call => call.name === 'visualization' && call.input.action === 'read_preview'), 'real model requests the saved full-canvas vision result');
  const edits = (await call('GET', `${path}/proposals`)).filter(p => p.status === 'pending'); assert.equal(edits.length, 1); const edit = edits[0]; await persist('proposal-stage-3.json', edit);
  const oldElements = beforeEdit.scene.document.elements, newElements = edit.document.elements;
  const changed = newElements.filter(e => JSON.stringify(e) !== JSON.stringify(oldElements.find(old => old.id === e.id)));
  assert.equal(changed.length, 1, 'model preserves every unrelated persisted object'); assert.equal(changed[0].type, 'text'); assert.equal(normalized(changed[0].text), '细胞响应X');
  assert.equal(newElements.length, oldElements.length); assert.equal((await call('GET', path)).metadata.revision, beforeEdit.metadata.revision);
  checkpoints.push('read real canvas preview and proposed only the requested label change');
  await page.getByRole('tab', { name: 'Live figure', exact: true }).click(); await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
  await preview.getByRole('button', { name: new RegExp(edit.summary.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).click();
  await preview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor(); await page.screenshot({ path: join(directory, 'local-edit.png'), animations: 'disabled' });
  await preview.getByRole('button', { name: '采用提案', exact: true }).click(); await preview.waitFor({ state: 'hidden' }); await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const beforeIllustration = await call('GET', path); await persist('before-bioart.json', beforeIllustration);
  await handoff('请为“受体 R”寻找合适的 NIH BioArt 通用膜受体示意素材。保持概念性，不要把它当作某个特定已验证蛋白或补写机制。先检索并预览图片，核对含义和来源许可；找到合适素材后，用替换提案升级受体占位形状，保留原有标签、连线和其他对象。不要直接插入正式图稿。如果没有合适素材，请解释原因并保留占位形状。');
  await send(4);
  const assetCalls = turns[3].calls.filter(call => call.name === 'bioart');
  assert.ok(assetCalls.some(call => call.input.action === 'search'), 'real agent searches the live NIH catalog');
  const replacement = assetCalls.findLast(call => call.input.action === 'replace'); assert.ok(replacement, 'real agent proposes a visual replacement');
  assert.ok(assetCalls.some(call => call.input.action === 'preview' && call.input.asset_id === replacement.input.asset_id), 'preview the same asset before replacing the placeholder');
  const assetEdits = (await call('GET', `${path}/proposals`)).filter(p => p.status === 'pending'); assert.equal(assetEdits.length, 1);
  const illustrated = assetEdits[0]; await persist('proposal-stage-4.json', illustrated);
  image = illustrated.document.elements.find(element => element.type === 'image'); assert.ok(image);
  assert.equal(image.customData.asset.id, replacement.input.asset_id); assert.ok(image.customData.asset.licenses.length > 0);
  assert.ok(image.customData.asset.source_url.startsWith('https://bioart.niaid.nih.gov/'));
  assert.ok(illustrated.document.files[image.fileId].dataURL.startsWith('data:image/png;base64,'));
  receptorLabel = beforeIllustration.scene.document.elements.find(element => element.type === 'text' && normalized(element.text) === '受体R'); assert.ok(receptorLabel);
  receptor = beforeIllustration.scene.document.elements.find(element => element.id === receptorLabel.containerId); assert.ok(receptor);
  for (const element of beforeIllustration.scene.document.elements.filter(e => ![receptor.id, receptorLabel.id].includes(e.id))) {
    assert.deepEqual(illustrated.document.elements.find(next => next.id === element.id), element, `asset replacement preserves unrelated ${element.id}`);
  }
  assert.equal(normalized(illustrated.document.elements.find(e => e.id === receptorLabel.id).text), '受体R');
  assert.equal((await call('GET', path)).metadata.revision, beforeIllustration.metadata.revision);
  checkpoints.push('searched and visually previewed a live licensed NIH asset, then proposed a localized replacement');
  await page.getByRole('tab', { name: 'Live figure', exact: true }).click(); await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
  await preview.getByRole('button', { name: new RegExp(illustrated.summary.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).click();
  await preview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor(); await page.screenshot({ path: join(directory, 'bioart-proposal.png'), animations: 'disabled' });
  await preview.getByRole('button', { name: '采用提案', exact: true }).click(); await preview.waitFor({ state: 'hidden' }); await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  } else {
    const artifact=JSON.parse(await readFile(reviewSource,'utf8')),document=artifact.scene?.document ?? artifact.document ?? artifact;
    assert.ok(Array.isArray(document.elements)&&document.files,'review scene must be an editable saved document with files');
    const head=await call('GET',path);
    await call('PUT',path,{expected_revision:head.metadata.revision,title:'Live figure',document});
    page.once('dialog',dialog=>dialog.accept());
    await page.getByRole('button',{name:'重新加载服务端版本',exact:true}).click();
    const seededDownload=page.waitForEvent('download');await page.getByRole('button',{name:'导出 源文件',exact:true}).click();const seededScene=JSON.parse(await readFile(await (await seededDownload).path(),'utf8'));assert.ok(seededScene.elements.some(e=>e.type==='image'),'editor loaded the imported scene');
    await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
    const imported=await call('GET',path);image=imported.scene.document.elements.find(e=>e.type==='image'&&!e.isDeleted);assert.ok(image,'isolated case requires the illustrated receptor fixture');
    receptorLabel=imported.scene.document.elements.find(e=>e.type==='text'&&normalized(e.text)==='受体R');assert.ok(receptorLabel);
    receptor=imported.scene.document.elements.find(e=>e.id===image.customData?.semanticAnchorId);assert.ok(receptor);
    assert.equal(image.customData?.semanticAnchorId,receptor.id,'reference image belongs to the marked receptor');assert.ok(receptor.customData?.visualLabelIds?.includes(receptorLabel.id),'label belongs to illustrated semantic anchor');
    await persist('seed-scene.json',imported);checkpoints.push('imported an explicit prior illustrated scene into a separate test database; no earlier model turns were replayed');
  }
  if(candidateMode){
    const evidence=await runCandidateRevision({page,call,path,send,persist,directory,turns,until});
    assert.deepEqual(errors,[]);assert.ok(sseResponses>0);
    await persist('candidate-evidence.json',evidence);await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});
    await persist('result.json',{passed:true,case:'candidate_revision',transport:'production_gateway',provider,model,project,figure,turns:turns.length,...evidence});
    console.log(`canvas-live: candidate revision checks passed; artifacts=${directory}`);
  }else{
  const beforeMarkup = await call('GET', path); await persist('before-markup.json', beforeMarkup);
  await page.screenshot({ path: join(directory, 'illustrated-figure.png'), animations: 'disabled' });
  const beforeElements = beforeMarkup.scene.document.elements.filter(e => !e.isDeleted);
  await page.getByRole('button', { name: '草图标记', exact: true }).click();
  const markup = page.getByRole('dialog', { name: '草图标记与审阅' });
  await markup.getByRole('button', { name: '新建标记', exact: true }).click();
  const ink = markup.getByRole('application', { name: '草图标记区域' }); await ink.waitFor();
  await markup.getByRole('button', { name: '保存草图标记', exact: true }).waitFor();
  await markup.getByRole('button', { name: '划除', exact: true }).click();
  const bounds = await ink.boundingBox();
  // This compact horizontal scene fits at zoom 1. Validate the saved scene-space
  // stroke below so viewport timing or fitting errors cannot produce a false pass.
  const minX = Math.min(...beforeElements.map(e => e.x)), maxX = Math.max(...beforeElements.map(e => e.x + e.width));
  const minY = Math.min(...beforeElements.map(e => e.y)), maxY = Math.max(...beforeElements.map(e => e.y + e.height));
  const currentImage = beforeElements.find(e => e.id === image.id); assert.ok(currentImage);
  const centerX = currentImage.x + currentImage.width / 2, centerY = currentImage.y + currentImage.height / 2;
  const inkX = bounds.x + bounds.width / 2 + centerX - (minX + maxX) / 2;
  const inkY = bounds.y + bounds.height / 2 + centerY - (minY + maxY) / 2;
  await page.mouse.move(inkX - 45, inkY - 65); await page.mouse.down();
  await page.mouse.move(inkX + 45, inkY + 65, { steps: 10 }); await page.mouse.up();
  await markup.getByRole('textbox', { name: '标记修改说明' }).fill('移除划掉的组件及其直接连接和关系文字；保留其余组件及位置，不补画任何新关系。');
  await markup.getByRole('button', { name: '保存草图标记', exact: true }).click();
  await markup.getByRole('img', { name: '已保存的草图标记' }).waitFor();
  const reviews = await call('GET', `${path}/markups`); assert.equal(reviews.length, 1);
  const review = await call('GET', `${path}/markups/${reviews[0].id}`); await persist('markup-stage-5.json', review);
  assert.equal(review.at_revision, beforeMarkup.metadata.revision); assert.equal(review.strokes.length, 1);
  const [a, b] = review.strokes[0].points;
  assert.ok(Math.min(a[0], b[0]) < centerX && Math.max(a[0], b[0]) > centerX && Math.min(a[1], b[1]) < centerY && Math.max(a[1], b[1]) > centerY, 'GUI cross covers the receptor image in scene coordinates');
  await page.screenshot({ path: join(directory, 'markup.png'), animations: 'disabled' });
  await markup.getByRole('button', { name: '关闭标记', exact: true }).click();
  if (prepareOnly) {
    assert.equal(turns.length,0);assert.deepEqual(errors,[]);
    await persist('result.json',{prepared:true,passed:false,model_evaluated:false,case:'isolated_markup',project,figure,checkpoints,review_id:review.id});
    console.log(`canvas-live: isolated markup prepared without model inference; artifacts=${directory}`);
    process.exitCode=0;
  } else {
  await handoff('请查看本次保存的草图标记截图，结合笔迹和修改说明识别应移除的组件，生成局部修改提案。不要直接改正式图稿，不要推断新的科学关系。');
  await send(reviewSource ? 1 : 5);
  assert.ok(turns[reviewSource ? 0 : 4].calls.some(c => c.name === 'visualization' && c.input.action === 'read_markup' && c.input.markup_id === review.id), 'real agent reads the exact review as a vision result');
  const markupEdits = (await call('GET', `${path}/proposals`)).filter(p => p.status === 'pending'); assert.equal(markupEdits.length, 1);
  const deletion = markupEdits[0]; await persist('proposal-stage-5.json', deletion);
  assert.deepEqual(deletion.review_markups.map(item=>item.id),[review.id], 'candidate explicitly links only the addressed review');
  assert.equal((await call('GET', `${path}/markups/${review.id}`)).status,'open', 'proposing does not resolve the review');
  assert.deepEqual(deletion.document.files,beforeMarkup.scene.document.files, 'review edit preserves source image bytes');
  const kept = beforeElements.filter(e => ![receptor.id, receptorLabel.id, image.id].includes(e.id) && e.type !== 'arrow' && !(e.type === 'text' && beforeElements.some(arrow => arrow.type === 'arrow' && arrow.id === e.containerId)));
  assert.equal(kept.length, 4); assert.equal(deletion.document.elements.length, kept.length);
  const removedIds = new Set(beforeElements.filter(e => !kept.some(k => k.id === e.id)).map(e => e.id));
  for (const element of kept) {
    const expected = structuredClone(element);
    if (expected.boundElements) expected.boundElements = expected.boundElements.filter(binding => !removedIds.has(binding.id));
    assert.deepEqual(deletion.document.elements.find(e => e.id === element.id), expected, `review edit preserves surviving ${element.id}`);
  }
  assert.equal((await call('GET', path)).metadata.revision, beforeMarkup.metadata.revision);
  checkpoints.push('interpreted a visual cross and proposed removing only its component and explicit relationships');
  await page.getByRole('tab', { name: 'Live figure', exact: true }).click(); await page.getByRole('button', { name: '查看 Agent 提案', exact: true }).click();
  await preview.getByRole('button', { name: `${deletion.summary} · v${deletion.base_revision}`, exact: true }).click();
  await preview.getByRole('region', { name: '候选画布' }).locator('.excalidraw').waitFor();
  await page.screenshot({ path: join(directory, 'markup-proposal.png'), animations: 'disabled' });
  await preview.getByRole('checkbox',{name:'采用时同时完成关联标记（1 组）',exact:true}).check();
  await preview.getByRole('button', { name: '采用提案', exact: true }).click(); await preview.waitFor({ state: 'hidden' });
  await page.locator('[data-canvas-surface] .excalidraw:visible').waitFor();
  const adoptedReview=await call('GET',path), resolvedReview=await call('GET',`${path}/markups/${review.id}`), acceptedReviewProposal=await call('GET',`${path}/proposals/${deletion.id}`);
  assert.equal(adoptedReview.scene.document.elements.length,4);assert.equal(adoptedReview.metadata.revision,beforeMarkup.metadata.revision+1);
  assert.deepEqual(adoptedReview.scene.document,deletion.document);assert.equal(acceptedReviewProposal.status,'accepted');assert.equal(resolvedReview.status,'resolved');
  assert.equal(resolvedReview.at_revision,review.at_revision);assert.deepEqual(resolvedReview.strokes,review.strokes);
  await persist('resolved-review.json',resolvedReview);await persist('accepted-review-proposal.json',acceptedReviewProposal);
  checkpoints.push('GUI adopted the exact candidate and explicitly resolved the linked review without altering its original strokes');
  await page.getByRole('button', { name: '保存画布', exact: true }).waitFor();
  const download = page.waitForEvent('download'); await page.getByRole('button', { name: '导出 SVG', exact: true }).click(); await writeFile(join(directory, 'figure.svg'), await readFile(await (await download).path()));
  await page.screenshot({ path: join(directory, 'figure.png'), animations: 'disabled' }); await persist('final.json', await call('GET', path));
  if(!gatewayMode){
  const auditedRequests=(await readFile(join(directory,'trace.requests.jsonl'),'utf8')).trim().split('\n').map(line=>JSON.parse(line));
  const visionCallCount=turns.flatMap(turn=>turn.calls).filter(call=>(call.name==='visualization'&&['read_image','read_preview','read_proposal_preview','read_markup'].includes(call.input.action))||(call.name==='bioart'&&call.input.action==='preview')).length;
  assert.ok(auditedRequests.length>0);assert.ok(auditedRequests.some(request=>request.images>0),'real model receives vision images');
  assert.ok(auditedRequests.every(request=>request.images<=visionCallCount),'request images cannot multiply beyond actual image-producing tool calls');
  await persist('request-audit.json',{requests:auditedRequests.length,vision_tool_calls:visionCallCount,max_images:Math.max(...auditedRequests.map(request=>request.images)),contains_payloads:false});
  }else{assert.ok(sseResponses>0,'actual SSE stream delivered runtime events');await persist('gateway-transport.json',{authenticated:true,sse_responses:sseResponses,turns:turns.length,model_responses_scripted:false,api_routes_mocked:false});}
  assert.deepEqual(errors, []); await persist('result.json', { passed: true, transport:gatewayMode?'production_gateway':'native_test_host', provider, model, project, figure, checkpoints, turns: turns.length, case:reviewSource?'isolated_markup':'full_workflow' });
  console.log(`canvas-live: all checks passed; artifacts=${directory}`);
  }
  }
  }
} catch (error) {
  // Browser failure must not erase the authoritative Agent state/history.
  if(gatewayMode&&gatewayReady&&native.exitCode===null&&native.signalCode===null){
    try{
      await observeLiveAgent({
        poll:async()=>{const status=await retainAgentEvidence({origin,token:gatewayReady.token,agent:gatewayReady.agent,persist});return ['IDLE','ERROR','CANCELLED'].includes(String(status).toUpperCase());},
        isAlive:()=>native.exitCode===null&&native.signalCode===null,
        windowMs:turnTimeout,intervalMs:5000,
        onWindow:async evidence=>{await persist('failure-agent-observation.json',{...evidence,terminal:false});console.log(`LIVE_FAILURE_OBSERVATION_RENEWED elapsed_ms=${evidence.elapsed_ms}`);},
      });
    }catch{await persist('failure-evidence-unavailable.json',{checked_at:new Date().toISOString(),reason:'independent Gateway evidence retrieval failed'});}
  }
  if(gatewayMode&&gatewayReady&&figureEvidencePath&&native.exitCode===null&&native.signalCode===null){
    try{await retainFigureEvidence({origin,token:gatewayReady.token,figurePath:figureEvidencePath,persist});}
    catch{await persist('failure-figure-evidence-unavailable.json',{checked_at:new Date().toISOString(),reason:'independent figure snapshot retrieval failed'});}
  }
  await page?.screenshot({ path: join(directory, 'failure.png'), animations: 'disabled' }).catch(() => {});
  await persist('result.json', { passed: false, case:process.env.VISUALIZATION_LIVE_FULL_MATERIALIZATION==='1'?'live_fresh_apoptosis_full_figure':planReviewSource?'live_plan_label_review':liveReference?'live_reference_reconstruction':ifngArtSource?'live_ifng_art':sourceReviewSource?(process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='apoptosis-materialize'?'live_apoptosis_materialization':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='crop-repair'?'live_crop_repair':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='style-author'?'live_style_author':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='frame-pack'?'live_frame_pack':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='candidate-position'?'live_gui_candidate_attachment_position':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='markup-position'?'live_gui_markup_position':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='connector-routing'?'live_connector_route_review':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='apoptosis-context-art'?'live_context_artwork':process.env.VISUALIZATION_LIVE_SOURCE_REVIEW_CASE==='formal-scoped-retrieval-scope'?'live_formal_scoped_caption_review':'live_source_caption_review'):liveEvidenceWorkflow?'live_evidence_workflow':liveEvidence?'live_evidence_planning':liveCompose?'live_composition':liveSvg?(visibleSvg?(pathHandoff?(candidatePathHandoff?(sharedPathContext?'live_shared_svg_path_context':'live_candidate_svg_path_handoff'):'live_svg_path_handoff'):'live_svg_visible_repair'):onlineSvg?'live_bioart_svg_online':'live_svg_refinement'):liveFit?'live_illustration_fit':materializationSource?'candidate_materialization':scientificCase?'scientific_case':candidateMode?'candidate_revision':reviewSource?'isolated_markup':'full_workflow', provider, model, checkpoints, error: String(error), errors });
  console.error(`canvas-live: failed; artifacts=${directory}`); throw error;
} finally {
  await writeFile(join(directory, 'host.log'), output, { mode: 0o600 }); await page?.unrouteAll({behavior:'ignoreErrors'}).catch(()=>{}); await browser?.close(); if(gatewayMode&&native.exitCode===null&&native.signalCode===null)await writeFile(join(directory,'stop'),'stop');else native.kill(); vite?.kill();
  await Promise.all([native, vite].filter(Boolean).map(child => child.exitCode !== null || child.signalCode !== null ? Promise.resolve() : new Promise(resolve => {const timer=setTimeout(()=>child.kill('SIGKILL'),10000);timer.unref();child.once('exit',()=>{clearTimeout(timer);resolve();});})));
}
