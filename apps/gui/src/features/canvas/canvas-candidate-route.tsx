import {CanvasCandidateRevision,type CandidateRevisionProps} from './canvas-candidate-revision';
export function CanvasCandidateRoute(props:CandidateRevisionProps&{focusedId?:string}){
 const {candidate,focusedId}=props,elements=(candidate.document.elements??[]).filter(e=>!e.isDeleted);
 const selected=elements.find(e=>e.id===focusedId),arrow=elements.find(e=>e.id===(selected?.type==='text'?selected.containerId:selected?.id));
 if(!arrow||arrow.type!=='arrow'||!candidate.document_sha256)return null;
 const crossPanel=arrow.customData?.crossPanel===true;
 const label=elements.find(e=>e.type==='text'&&e.containerId===arrow.id),valid=!arrow.locked&&!label?.locked&&arrow.angle===0&&(crossPanel?!arrow.frameId:Boolean(arrow.frameId)&&!arrow.roundness&&arrow.roughness===0)&&!('elbowed' in arrow&&arrow.elbowed);
 return <CanvasCandidateRevision {...props} draftKey={['reroute',arrow.id]} onCommitted={()=>{}} label={crossPanel?"候选跨分面连线重布线":"候选局部连线重布线"} title={crossPanel?"重新规划跨分面连线":"避让局部组件标签"} summary={crossPanel?"按现有内容重新规划跨分面连线":"保留端点并避让局部组件标签"} submitLabel="预览连线重布线" valid={valid} operations={[{op:'reroute',id:arrow.id}]}>
 <p>{crossPanel?"按当前组件和文字边界重新规划此连线及其说明位置，保留端点绑定、方向、线型、完整说明与科学依据。新路径可能仍有折返，请核对候选预览后再采用。":"在原分面内避让当前组件和文字，保留端点坐标、绑定、方向和说明位置。仅支持尖角直角折线；空间不足时不生成修改。请核对候选预览后再采用。"}</p>
 {!valid&&<p role="status">此操作要求未锁定、无旋转的原生连线与说明；局部连线须与两端组件同属一个分面。</p>}
 </CanvasCandidateRevision>;
}
