import lute from "vditor/dist/js/lute/lute.min.js?raw";
import chinese from "vditor/dist/js/i18n/zh_CN.js?raw";
import icons from "vditor/dist/js/icons/ant.js?raw";
import highlight from "vditor/dist/js/highlight.js/highlight.min.js?raw";
import highlightLanguages from "vditor/dist/js/highlight.js/third-languages.js?raw";
import highlightCss from "vditor/dist/js/highlight.js/styles/github.min.css?inline";
import highlightDarkCss from "vditor/dist/js/highlight.js/styles/github-dark.min.css?inline";
import katexCss from "katex/dist/katex.min.css?inline";
import katex from "katex";
import "katex/dist/contrib/mhchem.mjs";
import mermaid from "mermaid";

// The gateway embeds a single JS/CSS bundle. Load Vditor's core resources
// from that bundle rather than its default CDN (also works offline in Tauri).
function script(id: string, source = "") {
  if (document.getElementById(id)) return;
  const element = document.createElement("script");
  element.id = id;
  element.textContent = source;
  document.head.append(element);
}

function style(id: string, css: string, href?: string) {
  if (document.getElementById(id)) return;
  const element = document.createElement("style");
  element.id = id;
  if (href) element.setAttribute("href", href);
  element.textContent = css;
  document.head.append(element);
}

export function prepareVditor() {
  script("vditorLuteScript", lute);
  script("vditorI18nScriptzh_CN", chinese);
  script("vditorIconScript", document.getElementById("vditor-icon-bold") ? "" : icons);
  script("vditorHljsScript", highlight);
  script("vditorHljsThirdScript", highlightLanguages);
  Object.assign(window, { katex, mermaid });
  script("vditorKatexScript");
  script("vditorKatexChemScript");
  script("vditorMermaidScript");
  style("vditorKatexStyle", katexCss);
  style("vditorHljsStyle", highlightCss, "/vditor/dist/js/highlight.js/styles/github.min.css");
}

export function setVditorCodeTheme(dark: boolean) {
  const element = document.getElementById("vditorHljsStyle");
  const css = dark ? highlightDarkCss : highlightCss;
  if (element && element.textContent !== css) element.textContent = css;
}
