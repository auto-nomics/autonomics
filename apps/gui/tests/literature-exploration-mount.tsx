import { useState } from "react";
import { createRoot } from "react-dom/client";
import { LiteratureExploration } from "../src/features/discovery/literature-exploration";
import type { BibArticle } from "../src/types";

function Harness() {
  const [which,setWhich]=useState("a");
  const article: BibArticle={id:which,title:which === "a" ? "Alpha seed" : "Beta seed",identifiers:which === "none" ? [] : [{kind:"s2",value:which.repeat(40)}]};
  return <main style={{maxWidth:480,padding:12}}>
    <nav style={{display:"flex",flexWrap:"wrap",gap:12}}>
      <button onClick={()=>setWhich("a")}>Select Alpha</button>
      <button onClick={()=>setWhich("c")}>Select Beta</button>
      <button onClick={()=>setWhich("none")}>Remove identifier</button>
    </nav>
    <h1>{article.title}</h1><LiteratureExploration article={article}/>
  </main>;
}
export function mountExploration(element: HTMLElement) { createRoot(element).render(<Harness/>); }
