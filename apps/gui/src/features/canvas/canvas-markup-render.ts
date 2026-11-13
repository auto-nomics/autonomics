import { convertToExcalidrawElements, exportToBlob } from "@excalidraw/excalidraw";
import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
export type MarkupPoint = [number, number];
export type MarkupStroke = { kind: "draw" | "cross" | "emphasis"; points: MarkupPoint[] };
export type MarkupSummary = { id: string; at_revision: number; note: string; stroke_count: number; status: string };
export type MarkupPage = { items: MarkupSummary[]; next_before: string | null };
export type MarkupRecord = MarkupSummary & { strokes: MarkupStroke[]; dataURL: string };
export const markupColor = (kind: MarkupStroke["kind"]) => kind === "draw" ? "#1971c2" : kind === "cross" ? "#e03131" : "#e67700";
/** Transient render elements: they are never inserted into the editable scene. */
export function markupElements(strokes: MarkupStroke[]) {
  const prefix = `review-${crypto.randomUUID()}`;
  return strokes.flatMap((stroke, index) => {
    const [a, b] = [stroke.points[0], stroke.points.at(-1)!];
    const style = { strokeColor: markupColor(stroke.kind), strokeWidth: 3, roughness: 0, backgroundColor: "transparent" };
    if (stroke.kind === "emphasis") return convertToExcalidrawElements([{ ...style, id: `${prefix}-${index}`, type: "ellipse", x: Math.min(a[0], b[0]), y: Math.min(a[1], b[1]), width: Math.abs(b[0] - a[0]), height: Math.abs(b[1] - a[1]) }]);
    const paths = stroke.kind === "cross" ? [[a, b], [[a[0], b[1]], [b[0], a[1]]]] as MarkupPoint[][] : [stroke.points];
    return paths.flatMap((path, part) => convertToExcalidrawElements([{ ...style, id: `${prefix}-${index}-${part}`, type: "line", x: path[0][0], y: path[0][1], points: path.map(point => [point[0] - path[0][0], point[1] - path[0][1]] as [number, number]) }]));
  });
}
export async function renderMarkup(api: ExcalidrawImperativeAPI, strokes: MarkupStroke[]) {
  const elements = [...api.getSceneElements(), ...markupElements(strokes)];
  const render = (size: number) => exportToBlob({ elements, appState: { ...api.getAppState(), exportWithDarkMode: false, exportEmbedScene: false, exportBackground: true }, files: api.getFiles(), mimeType: "image/png", maxWidthOrHeight: size });
  let blob = await render(1280);
  if (blob.size > 4.4 * 1024 * 1024) blob = await render(1024);
  return new Promise<string>((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(reader.error); reader.readAsDataURL(blob); });
}
