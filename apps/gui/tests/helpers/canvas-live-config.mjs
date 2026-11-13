// Persist only evaluation settings; never copy arbitrary environment variables.
const strictEgfr = {
  VISUALIZATION_LIVE_GATEWAY:'1',
  VISUALIZATION_LIVE_EVIDENCE_WORKFLOW:'1',
  VISUALIZATION_LIVE_EVIDENCE_CASE:'egfr-mapk',
  VISUALIZATION_LIVE_ART_PROVIDER:'bioart',
  VISUALIZATION_LIVE_LAYOUT_PREFERENCE:'compact',
  VISUALIZATION_LIVE_CONCISE_CAPTION:'1',
  VISUALIZATION_LIVE_REQUIRE_GEOMETRY_REVIEW:'1',
  VISUALIZATION_LIVE_REQUIRE_LABEL_CLEARANCE:'1',
  VISUALIZATION_LIVE_REQUIRE_SCALAR_TEXT:'1',
  VISUALIZATION_LIVE_REQUIRE_SCALAR_FIT:'1',
  VISUALIZATION_LIVE_REQUIRE_SCALAR_TRANSLATE:'1',
  VISUALIZATION_LIVE_REQUIRE_SCALAR_SCALE:'1',
  VISUALIZATION_LIVE_IFNG_CONTINUATION:'1',
  VISUALIZATION_LIVE_ILLUSTRATION_RESIZE:'1',
  VISUALIZATION_LIVE_REVIEW_UNREAD_SOURCES:'1',
};
const strictIfng={...strictEgfr,VISUALIZATION_LIVE_EVIDENCE_CASE:'ifng',VISUALIZATION_LIVE_REQUIRE_BATCH_EVIDENCE:'1'};
const apoptosisSketch={VISUALIZATION_LIVE_GATEWAY:'1',VISUALIZATION_LIVE_EVIDENCE:'1',VISUALIZATION_LIVE_EVIDENCE_WORKFLOW:'0',VISUALIZATION_LIVE_EVIDENCE_CASE:'intrinsic-apoptosis',VISUALIZATION_LIVE_LAYOUT_PREFERENCE:'compact',VISUALIZATION_LIVE_CONCISE_CAPTION:'1',VISUALIZATION_LIVE_REQUIRE_GEOMETRY_REVIEW:'1',VISUALIZATION_LIVE_REQUIRE_BATCH_EVIDENCE:'1',VISUALIZATION_LIVE_REQUIRE_SCALAR_TEXT:'1',VISUALIZATION_LIVE_REVIEW_UNREAD_SOURCES:'1'};
const profiles={'apoptosis-full-figure':{...apoptosisSketch,VISUALIZATION_LIVE_FULL_MATERIALIZATION:'1',VISUALIZATION_LIVE_REASONING_EFFORT:'low'},'tgfb-compact-sketch':{...apoptosisSketch,VISUALIZATION_LIVE_EVIDENCE_CASE:'tgfb-smad'},'apoptosis-context-art':{...apoptosisSketch,VISUALIZATION_LIVE_CONTEXT_ART:'1'},'apoptosis-compact-sketch':apoptosisSketch,'egfr-compact-bioart':strictEgfr,'ifng-compact-bioart':strictIfng,'ifng-multi-bioart':{...strictIfng,VISUALIZATION_LIVE_SECOND_MATERIAL:'1'}};
export function evaluationConfig(env) {
  const svgSettings={enabled:env.VISUALIZATION_LIVE_SVG==='1',bioart:env.VISUALIZATION_LIVE_SVG_BIOART==='1',online:env.VISUALIZATION_LIVE_SVG_ONLINE==='1',visible_repair:env.VISUALIZATION_LIVE_SVG_VISIBLE==='1',path_handoff:env.VISUALIZATION_LIVE_SVG_PATH_HANDOFF==='1',candidate_path_handoff:env.VISUALIZATION_LIVE_SVG_CANDIDATE_PATH==='1',shared_path_context:env.VISUALIZATION_LIVE_SVG_SHARED_CONTEXT==='1'};
  if(svgSettings.shared_path_context&&!svgSettings.candidate_path_handoff)throw new Error('shared path context requires candidate path handoff mode');
  if(svgSettings.candidate_path_handoff&&!svgSettings.path_handoff)throw new Error('candidate path handoff requires path handoff mode');
  if(svgSettings.path_handoff&&!svgSettings.visible_repair)throw new Error('path handoff requires visible SVG repair mode');
  if(svgSettings.visible_repair&&(!svgSettings.enabled||!svgSettings.bioart||svgSettings.online||!env.VISUALIZATION_LIVE_FIT_SCENE))throw new Error('visible SVG repair requires SVG/BioArt mode and an explicit scene; cannot run online acquisition');
  if(svgSettings.online&&(!svgSettings.enabled||!svgSettings.bioart||env.VISUALIZATION_LIVE_FIT_SCENE))throw new Error('online BioArt SVG requires SVG/BioArt mode and no imported scene');
  if(svgSettings.bioart&&(!svgSettings.enabled||(!svgSettings.online&&!env.VISUALIZATION_LIVE_FIT_SCENE)))throw new Error('BioArt SVG requires SVG mode and an explicit saved scene or online acquisition');
  if(env.VISUALIZATION_LIVE_CONTEXT_ART==='1'){
    if(env.VISUALIZATION_LIVE_EVIDENCE!=='1'||env.VISUALIZATION_LIVE_EVIDENCE_WORKFLOW!=='0'||env.VISUALIZATION_LIVE_EVIDENCE_CASE!=='intrinsic-apoptosis')throw new Error('context artwork continuation requires fresh apoptosis evidence planning');
    if(['VISUALIZATION_LIVE_REFERENCE','VISUALIZATION_LIVE_COMPOSE','VISUALIZATION_LIVE_SVG','VISUALIZATION_LIVE_FIT','VISUALIZATION_LIVE_CANDIDATE','VISUALIZATION_LIVE_PREPARE_ONLY'].some(key=>env[key]==='1')||env.VISUALIZATION_LIVE_SCIENTIFIC)throw new Error('fresh context artwork continuation requires a separate evaluation');
    if(['VISUALIZATION_LIVE_FIT_SCENE','VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE','VISUALIZATION_LIVE_REVIEW_SCENE','VISUALIZATION_LIVE_IFNG_ART_SCENE','VISUALIZATION_LIVE_MATERIALIZE_SCENE'].some(key=>env[key]))throw new Error('fresh context artwork continuation forbids imported scenes');
  }
  if(env.VISUALIZATION_LIVE_FULL_MATERIALIZATION==='1'){
    if(env.VISUALIZATION_LIVE_GATEWAY!=='1'||env.VISUALIZATION_LIVE_EVIDENCE!=='1'||env.VISUALIZATION_LIVE_EVIDENCE_WORKFLOW!=='0'||env.VISUALIZATION_LIVE_EVIDENCE_CASE!=='intrinsic-apoptosis')throw new Error('full materialization requires fresh gateway apoptosis evidence planning');
    if(['VISUALIZATION_LIVE_FIT_SCENE','VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE','VISUALIZATION_LIVE_REVIEW_SCENE','VISUALIZATION_LIVE_IFNG_ART_SCENE','VISUALIZATION_LIVE_MATERIALIZE_SCENE','VISUALIZATION_LIVE_PLAN_REVIEW_SOURCE'].some(key=>env[key]))throw new Error('fresh full materialization forbids imported scenes or plans');
    if(['VISUALIZATION_LIVE_CONTEXT_ART','VISUALIZATION_LIVE_REFERENCE','VISUALIZATION_LIVE_COMPOSE','VISUALIZATION_LIVE_SVG','VISUALIZATION_LIVE_FIT','VISUALIZATION_LIVE_CANDIDATE','VISUALIZATION_LIVE_PREPARE_ONLY'].some(key=>env[key]==='1')||env.VISUALIZATION_LIVE_SCIENTIFIC)throw new Error('fresh full materialization requires a separate evaluation');
  }
  if(env.VISUALIZATION_LIVE_REASONING_EFFORT&&env.VISUALIZATION_LIVE_REASONING_EFFORT!=='low')throw new Error('evaluation reasoning effort supports only explicit low');
  const profile=env.VISUALIZATION_LIVE_PROFILE??'custom';
  if(profile==='tgfb-compact-sketch'&&['VISUALIZATION_LIVE_FIT_SCENE','VISUALIZATION_LIVE_SOURCE_REVIEW_SCENE','VISUALIZATION_LIVE_REVIEW_SCENE','VISUALIZATION_LIVE_IFNG_ART_SCENE','VISUALIZATION_LIVE_MATERIALIZE_SCENE'].some(key=>env[key]))throw new Error('fresh TGFB profile forbids imported scenes');
  if(profile!=='custom'&&!Object.hasOwn(profiles,profile))throw new Error(`unknown evaluation profile: ${profile}`);
  if(profile!=='custom')for(const [key,value] of Object.entries(profiles[profile])) {
    if(env[key]!==value)throw new Error(`${profile} requires ${key}=${value}; no model was started`);
  }
  if(profile!=='custom'&&env.VISUALIZATION_LIVE_SECOND_MATERIAL==='1'&&profile!=='ifng-multi-bioart')throw new Error('VISUALIZATION_LIVE_SECOND_MATERIAL=1 requires ifng-multi-bioart profile or custom');
  if(env.VISUALIZATION_LIVE_SECOND_MATERIAL==='1'&&(env.VISUALIZATION_LIVE_EVIDENCE_WORKFLOW!=='1'||env.VISUALIZATION_LIVE_ART_PROVIDER!=='bioart'))throw new Error('second material requires the evidence workflow and bioart provider');
  if(env.VISUALIZATION_LIVE_SECOND_MATERIAL==='1'&&env.VISUALIZATION_LIVE_EVIDENCE_CASE&&env.VISUALIZATION_LIVE_EVIDENCE_CASE!=='ifng')throw new Error('second material workflow requires VISUALIZATION_LIVE_EVIDENCE_CASE=ifng');
  return {profile,provider:env.VISUALIZATION_LIVE_PROVIDER??null,model:env.VISUALIZATION_LIVE_MODEL??null,svg_settings:svgSettings,
    settings:Object.fromEntries([...new Set(Object.values(profiles).flatMap(value=>Object.keys(value)))].map(key=>[key,env[key]??null])),
    defaults:{VISUALIZATION_LIVE_ART_PROVIDER:'bioicons',VISUALIZATION_LIVE_LAYOUT_PREFERENCE:'auto',VISUALIZATION_LIVE_EVIDENCE_CASE:'ifng',boolean_flags:'disabled unless exactly 1'}};
}
