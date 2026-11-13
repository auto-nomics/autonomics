import "../../src/app.css";
import React from 'react';
import {createRoot} from 'react-dom/client';
import {serializeAsJSON,restoreElements,exportToBlob} from '@excalidraw/excalidraw';
import {CanvasCodeDrawing} from '../../src/features/canvas/canvas-code-drawing';
let elements:any[]=[],files:Record<string,any>={},state:any={selectedElementIds:{},zoom:{value:1},width:1200,height:800,scrollX:0,scrollY:0,offsetLeft:0,offsetTop:0};
const api:any={getSceneElements:()=>elements,getFiles:()=>files,getAppState:()=>state,addFiles:(added:any[])=>added.forEach(f=>files[f.id]=f),updateScene:(next:any)=>{elements=next.elements??elements;state={...state,...next.appState};},scrollToContent:()=>{}};
(window as any).codeHarness={get:()=>({elements,files}),reload:()=>{const saved=JSON.parse(serializeAsJSON(elements,state,files,'local'));elements=restoreElements(saved.elements,null,{repairBindings:true});files=saved.files;},export:async()=>{const blob=await exportToBlob({elements,files,appState:{exportBackground:false},mimeType:'image/png'});return blob.size;}};
createRoot(document.getElementById('root')!).render(<CanvasCodeDrawing getApi={()=>api}/>);
