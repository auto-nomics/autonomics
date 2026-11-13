import { useState } from "react";
import { DefaultSidebar, Sidebar } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { BioArtPanel, useBioArtState } from "./bioart-panel";
import { BioIconsPanel, useBioIconsState } from "./bioicons-panel";
import "./canvas-material-browser.css";

/** Native library extensions share persistent controllers across tab/viewport changes. */
export function CanvasMaterialBrowser({ getApi, onOpen }: { getApi: () => ExcalidrawImperativeAPI | null; onOpen: () => void }) {
  const [tab, setTab] = useState("library");
  const bioart = useBioArtState();
  const bioicons = useBioIconsState();
  return <>
    <DefaultSidebar className="canvas-material-browser" onStateChange={state => { if (state) { setTab(state.tab ?? "library"); onOpen(); } }}>
      <DefaultSidebar.TabTriggers>
        <select ref={element => { element?.closest(".canvas-material-browser")?.setAttribute("data-prevent-outside-click", "true"); }} aria-label="素材库来源" className="canvas-material-source-select" value={tab} onChange={event => { const value = event.target.value; setTab(value); getApi()?.toggleSidebar({ name: "default", tab: value, force: true }); }}>
          {tab === "search" && <option value="search" disabled>画布搜索</option>}
          <option value="library">收藏素材</option>
          <option value="bioart">BioArt 素材</option>
          <option value="bioicons">矢量素材</option>
        </select>
      </DefaultSidebar.TabTriggers>
      <Sidebar.Tab tab="bioart"><BioArtPanel embedded controller={bioart} getApi={getApi} /></Sidebar.Tab>
      <Sidebar.Tab tab="bioicons"><BioIconsPanel embedded controller={bioicons} getApi={getApi} /></Sidebar.Tab>
    </DefaultSidebar>
  </>;
}
