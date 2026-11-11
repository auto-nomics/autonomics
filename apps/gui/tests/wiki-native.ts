import "../src/app.css";
import { invoke } from "@tauri-apps/api/core";
import { mount } from "./wiki-images-mount";
const id = "a2666ff7-a305-456e-8f4e-a7733b7bc2f0";
const pngBase64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNQaHjwHwAExAKAc00zmAAAAABJRU5ErkJggg==";
async function waitFor(test: () => boolean, label: string) {
  const deadline = Date.now() + 40_000;
  while (!test()) { if (Date.now() > deadline) throw new Error(label); await new Promise(resolve => setTimeout(resolve, 30)); }
}
function check(value: unknown, label: string): asserts value { if (!value) throw new Error(label); }
async function qualify() {
  let revoked = 0;
  const revoke = URL.revokeObjectURL;
  URL.revokeObjectURL = url => { revoked++; revoke.call(URL, url); };
  mount(id);
  await waitFor(() => document.querySelectorAll(".wiki-md img").length === 2 && [...document.querySelectorAll<HTMLImageElement>(".wiki-md img")].every(img => img.src.startsWith("blob:") && img.naturalWidth > 0), "native legacy and portable image rendering");
  document.querySelector<HTMLButtonElement>('button[title="编辑模式"]')!.click();
  await waitFor(() => !!document.querySelector('input[type="file"]'), "native editor");
  const filename = "native-upload [1] $a$.png";
  const transfer = new DataTransfer();
  transfer.items.add(new File([Uint8Array.from(atob(pngBase64), c => c.charCodeAt(0))], filename, { type: "image/png" }));
  const input = document.querySelector<HTMLInputElement>('input[type="file"]')!;
  input.files = transfer.files;
  input.dispatchEvent(new Event("change", { bubbles: true }));
  await waitFor(() => [...document.querySelectorAll<HTMLImageElement>(".wiki-block img")].some(img => img.alt === filename && img.src.startsWith("blob:") && img.naturalWidth > 0), "native multipart upload and image preview");
  await waitFor(() => !document.querySelector<HTMLButtonElement>('button[title="保存 Ctrl+S"]')!.disabled, "upload completed before save");
  document.querySelector<HTMLButtonElement>('button[title="保存 Ctrl+S"]')!.click();
  await waitFor(() => document.querySelectorAll(".wiki-md img").length === 3 && [...document.querySelectorAll<HTMLImageElement>(".wiki-md img")].every(img => img.src.startsWith("blob:") && img.naturalWidth > 0), "saved native reading view");
  const response = await invoke<{ data: { content: string } }>("api", { method: "GET", path: `/wiki/entries/${id}`, body: null });
  check(response.data.content.includes("native-upload"), "saved uploaded image");
  check(!response.data.content.includes("blob:") && !response.data.content.includes("wiki-image-upload:"), "portable persisted content");
  document.querySelector<HTMLImageElement>(".wiki-md img")!.click();
  await waitFor(() => !!document.querySelector<HTMLImageElement>('[role="dialog"] img')?.naturalWidth, "native image lightbox");
  check(revoked > 0, "native object URLs released");
  await invoke("smoke_done", { result: { ok: true, checks: ["legacy/portable rendering", "multipart File upload", "special filename", "save and native binary fetch", "image lightbox", "object URL cleanup"] } });
}
qualify().catch(error => invoke("smoke_done", { result: { ok: false, error: String(error), body: document.body.innerText } }));
