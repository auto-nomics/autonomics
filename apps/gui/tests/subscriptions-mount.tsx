import { useState } from "react";
import { createRoot } from "react-dom/client";
import { SubscribeSearchButton, Subscriptions } from "../src/features/discovery/subscriptions";
function Harness() {
  const [view,setView] = useState("discover");
  return <main style={{height:"100vh",display:"flex",flexDirection:"column",minWidth:0}}>
    <nav style={{display:"flex",gap:16,padding:8}}><button onClick={()=>setView("discover")}>discover</button><button onClick={()=>setView("subscriptions")}>subscriptions</button></nav>
    <div hidden={view!=="discover"}><SubscribeSearchButton request={{query:"immune",sources:["openalex"],filters:{year_from:2020,publication_types:["article"]},sort:"newest",page_size:25}}/></div>
    <div hidden={view!=="subscriptions"} style={view==="subscriptions"?{display:"flex",flex:1,minHeight:0,minWidth:0}:undefined}><Subscriptions active={view==="subscriptions"}/></div>
  </main>;
}
export function mountSubscriptions(element:HTMLElement) {createRoot(element).render(<Harness/>);}
