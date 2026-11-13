import assert from 'node:assert/strict';
import {evaluationConfig} from './helpers/canvas-live-config.mjs';
const base=evaluationConfig({SECRET_TOKEN:'not-for-artifacts'});
assert.equal(base.profile,'custom');assert.ok(!JSON.stringify(base).includes('not-for-artifacts'));
assert.throws(()=>evaluationConfig({VISUALIZATION_LIVE_PROFILE:'egfr-compact-bioart'}),/requires VISUALIZATION_LIVE_GATEWAY=1/);
const strict=Object.fromEntries(Object.keys(base.settings).map(key=>[key,'1']));
Object.assign(strict,{VISUALIZATION_LIVE_PROFILE:'egfr-compact-bioart',VISUALIZATION_LIVE_EVIDENCE_CASE:'egfr-mapk',VISUALIZATION_LIVE_ART_PROVIDER:'bioart',VISUALIZATION_LIVE_LAYOUT_PREFERENCE:'compact'});
delete strict.VISUALIZATION_LIVE_SECOND_MATERIAL;
delete strict.VISUALIZATION_LIVE_CONTEXT_ART;
delete strict.VISUALIZATION_LIVE_FULL_MATERIALIZATION;
delete strict.VISUALIZATION_LIVE_REASONING_EFFORT;
assert.equal(evaluationConfig(strict).profile,'egfr-compact-bioart');
for(const key of Object.keys(base.settings).filter(key=>!['VISUALIZATION_LIVE_REQUIRE_BATCH_EVIDENCE','VISUALIZATION_LIVE_SECOND_MATERIAL','VISUALIZATION_LIVE_EVIDENCE','VISUALIZATION_LIVE_CONTEXT_ART','VISUALIZATION_LIVE_FULL_MATERIALIZATION','VISUALIZATION_LIVE_REASONING_EFFORT'].includes(key))){const invalid={...strict};delete invalid[key];assert.throws(()=>evaluationConfig(invalid),new RegExp(`requires ${key}=`));}
assert.throws(()=>evaluationConfig({VISUALIZATION_LIVE_PROFILE:'unknown'}),/unknown evaluation profile/);


const ifng={...strict,VISUALIZATION_LIVE_PROFILE:'ifng-compact-bioart',VISUALIZATION_LIVE_EVIDENCE_CASE:'ifng'};
assert.equal(evaluationConfig(ifng).profile,'ifng-compact-bioart');
const noBatch={...ifng};delete noBatch.VISUALIZATION_LIVE_REQUIRE_BATCH_EVIDENCE;assert.throws(()=>evaluationConfig(noBatch),/requires VISUALIZATION_LIVE_REQUIRE_BATCH_EVIDENCE=1/);
assert.throws(()=>evaluationConfig({...ifng,VISUALIZATION_LIVE_EVIDENCE_CASE:'egfr-mapk'}),/requires VISUALIZATION_LIVE_EVIDENCE_CASE=ifng/);

console.log('evaluation config: strict EGFR/IFNG settings and required batch evidence cannot be omitted; manifests exclude unrelated environment values');

const multi={...ifng,VISUALIZATION_LIVE_PROFILE:'ifng-multi-bioart',VISUALIZATION_LIVE_SECOND_MATERIAL:'1'};
assert.equal(evaluationConfig(multi).profile,'ifng-multi-bioart');
assert.throws(()=>evaluationConfig({...multi,VISUALIZATION_LIVE_SECOND_MATERIAL:undefined}),/requires VISUALIZATION_LIVE_SECOND_MATERIAL=1/);
assert.throws(()=>evaluationConfig({...ifng,VISUALIZATION_LIVE_SECOND_MATERIAL:'1'}),/requires ifng-multi-bioart/);
assert.throws(()=>evaluationConfig({...multi,VISUALIZATION_LIVE_EVIDENCE_CASE:'egfr-mapk'}),/requires VISUALIZATION_LIVE_EVIDENCE_CASE=ifng/);

assert.throws(()=>evaluationConfig({VISUALIZATION_LIVE_SECOND_MATERIAL:'1'}),/requires the evidence workflow and bioart/);
assert.throws(()=>evaluationConfig({...multi,VISUALIZATION_LIVE_PROFILE:'custom',VISUALIZATION_LIVE_ART_PROVIDER:'bioicons'}),/requires the evidence workflow and bioart/);

const apoptosis={VISUALIZATION_LIVE_PROFILE:'apoptosis-compact-sketch',VISUALIZATION_LIVE_GATEWAY:'1',VISUALIZATION_LIVE_EVIDENCE:'1',VISUALIZATION_LIVE_EVIDENCE_WORKFLOW:'0',VISUALIZATION_LIVE_EVIDENCE_CASE:'intrinsic-apoptosis',VISUALIZATION_LIVE_LAYOUT_PREFERENCE:'compact',VISUALIZATION_LIVE_CONCISE_CAPTION:'1',VISUALIZATION_LIVE_REQUIRE_GEOMETRY_REVIEW:'1',VISUALIZATION_LIVE_REQUIRE_BATCH_EVIDENCE:'1',VISUALIZATION_LIVE_REQUIRE_SCALAR_TEXT:'1',VISUALIZATION_LIVE_REVIEW_UNREAD_SOURCES:'1'};
assert.equal(evaluationConfig(apoptosis).profile,'apoptosis-compact-sketch');
for(const key of Object.keys(apoptosis).filter(k=>k!=='VISUALIZATION_LIVE_PROFILE')){const invalid={...apoptosis};delete invalid[key];assert.throws(()=>evaluationConfig(invalid),new RegExp(`requires ${key}=`));}
assert.throws(()=>evaluationConfig({...apoptosis,VISUALIZATION_LIVE_EVIDENCE_WORKFLOW:'1'}),/requires VISUALIZATION_LIVE_EVIDENCE_WORKFLOW=0/);

const onlineSvg={VISUALIZATION_LIVE_SVG:'1',VISUALIZATION_LIVE_SVG_BIOART:'1',VISUALIZATION_LIVE_SVG_ONLINE:'1'};
assert.deepEqual(evaluationConfig(onlineSvg).svg_settings,{enabled:true,bioart:true,online:true,visible_repair:false,path_handoff:false,candidate_path_handoff:false,shared_path_context:false});
assert.throws(()=>evaluationConfig({...onlineSvg,VISUALIZATION_LIVE_FIT_SCENE:'fixture.json'}),/no imported scene/);
for(const key of ['VISUALIZATION_LIVE_SVG','VISUALIZATION_LIVE_SVG_BIOART'])assert.throws(()=>evaluationConfig({...onlineSvg,[key]:undefined}),/requires SVG\/BioArt/);
assert.throws(()=>evaluationConfig({VISUALIZATION_LIVE_SVG:'1',VISUALIZATION_LIVE_SVG_BIOART:'1'}),/explicit saved scene/);
assert.deepEqual(evaluationConfig({VISUALIZATION_LIVE_SVG:'1',VISUALIZATION_LIVE_SVG_BIOART:'1',VISUALIZATION_LIVE_FIT_SCENE:'explicit.json'}).svg_settings,{enabled:true,bioart:true,online:false,visible_repair:false,path_handoff:false,candidate_path_handoff:false,shared_path_context:false});

const visibleSvg={VISUALIZATION_LIVE_SVG:'1',VISUALIZATION_LIVE_SVG_BIOART:'1',VISUALIZATION_LIVE_SVG_VISIBLE:'1',VISUALIZATION_LIVE_FIT_SCENE:'explicit.json'};
assert.equal(evaluationConfig(visibleSvg).svg_settings.visible_repair,true);
assert.throws(()=>evaluationConfig({...visibleSvg,VISUALIZATION_LIVE_FIT_SCENE:undefined}),/explicit scene/);
assert.throws(()=>evaluationConfig({...visibleSvg,VISUALIZATION_LIVE_SVG_ONLINE:'1'}),/cannot run online/);

assert.equal(evaluationConfig({...visibleSvg,VISUALIZATION_LIVE_SVG_PATH_HANDOFF:'1'}).svg_settings.path_handoff,true);
assert.throws(()=>evaluationConfig({VISUALIZATION_LIVE_SVG_PATH_HANDOFF:'1'}),/requires visible SVG/);

assert.throws(()=>evaluationConfig({VISUALIZATION_LIVE_SVG_CANDIDATE_PATH:'1'}),/requires path handoff/);
assert.equal(evaluationConfig({...visibleSvg,VISUALIZATION_LIVE_SVG_PATH_HANDOFF:'1',VISUALIZATION_LIVE_SVG_CANDIDATE_PATH:'1'}).svg_settings.candidate_path_handoff,true);

assert.throws(()=>evaluationConfig({VISUALIZATION_LIVE_SVG_SHARED_CONTEXT:'1'}),/requires candidate path/);
assert.equal(evaluationConfig({...visibleSvg,VISUALIZATION_LIVE_SVG_PATH_HANDOFF:'1',VISUALIZATION_LIVE_SVG_CANDIDATE_PATH:'1',VISUALIZATION_LIVE_SVG_SHARED_CONTEXT:'1'}).svg_settings.shared_path_context,true);

const freshArt={...apoptosis,VISUALIZATION_LIVE_PROFILE:'apoptosis-context-art',VISUALIZATION_LIVE_CONTEXT_ART:'1'};
assert.equal(evaluationConfig(freshArt).profile,'apoptosis-context-art');
assert.throws(()=>evaluationConfig({...freshArt,VISUALIZATION_LIVE_CONTEXT_ART:undefined}),/requires VISUALIZATION_LIVE_CONTEXT_ART/);
for(const key of ['VISUALIZATION_LIVE_FIT_SCENE','VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE','VISUALIZATION_LIVE_REVIEW_SCENE','VISUALIZATION_LIVE_IFNG_ART_SCENE','VISUALIZATION_LIVE_MATERIALIZE_SCENE'])assert.throws(()=>evaluationConfig({...freshArt,[key]:'old.json'}),/forbids imported/);
assert.throws(()=>evaluationConfig({...freshArt,VISUALIZATION_LIVE_EVIDENCE_CASE:'ifng'}),/requires fresh apoptosis/);
assert.throws(()=>evaluationConfig({...freshArt,VISUALIZATION_LIVE_CANDIDATE:'1'}),/separate evaluation/);

const tgfb={...apoptosis,VISUALIZATION_LIVE_PROFILE:'tgfb-compact-sketch',VISUALIZATION_LIVE_EVIDENCE_CASE:'tgfb-smad'};
assert.equal(evaluationConfig(tgfb).profile,'tgfb-compact-sketch');
for(const key of Object.keys(tgfb).filter(k=>k!=='VISUALIZATION_LIVE_PROFILE')){const invalid={...tgfb};delete invalid[key];assert.throws(()=>evaluationConfig(invalid),new RegExp(`requires ${key}=`));}
assert.throws(()=>evaluationConfig({...tgfb,VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE:'old.json'}),/forbids imported/);

const fullFigure={...apoptosis,VISUALIZATION_LIVE_PROFILE:'apoptosis-full-figure',VISUALIZATION_LIVE_FULL_MATERIALIZATION:'1',VISUALIZATION_LIVE_REASONING_EFFORT:'low'};
assert.equal(evaluationConfig(fullFigure).profile,'apoptosis-full-figure');
for(const key of ['VISUALIZATION_LIVE_FULL_MATERIALIZATION','VISUALIZATION_LIVE_REASONING_EFFORT']){const missing={...fullFigure};delete missing[key];assert.throws(()=>evaluationConfig(missing),new RegExp(`requires ${key}=`));}
for(const key of ['VISUALIZATION_LIVE_FIT_SCENE','VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE','VISUALIZATION_LIVE_REVIEW_SCENE','VISUALIZATION_LIVE_IFNG_ART_SCENE','VISUALIZATION_LIVE_MATERIALIZE_SCENE','VISUALIZATION_LIVE_PLAN_REVIEW_SOURCE'])assert.throws(()=>evaluationConfig({...fullFigure,[key]:'saved.json'}),/forbids imported/);
assert.throws(()=>evaluationConfig({...fullFigure,VISUALIZATION_LIVE_CONTEXT_ART:'1'}),/separate evaluation/);
assert.throws(()=>evaluationConfig({...fullFigure,VISUALIZATION_LIVE_REASONING_EFFORT:'high'}),/supports only explicit low/);
assert.equal(evaluationConfig(fullFigure).settings.VISUALIZATION_LIVE_FULL_MATERIALIZATION,'1');
console.log('fresh full-figure config: no imported scenes/plans, strict source workflow and explicit isolated reasoning setting passed');
