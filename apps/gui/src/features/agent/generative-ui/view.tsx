import { useEffect, useMemo, useRef, useState } from "react";
import DOMPurify from "dompurify";
import { parseDrawingContent, type DrawingContent } from "./content";
import { loadWebsandbox, type SandboxInstance } from "./websandbox-loader";
import { buildFinalFrameContent, buildPreviewBodyMorph, MEASUREMENT_JS, RESIZE_MESSAGE_TYPE } from "./frame-content";
import { processPartialHtml } from "./process-partial-html";
import { IDIOMORPH_JS } from "./idiomorph-inline";
import { applyDrawingTheme, drawingTheme } from "./theme";

const ERROR_MESSAGE_TYPE = "autonomics-drawing-error";
const ERROR_REPORTER = `addEventListener('error', e => parent.postMessage({type:'${ERROR_MESSAGE_TYPE}', message:e.message || '绘图脚本执行失败'}, '*'));addEventListener('unhandledrejection', e => parent.postMessage({type:'${ERROR_MESSAGE_TYPE}', message:String(e.reason)}, '*'));`;
const heightFor = (height: unknown) => typeof height === "number" && Number.isFinite(height) ? Math.max(100, Math.min(4000, Math.ceil(height))) : 300;

// Adapted from CopilotKit/OpenIntelligentUI's preview/final sandbox lifecycle.
// All mutations share one queue; completed JS expressions execute once in order.
function DrawingFrame({ content }: { content: DrawingContent }) {
  const container = useRef<HTMLDivElement>(null);
  const latest = useRef(content);
  const sync = useRef<(() => void) | null>(null);
  const [height, setHeight] = useState(heightFor(content.initialHeight));
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  const fullHtml = content.htmlComplete ? content.html?.join("") : undefined;
  const css = content.cssComplete ? content.css ?? "" : undefined;
  const hasBody = !!content.html?.join("").trim();

  useEffect(() => { latest.current = content; sync.current?.(); }, [content]);

  useEffect(() => {
    const host = container.current;
    if (!host || css === undefined || !hasBody) return;
    let cancelled = false;
    let sandbox: SandboxInstance | undefined;
    let queue = Promise.resolve();
    let expressionsRun = 0;
    let functionsInstalled = false;
    let previousBody = "";
    let previousTheme = "";
    setError(null);
    setReady(false);
    setHeight(heightFor(latest.current.initialHeight));
    const fail = (cause: unknown) => { if (!cancelled) setError(String(cause)); };
    const enqueue = (task: () => Promise<void>) => {
      queue = queue.then(async () => { if (!cancelled) await task(); }).catch(fail);
    };
    const update = () => enqueue(async () => {
      if (!sandbox) return;
      const next = latest.current;
      const theme = drawingTheme(host);
      if (theme !== previousTheme) {
        await sandbox.run(applyDrawingTheme(theme));
        previousTheme = theme;
      }
      if (fullHtml === undefined) {
        const body = DOMPurify.sanitize(processPartialHtml(next.html?.join("") ?? ""));
        if (body !== previousBody) {
          await sandbox.run(buildPreviewBodyMorph(body));
          previousBody = body;
        }
      } else {
        if (!functionsInstalled && next.jsFunctionsComplete) {
          if (next.jsFunctions) await sandbox.run(next.jsFunctions);
          functionsInstalled = true;
        }
        if (functionsInstalled) {
          const expressions = next.jsExpressions ?? [];
          while (expressionsRun < expressions.length) {
            const expression = expressions[expressionsRun++];
            await sandbox.run(expression);
          }
        }
      }
    });
    const onMessage = (event: MessageEvent) => {
      if (!sandbox || event.source !== sandbox.iframe.contentWindow) return;
      if (event.data?.type === RESIZE_MESSAGE_TYPE && typeof event.data.height === "number" && Number.isFinite(event.data.height)) setHeight(heightFor(event.data.height));
      if (event.data?.type === ERROR_MESSAGE_TYPE) fail(event.data.message);
    };
    window.addEventListener("message", onMessage);
    const themeObserver = new MutationObserver(() => sync.current?.());
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "style", "data-theme"] });
    const timeout = window.setTimeout(() => fail("绘图加载超时，请重试。"), 15000);
    void (async () => {
      const Websandbox = await loadWebsandbox();
      if (cancelled) return;
      sandbox = Websandbox.create({}, {
        frameContainer: host,
        frameContent: buildFinalFrameContent(fullHtml ?? "<body></body>", css),
        allowAdditionalAttributes: "",
      });
      sandbox.iframe.title = "交互式绘图";
      sandbox.iframe.style.cssText = "width:100%;height:100%;border:0;background:transparent";
      await sandbox.promise;
      if (cancelled) return;
      await sandbox.run(ERROR_REPORTER);
      await sandbox.run(applyDrawingTheme(drawingTheme(host)));
      await sandbox.run(MEASUREMENT_JS);
      if (fullHtml === undefined) await sandbox.run(IDIOMORPH_JS);
      if (cancelled) return;
      window.clearTimeout(timeout);
      sync.current = update;
      update();
      setReady(true);
    })().catch(fail);
    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
      sync.current = null;
      themeObserver.disconnect();
      window.removeEventListener("message", onMessage);
      sandbox?.destroy();
    };
  }, [fullHtml, css, hasBody]);

  return <section className="min-w-0 space-y-2" aria-label="交互式解释" aria-busy={content.generating}>
    {!ready && !error && <p role="status" className="text-xs text-muted-foreground">{content.placeholderMessages?.[0] || "正在绘制交互图…"}</p>}
    {error && <div role="alert" className="rounded border border-destructive/30 p-3 text-sm text-destructive">绘图未能完整运行：{error}</div>}
    <div ref={container} className="w-full overflow-hidden rounded-lg" style={{ height, display: css === undefined || !hasBody ? "none" : undefined }} />
    {content.generating && ready && <p role="status" className="text-xs text-muted-foreground">正在补充图形与交互…</p>}
  </section>;
}

export default function GenerativeUI({ code, streaming }: { code: string; streaming: boolean }) {
  const parsed = useMemo(() => {
    try { return { content: parseDrawingContent(code, streaming), error: null }; }
    catch (cause) { return { content: null, error: String(cause) }; }
  }, [code, streaming]);
  const lastGood = useRef<DrawingContent | null>(null);
  useEffect(() => { if (parsed.content) lastGood.current = parsed.content; }, [parsed]);
  const content = parsed.content ?? (streaming ? lastGood.current : null);
  if (!content) return <div role={streaming ? "status" : "alert"} className="text-sm text-muted-foreground">{streaming ? "正在准备交互图…" : `无法解析绘图回复：${parsed.error}`}</div>;
  return <DrawingFrame content={content} />;
}
