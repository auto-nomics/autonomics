import { useSyncExternalStore } from "react";
import { gatewayRequest } from "@/gateway";
import * as legacy from "./legacy-canvas-store";
export type Project = { id: string; title: string };
export type CanvasRecord = { id: string; projectId?: string; revision?: number; title: string; scene: string | null; updatedAt: number };
type Metadata = { id: string; project_id: string; revision: number; title: string; updated_at: string };
let summaries: CanvasRecord[] = [];
let selectedProject: string | undefined;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach(listener => listener());
const path = (projectId: string) => `/visualization/projects/${encodeURIComponent(projectId)}/figures`;
const record = (meta: Metadata): CanvasRecord => ({ id: meta.id, projectId: meta.project_id, revision: meta.revision, title: meta.title, updatedAt: Date.parse(meta.updated_at), scene: null });
const projectPreference = "autonomics:visualization:project";
export const selectedCanvasProject = () => {
  if (selectedProject !== undefined) return selectedProject;
  try { return localStorage.getItem(projectPreference) || undefined; } catch { return undefined; }
};
export const listProjects = (): Promise<Project[]> => gatewayRequest("GET", "/visualization/projects");
export const createProject = (title: string): Promise<Project> => gatewayRequest("POST", "/visualization/projects", { id: crypto.randomUUID(), title });
export async function readCanvas(id: string, projectId?: string): Promise<CanvasRecord> {
  if (!projectId) return legacy.readCanvas(id);
  const response = await gatewayRequest("GET", `${path(projectId)}/${encodeURIComponent(id)}`) as { metadata: Metadata; scene: { document: unknown } };
  return { ...record(response.metadata), scene: JSON.stringify(response.scene.document) };
}
export async function refreshCanvases(projectId?: string) {
  selectedProject = projectId;
  try { if (projectId) localStorage.setItem(projectPreference,projectId); else localStorage.removeItem(projectPreference); } catch { /* Storage may be unavailable in embedded views. */ }
  if (!projectId) { summaries = []; emit(); return; }
  const records = await gatewayRequest("GET", path(projectId)) as Metadata[];
  if (selectedProject === projectId) { summaries = records.map(record); emit(); }
}
export async function saveCanvas(canvas: CanvasRecord): Promise<number | undefined> {
  if (!canvas.projectId) { await legacy.saveCanvas(canvas); return; }
  const saved = await gatewayRequest("PUT", `${path(canvas.projectId)}/${encodeURIComponent(canvas.id)}`, { expected_revision: canvas.revision, title: canvas.title, document: JSON.parse(canvas.scene || "{}") }) as Metadata;
  // A successful write stays successful even when the sidebar refresh fails.
  if (selectedProject === canvas.projectId) { summaries = summaries.map(item => item.id === saved.id ? record(saved) : item); emit(); }
  return saved.revision;
}
export async function createCanvas(projectId: string) {
  const saved = await gatewayRequest("POST", path(projectId), { title: "未命名画布", format: "excalidraw/v2", document: { type: "excalidraw", version: 2, elements: [], files: {}, appState: { currentItemRoughness: 0, currentItemFontFamily: 9, currentItemRoundness: "sharp", viewBackgroundColor: "#ffffff" } } }) as Metadata;
  if (selectedProject === projectId) { summaries = [record(saved), ...summaries]; emit(); }
  return record(saved);
}
export function useCanvases() {
  return useSyncExternalStore(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => summaries);
}
