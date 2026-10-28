import { useState } from "react";
import { createRoot } from "react-dom/client";
import { SaveSnapshotButton, SearchSnapshots } from "../src/features/search-snapshots";
function Harness() {
  const [view,setView]=useState("library");
  return <main style={{height:"100vh",display:"flex",flexDirection:"column"}}>
    <nav style={{display:"flex",gap:16,padding:12}}>{["library","discovery","snapshots"].map(name=><button key={name} onClick={()=>setView(name)}>{name}</button>)}</nav>
    <div hidden={view!=="library"}><SaveSnapshotButton execution={{kind:"library",search:{query:"Study",page_size:50,scope:{library_id:1}}}}/></div>
    <div hidden={view!=="discovery"}><SaveSnapshotButton execution={{kind:"discovery",session_id:"session-fixture",sequence:7,order:"first_seen"}}/></div>
    <div hidden={view!=="snapshots"} style={view==="snapshots"?{display:"flex",flex:1,minHeight:0}:undefined}><SearchSnapshots active={view==="snapshots"}/></div>
  </main>;
}
export function mountSnapshots(element:HTMLElement) {createRoot(element).render(<Harness/>);}
