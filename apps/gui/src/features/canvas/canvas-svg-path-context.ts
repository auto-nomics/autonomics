export type SvgPathSnapshot = {
  project_id: string; figure_id: string; element_id: string; expected_revision: number;
  sha256: string; part_id: string; part_index: number;
};
/** Only read actions: review links are mutation intent, not SVG inspection arguments. */
export function svgPathReadInputs(input: SvgPathSnapshot, formalPreviewAvailable = false) {
  const figure = { project_id:input.project_id,figure_id:input.figure_id,expected_revision:input.expected_revision };
  const scope = { ...figure,element_id:input.element_id };
  return {
      read:{action:"read",...figure,element_ids:[input.element_id]},
      read_image:{action:"read_image",...scope},
      ...(formalPreviewAvailable?{read_preview:{action:"read_preview",...figure}}:{}),
    parts:{action:"parts",...scope},
    geometry:{action:"geometry",...scope,sha256:input.sha256,part_id:input.part_id},
  };
}
export type SvgPathContext = SvgPathSnapshot & { scope: "saved_revision"; panel_visible: boolean; follow_up_inputs?:ReturnType<typeof svgPathReadInputs>; follow_up_inputs_omitted?:"size_limit" };
export type SvgPathState = { owner: string | null; snapshot: SvgPathContext | null };
export const emptySvgPathState: SvgPathState = { owner: null, snapshot: null };
export function svgPathSnapshot(input: SvgPathSnapshot): SvgPathContext | null {
  const id = (value: unknown): value is string => typeof value === "string" && value.trim().length > 0 && new TextEncoder().encode(value).length <= 512;
  const hash = (value: unknown): value is string => typeof value === "string" && /^[a-f0-9]{64}$/i.test(value);
  if (![input.project_id,input.figure_id,input.element_id,input.part_id].every(id) || !hash(input.sha256) || !Number.isSafeInteger(input.expected_revision) || input.expected_revision < 1 || !Number.isInteger(input.part_index) || input.part_index < 1 || input.part_index > 256) return null;
  const identity: SvgPathContext = { project_id:input.project_id,figure_id:input.figure_id,element_id:input.element_id,expected_revision:input.expected_revision,sha256:input.sha256,part_id:input.part_id,part_index:input.part_index,scope:"saved_revision",panel_visible:true };
  const value={...identity,follow_up_inputs:svgPathReadInputs(input)};
  if (new TextEncoder().encode(JSON.stringify(value)).length <= 4096) return value;
  const fallback={...identity,follow_up_inputs_omitted:"size_limit" as const};
  return new TextEncoder().encode(JSON.stringify(fallback)).length <= 4096 ? fallback : null;
}
export type SvgPathEvent =
  | { kind:"inspect"; owner:string; snapshot:SvgPathSnapshot }
  | { kind:"hide"; owner:string }
  | { kind:"clear" }
  | { kind:"head_observed"; revision:number }
  | { kind:"scene"; revision:number|undefined; dirty:boolean; selectionChanged:boolean; selectedIds:string[] };
export function svgPathTransition(state:SvgPathState,event:SvgPathEvent):SvgPathState {
  if(event.kind==="inspect") {const snapshot=svgPathSnapshot(event.snapshot);return snapshot?{owner:event.owner,snapshot}:emptySvgPathState;}
  if(event.kind==="clear")return emptySvgPathState;
  const current=state.snapshot;if(!current)return state;
  if(event.kind==="head_observed")return event.revision>current.expected_revision?emptySvgPathState:state;
  if(event.kind==="hide")return state.owner===event.owner&&current.panel_visible?{...state,snapshot:{...current,panel_visible:false}}:state;
  if(event.kind==="scene"&&(event.dirty||event.revision!==current.expected_revision||(event.selectionChanged&&!event.selectedIds.includes(current.element_id))))return emptySvgPathState;
  return state;
}
