import i18n from "@/i18n";
export type CodeDrawing = { version: 1; code: string; params: Record<string, unknown>; width: number; height: number };
export const DEFAULT_DRAWING: CodeDrawing = {
  version: 1, width: 480, height: 320, params: { label: "Protein", color: "#60a5fa", count: 8 },
  code: `function draw(ctx, params, width, height) {
  ctx.translate(width / 2, height / 2);
  for (let i = 0; i < params.count; i++) {
    const angle = i * Math.PI * 2 / params.count;
    ctx.beginPath();
    ctx.ellipse(Math.cos(angle) * 65, Math.sin(angle) * 55, 48, 35, angle, 0, Math.PI * 2);
    ctx.fillStyle = params.color;
    ctx.fill();
    ctx.strokeStyle = "#1e3a8a";
    ctx.lineWidth = 2;
    ctx.stroke();
  }
  ctx.fillStyle = "#0f172a";
  ctx.font = "24px sans-serif";
  ctx.textAlign = "center";
  ctx.fillText(params.label, 0, 8);
}`,
};
export function validateDrawing(value: CodeDrawing) {
  if (value.version !== 1 || typeof value.code !== "string" || !value.code.trim() || value.code.length > 100000) throw new Error(i18n.t("canvas:codeRuntime.codeLength"));
  if (![value.width, value.height].every(n => Number.isInteger(n) && n >= 1 && n <= 4096) || value.width * value.height > 8_000_000) throw new Error(i18n.t("canvas:codeRuntime.dimensions"));
  if (!value.params || typeof value.params !== "object" || Array.isArray(value.params) || JSON.stringify(value.params).length > 100000) throw new Error(i18n.t("canvas:codeRuntime.params"));
}
// A sandboxed opaque-origin frame supplies a CSP to its disposable worker.
// Neither the frame nor drawing code has access to the application's DOM/storage.
export function renderCodeDrawing(drawing: CodeDrawing, timeoutMs = 4000): Promise<string> {
  validateDrawing(drawing);
  const workerSource = `self.onmessage = async ({data}) => {
    try {
      const canvas = new OffscreenCanvas(data.width, data.height);
      const ctx = canvas.getContext('2d');
      const draw = new Function(data.code + '\\n;return draw;')();
      if (typeof draw !== 'function') throw new Error(i18n.t("canvas:codeRuntime.defineDraw"));
      await draw(ctx, data.params, data.width, data.height);
      const blob = await canvas.convertToBlob({type:'image/png'});
      const reader = new FileReaderSync();
      self.postMessage({image:reader.readAsDataURL(blob)});
    } catch (error) { self.postMessage({error:String(error)}); }
  };`;
  return new Promise((resolve, reject) => {
    const frame = document.createElement("iframe");
    frame.hidden = true;
    frame.setAttribute("sandbox", "allow-scripts");
    const token = crypto.randomUUID();
    const finish = (error?: Error, image?: string) => {
      clearTimeout(timer); window.removeEventListener("message", receive); frame.remove();
      if (error) reject(error); else resolve(image!);
    };
    const receive = (event: MessageEvent) => {
      if (event.source !== frame.contentWindow || event.data?.token !== token) return;
      if (event.data.ready) { frame.contentWindow?.postMessage(drawing, "*"); return; }
      if (event.data.error) finish(new Error(event.data.error === "__autonomics_i18n_exec_failed__" ? i18n.t("canvas:codeRuntime.execFailed") : event.data.error));
      else if (typeof event.data.image === "string" && event.data.image.startsWith("data:image/png;base64,") && event.data.image.length < 48_000_000) finish(undefined, event.data.image);
      else finish(new Error(i18n.t("canvas:codeRuntime.invalidOutput")));
    };
    const timer = setTimeout(() => finish(new Error(i18n.t("canvas:codeRuntime.timeout"))), timeoutMs);
    window.addEventListener("message", receive);
    const script = `const token=${JSON.stringify(token)};
      const worker = new Worker(URL.createObjectURL(new Blob([${JSON.stringify(workerSource)}], {type:'text/javascript'})));
      worker.onmessage = e => { parent.postMessage({...e.data,token}, '*'); worker.terminate(); };
      worker.onerror = () => parent.postMessage({error:${JSON.stringify("__autonomics_i18n_exec_failed__")},token}, '*');
      onmessage = e => { if(e.source === parent) worker.postMessage(e.data); };
      parent.postMessage({ready:true,token}, '*');`;
    frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' blob:; worker-src blob:; connect-src 'none';"><script>${script.replaceAll("</script", "<\\/script")}<\/script>`;
    document.body.append(frame);
  });
}
