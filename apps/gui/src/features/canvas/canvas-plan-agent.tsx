import { appendCanvasRequestDraft } from "./canvas-request-draft";
import { useRef, useState } from "react";
import { useWorkbenchRuntime } from "@/app/workbench/runtime-context";
import { openAgentEditor } from "@/app/workbench/workbench-store";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import type { Planning } from "./canvas-planning";

export function CanvasPlanAgent({ projectId, canvasId, record, disabled, unchanged, onBusy, onPrepared }: {
  projectId:string; canvasId:string; record:Planning; disabled:boolean;
  unchanged:()=>boolean; onBusy:(value:boolean)=>void; onPrepared:()=>void;
}) {
  const runtime=useWorkbenchRuntime();
  const runtimeRef=useRef(runtime);runtimeRef.current=runtime;
  const [target,setTarget]=useState(runtime.activeAgent||runtime.agents[0]?.path||"");
  const [request,setRequest]=useState("");
  const [error,setError]=useState("");
  const [pending,setPending]=useState(false);
  const [selectedFields,setSelectedFields]=useState<string[]>([]);
  const targetAvailable=runtime.agents.some(agent=>agent.path===target);
  const labels=record.layout_review?.long_labels?.filter(label=>label.plan_field)??[];
  return <details aria-label="计划标签审阅反馈" className="mb-3 rounded border p-3 text-sm">
    <summary className="cursor-pointer">交给 Agent 审阅计划标签</summary>
    <p className="mt-2 text-xs text-muted-foreground">填写本轮要求，核对已保存版本后放入草稿。不会自动发送、改写计划或生成图稿；未保存修改须先保存。</p>
    <label className="mt-2 block">Agent<select aria-label="计划审阅 Agent" className="ml-2 rounded border bg-background p-1" value={target} disabled={disabled||pending} onChange={e=>setTarget(e.target.value)}><option value="">选择 Agent</option>{runtime.agents.map(agent=><option key={agent.path} value={agent.path}>{agent.path}</option>)}</select></label>
    {labels.length>0&&<fieldset aria-label="本轮标签范围" className="mt-2 space-y-1 rounded border p-2" disabled={disabled||pending}>
      <legend>本轮标签范围</legend>
      <p className="text-xs text-muted-foreground">可勾选需要审阅的标签；未勾选时按本轮要求审阅计划。勾选只限定范围，不授权保存修改。</p>
      {labels.map(label=><label key={label.plan_field} className="flex items-start gap-2 text-xs"><input type="checkbox" aria-label={`审阅 ${label.plan_field}`} checked={selectedFields.includes(label.plan_field!)} onChange={event=>setSelectedFields(fields=>event.target.checked?[...fields,label.plan_field!]:fields.filter(field=>field!==label.plan_field))}/><span>{label.preview}（{label.lines} 行）</span></label>)}
    </fieldset>}
    <textarea aria-label="计划标签审阅要求" className="mt-2 min-h-20 w-full rounded border bg-background p-2" maxLength={12000} value={request} disabled={disabled||pending} onChange={e=>setRequest(e.target.value)} placeholder="例如：检查关系标签是否重复来源标题，保留分子状态和关键限定，先提出措辞建议" />
    <Button className="mt-2" size="sm" disabled={disabled||pending||!targetAvailable||!request.trim()} onClick={async()=>{
      if(disabled||pending||!targetAvailable||!request.trim())return;
      setPending(true);onBusy(true);setError("");
      try {
        if(!unchanged())throw new Error("画布或计划已变化，请保存并重新核对。");
        if(!runtime.agents.some(agent=>agent.path===target))throw new Error("所选 Agent 已不可用。");
        const path=`/visualization/projects/${encodeURIComponent(projectId)}/figures/${encodeURIComponent(canvasId)}`;
        const [latest,head]=await Promise.all([gatewayRequest("GET",`${path}/planning`),gatewayRequest("GET",path)]) as [Planning|null,{metadata:{revision:number;title?:string}}];
        if(!latest||latest.version!==record.version||latest.based_on_revision!==record.based_on_revision||head.metadata.revision!==record.based_on_revision||!unchanged())throw new Error("计划或画布版本已变化，请刷新计划后重试。");
        const scope={project_id:projectId,figure_id:canvasId,expected_revision:head.metadata.revision};
        const allLabels=latest.layout_review?.long_labels??[];
        if(selectedFields.some(field=>!allLabels.some(label=>label.plan_field===field)))throw new Error("所选标签诊断已变化，请重新核对。");
        const reviewLabels=selectedFields.length?allLabels.filter(label=>label.plan_field&&selectedFields.includes(label.plan_field)):allLabels;
        const context={...scope,planning_version:latest.version,prepared_request:request.trim(),selected_plan_fields:selectedFields,long_label_review:{scope:latest.layout_review?.scope??null,total:latest.layout_review?.long_label_count??0,omitted:latest.layout_review?.long_labels_omitted??0,unselected_labels:allLabels.length-reviewLabels.length,labels:reviewLabels},follow_up_inputs:{read_plan:{action:"read_plan",...scope}}};
        const prompt=["请按本轮要求审阅已保存的科研绘图计划。先用 follow_up_inputs.read_plan 读取完整计划，核对 planning_version 与图稿 expected_revision；不匹配时停止并重新核对。long_label_review.total 是整个计划的长标签总数，omitted 是诊断上限省略数，unselected_labels 是本轮未选而省略的已知诊断数；labels 仅携带本轮范围内诊断。long_label_review 是编译换行诊断，不是科学正确性检查或自动缩写授权；plan_field 指向该已保存版本的字段，不是画布对象ID。labels.context 是计划声明的节点/面板归属：关系cross_panel=false表示面板内连线，true表示跨面板，null表示未声明可比较的面板归属；不能从标签换行或宽度猜测归属，也不能据此断言换行原因。selected_plan_fields 非空时仅审阅这些字段；如果用户要求保存，也只能修改这些标签字段，其他字段保持不变。如需范围外修改，先说明原因并等待用户扩大范围。空数组不额外限制本轮要求。勾选字段不代表保存授权。先提出建议还是保存修订，以本条消息正文为准，prepared_request仅记录准备时的旧要求。只有用户要求修订时才保存；单标签修订优先 visualization_update_plan_label，传 plan_field、完整 expected_text 和新 text，只修改该标签；同一轮多标签修订优先 visualization_update_plan_labels，changes 中逐条传 plan_field、完整 expected_text 和新 text（最多20条），一次保存一个新版本；任一项不匹配时整批拒绝，先重新读取，不并行写同一版本。结构性修订才用 save_plan。两者均传最新 planning_version 和 expected_revision；保留未要求改动的节点、关系、方向、依据、假设和问题ID/答案，不能只保存诊断摘要。保留必要分子名称、活化/加工/区室限定和检索覆盖范围，不能靠删词或改变科学结论缩短标签。优先直接使用已提供的计划工具参数；回复按用户要求保持简短，通常只给所选标签的原文、建议和必要限定，保存后报告版本及改动即可，不重复工具过程或未修改字段清单。未经实际编译不能声称新标签行数已经减少。本轮不生成草图、不修改已有画布或候选，不自动采用。",JSON.stringify(context,null,2)].join("\n\n");
        if(!runtimeRef.current.agents.some(agent=>agent.path===target))throw new Error("所选 Agent 已不可用，请重新选择；要求已保留。");
        runtime.setDraftFor(target, previous => appendCanvasRequestDraft(previous, request, {title:`计划审阅上下文 · ${head.metadata.title||"当前图稿"}`,source:`visualization:${projectId}/${canvasId}/planning`,revision:`${head.metadata.revision}:${latest.version}`,prompt}));
        runtime.selectAgent(target);onPrepared();openAgentEditor(target);
      }catch(cause){setError(String(cause));}finally{setPending(false);onBusy(false);}
    }}>{pending?"核对计划版本…":"准备计划审阅草稿"}</Button>
    {error&&<p role="alert" className="mt-2 text-destructive">{error}</p>}
  </details>;
}
