import { useEffect, useRef, useState } from "react";

import { ScrollArea } from "@/components/ui/scroll-area";

interface PdfViewerProps {
  src: string;
  title?: string;
}

// Minimal PDF.js/embed-pdf-viewer integration backed by the official
// snippet hosted at https://snippet.embedpdf.com. The script exposes a
// default export with an `init` factory that mounts a full viewer into a
// target element. We lazy-load it on first render so the rest of the
// article detail stays lightweight until the user actually opens a PDF.
export function PdfViewer({ src, title }: PdfViewerProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewerRef = useRef<{ destroy?: () => void } | null>(null);
  const [status, setStatus] = useState<"idle" | "loading" | "ready" | "error">(
    "idle",
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setStatus("loading");
    setError(null);

    (async () => {
      try {
        const module = (await import(
          /* @vite-ignore */ "https://snippet.embedpdf.com/embedpdf.js"
        )) as { default: EmbedPdfFactory };
        if (cancelled) return;
        if (!containerRef.current) return;
        const factory = module.default;
        viewerRef.current = factory.init({
          type: "container",
          target: containerRef.current,
          src,
          worker: true,
          tabBar: "always",
          theme: { preference: "system" },
        });
        setStatus("ready");
      } catch (failure) {
        if (cancelled) return;
        setError(failure instanceof Error ? failure.message : String(failure));
        setStatus("error");
      }
    })();

    return () => {
      cancelled = true;
      const viewer = viewerRef.current;
      if (viewer && typeof viewer.destroy === "function") {
        try {
          viewer.destroy();
        } catch {
          // ignore: viewer may already be detached
        }
      }
      viewerRef.current = null;
      if (containerRef.current) {
        containerRef.current.innerHTML = "";
      }
    };
  }, [src]);

  return (
    <div className="flex h-[640px] flex-col overflow-hidden rounded-md border">
      <ScrollArea className="h-full">
        <div
          ref={containerRef}
          data-pdf-src={src}
          data-pdf-title={title ?? "PDF"}
          className="min-h-[640px] w-full"
        />
      </ScrollArea>
      {status === "loading" && (
        <p className="border-t bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
          Loading PDF viewer…
        </p>
      )}
      {status === "error" && (
        <p className="border-t bg-destructive/10 px-3 py-2 text-xs text-destructive">
          Failed to render PDF: {error ?? "unknown error"}
        </p>
      )}
    </div>
  );
}

interface EmbedPdfFactory {
  init: (options: {
    type: "container";
    target: HTMLElement;
    src: string;
    worker: boolean;
    tabBar: "always" | "never" | "auto";
    theme: { preference: "system" | "light" | "dark" };
  }) => { destroy?: () => void } | undefined;
}
