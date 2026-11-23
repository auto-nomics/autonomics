import { useSyncExternalStore } from "react";
import i18n from "@/i18n";
export type CanvasRecord = { id: string; title: string; scene: string | null; updatedAt: number };
let database: Promise<IDBDatabase> | undefined;
function db() {
  return database ??= new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open("autonomics.canvas.v1", 1);
    request.onupgradeneeded = () => request.result.createObjectStore("canvases", { keyPath: "id" });
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => { database = undefined; reject(request.error); };
  });
}
export async function readCanvas(id: string): Promise<CanvasRecord> {
  const connection = await db();
  return new Promise((resolve, reject) => {
    const request = connection.transaction("canvases").objectStore("canvases").get(id);
    request.onsuccess = () => request.result ? resolve(request.result) : reject(new Error(i18n.t("canvas:legacy.notFound")));
    request.onerror = () => reject(request.error);
  });
}
let summaries: CanvasRecord[] = [];
const listeners = new Set<() => void>();
export async function refreshCanvases() {
  const connection = await db();
  const records = await new Promise<CanvasRecord[]>((resolve, reject) => {
    const request = connection.transaction("canvases").objectStore("canvases").getAll();
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  summaries = records.map(record => ({ ...record, scene: null })).sort((a, b) => b.updatedAt - a.updatedAt);
  listeners.forEach(listener => listener());
}
export async function saveCanvas(record: CanvasRecord) {
  const connection = await db();
  await new Promise<void>((resolve, reject) => {
    const transaction = connection.transaction("canvases", "readwrite");
    transaction.objectStore("canvases").put(record);
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error);
    transaction.onabort = () => reject(transaction.error ?? new Error(i18n.t("canvas:legacy.saveAborted")));
  });
  await refreshCanvases();
}
export async function createCanvas() {
  const record: CanvasRecord = { id: crypto.randomUUID(), title: i18n.t("canvas:store.untitled"), scene: null, updatedAt: Date.now() };
  await saveCanvas(record);
  return record;
}
export async function deleteCanvas(id: string) {
  const connection = await db();
  await new Promise<void>((resolve, reject) => {
    const transaction = connection.transaction("canvases", "readwrite");
    transaction.objectStore("canvases").delete(id);
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error);
    transaction.onabort = () => reject(transaction.error ?? new Error(i18n.t("canvas:legacy.deleteAborted")));
  });
  summaries = summaries.filter(record => record.id !== id);
  listeners.forEach(listener => listener());
}
export function useCanvases() {
  return useSyncExternalStore(listener => { listeners.add(listener); return () => { listeners.delete(listener); }; }, () => summaries);
}
