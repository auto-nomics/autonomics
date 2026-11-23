import { Component, lazy, Suspense, useEffect, useMemo, useState, type ReactNode, type ErrorInfo } from "react";
import { useTranslation } from "react-i18next";
import { createResponseParser, type ResponsePart } from "./intelligent-response-parser";
const GenerativeUI = lazy(() => import("./generative-ui/view"));

type BoundaryProps = { children: ReactNode; fallback: ReactNode; resetKey: string };
class Boundary extends Component<BoundaryProps, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  componentDidCatch(error: Error, info: ErrorInfo) { console.error("Intelligent UI render failed", error, info.componentStack); }
  componentDidUpdate(previous: Readonly<BoundaryProps>) {
    if (this.state.failed && previous.resetKey !== this.props.resetKey) this.setState({ failed: false });
  }
  render() { return this.state.failed ? this.props.fallback : this.props.children; }
}

export function IntelligentResponse({ text, streaming, fallback }: { text: string; streaming: boolean; fallback: ReactNode }) {
  const { t } = useTranslation("agent");
  const parse = useMemo(createResponseParser, []);
  const [snapshot, setSnapshot] = useState<{ source: string; parts: ResponsePart[] } | null>(null);
  useEffect(() => {
    let cancelled = false;
    // Coalesce token bursts into a frame, including Comark's unfinished fences.
    const frame = requestAnimationFrame(() => {
      void parse(text, streaming).then(parts => {
        if (!cancelled) setSnapshot({ source: text, parts });
      }).catch(error => {
        console.error("Response parsing failed", error);
        if (!cancelled) setSnapshot(null);
      });
    });
    return () => { cancelled = true; cancelAnimationFrame(frame); };
  }, [parse, text, streaming]);
  const parts = snapshot && text.startsWith(snapshot.source) ? snapshot.parts : null;
  if (!parts) return fallback;
  return <div className="min-w-0 space-y-3">{parts.map((part, index) => {
    if (part.kind === "markdown") return <div key={index} className={`md${streaming ? " md-streaming" : ""}`} dangerouslySetInnerHTML={{ __html: part.content }} />;
    return <Boundary key={`${index}:${part.kind}`} resetKey={`${streaming}:${part.content}`} fallback={<div role="alert" className="text-sm text-muted-foreground">{t("generative.renderFailed")}<details><summary>{t("generative.showRaw")}</summary><pre className="whitespace-pre-wrap break-words">{part.content}</pre></details></div>}>
      <Suspense fallback={<p role="status" className="text-xs text-muted-foreground">{t("generative.loading")}</p>}><GenerativeUI code={part.content} streaming={streaming} /></Suspense>
    </Boundary>;
  })}</div>;
}
