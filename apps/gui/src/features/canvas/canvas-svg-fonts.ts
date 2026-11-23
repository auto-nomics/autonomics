import { convertToExcalidrawElements, exportToSvg } from "@excalidraw/excalidraw";
import i18n from "@/i18n";

/** Make local Helvetica runs (including native frame names) portable in SVG.
 * Liberation's compatible Latin metrics are already the pinned scientific font;
 * the public exporter supplies a real subset, without changing the saved scene.
 */
export async function embedLocalSvgFonts(svg: SVGSVGElement) {
  const texts = [...svg.querySelectorAll("text[font-family]")].filter(node =>
    node.getAttribute("font-family")?.split(",")[0].trim().replaceAll(/['"]/g, "") === "Helvetica");
  if (!texts.length) return;
  const glyphs = [...new Set(texts.map(node => node.textContent ?? "").join(""))].join("");
  if (!glyphs.trim()) return;
  const alias = "Autonomics Export Sans";
  const subset = await exportToSvg({
    elements: convertToExcalidrawElements([{ type: "text", text: glyphs, x: 0, y: 0, fontFamily: 9, fontSize: 14 }]),
    appState: { exportWithDarkMode: false }, files: {},
  });
  const styles = [...subset.querySelectorAll("style")].map(style => style.textContent ?? "").filter(css => css.includes("@font-face"));
  if (!styles.length) throw new Error(i18n.t("canvas:svgFonts.embedFailed"));
  const style = document.createElementNS("http://www.w3.org/2000/svg", "style");
  style.textContent = styles.join("\n").replaceAll(/font-family\s*:\s*[^;{}]+/g, `font-family: "${alias}"`);
  svg.prepend(style);
  for (const text of texts) text.setAttribute("font-family", `${alias}, ${text.getAttribute("font-family")}`);
}
