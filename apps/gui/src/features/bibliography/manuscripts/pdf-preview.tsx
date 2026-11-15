import { useEffect, useRef, useState } from "react";
import * as pdfjs from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import {
  ChevronLeft,
  ChevronRight,
  Download,
  FileText,
  Minus,
  Maximize2,
  Plus,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { saveBinaryFile } from "@/gateway";
pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
export function PdfPreview({
  bytes,
  title,
}: {
  bytes: Uint8Array | null;
  title: string;
}) {
  const preview = useRef<HTMLElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const paper = useRef<HTMLDivElement>(null);
  const [fit, setFit] = useState(true);
  const [document, setDocument] = useState<pdfjs.PDFDocumentProxy | null>(null),
    [page, setPage] = useState(1),
    [zoom, setZoom] = useState(1),
    [error, setError] = useState("");
  useEffect(() => {
    const element = preview.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
      event.preventDefault();
      if (!bytes || event.deltaY === 0) return;
      setFit(false);
      setZoom(current => Math.max(0.2, Math.min(2, current * Math.exp(-event.deltaY * 0.002))));
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [bytes]);
  useEffect(() => {
    setError("");
    setDocument(null);
    setPage(1);
    if (!bytes) return;
    let alive = true;
    const task = pdfjs.getDocument({ data: bytes.slice(), verbosity: pdfjs.VerbosityLevel.ERRORS });
    void task.promise
      .then((pdf) => {
        if (alive) setDocument(pdf);
      })
      .catch((error) => {
        if (alive) setError(String(error));
      });
    return () => {
      alive = false;
      void task.destroy();
    };
  }, [bytes]);
  useEffect(() => {
    if (!document || !paper.current || !fit) return;
    let alive = true;
    let width = 595;
    const resize = () => {
      if (alive && paper.current)
        setZoom(
          Math.max(0.2, Math.min(2, (paper.current.clientWidth - 44) / width)),
        );
    };
    const observer = new ResizeObserver(resize);
    observer.observe(paper.current);
    void document
      .getPage(page)
      .then((pdf) => {
        if (alive) {
          width = pdf.getViewport({ scale: 1 }).width;
          resize();
        }
      })
      .catch(() => {});
    return () => {
      alive = false;
      observer.disconnect();
    };
  }, [document, page, fit]);
  useEffect(() => {
    if (!document || !canvas.current) return;
    let alive = true;
    let task: pdfjs.RenderTask | undefined;
    void document
      .getPage(page)
      .then((pdf) => {
        if (!alive || !canvas.current) return;
        const viewport = pdf.getViewport({ scale: zoom });
        const ratio = window.devicePixelRatio || 1;
        const target = canvas.current;
        target.dataset.rendered = "false";
        target.width = viewport.width * ratio;
        target.height = viewport.height * ratio;
        target.style.width = `${viewport.width}px`;
        target.style.height = `${viewport.height}px`;
        task = pdf.render({
          canvasContext: target.getContext("2d")!,
          viewport,
          transform: [ratio, 0, 0, ratio, 0, 0],
        });
        return task.promise.then(() => {
          if (alive) target.dataset.rendered = "true";
        });
      })
      .catch((error) => {
        if (alive && error?.name !== "RenderingCancelledException")
          setError(String(error));
      });
    return () => {
      alive = false;
      task?.cancel();
    };
  }, [document, page, zoom]);
  const download = () => {
    if (bytes)
      void saveBinaryFile(`${title}.pdf`, bytes, "application/pdf").catch(
        (error) => setError(String(error)),
      );
  };
  return (
    <section ref={preview} className="manuscript-preview" aria-label="Compiled PDF">
      <div className="manuscript-pane-heading">
        <span>
          <FileText size={14} /> PDF 预览
        </span>
        <div className="manuscript-inline">
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Fit PDF width"
            title="适应宽度"
            onClick={() => setFit(true)}
          >
            <Maximize2 />
          </Button>
          <Button
            variant="ghost"
            size="icon-xs"
            title="缩小"
            aria-label="Zoom out"
            onClick={() => {
              setFit(false);
              setZoom(Math.max(0.2, zoom - 0.1));
            }}
          >
            <Minus />
          </Button>
          <span>{Math.round(zoom * 100)}%</span>
          <Button
            variant="ghost"
            size="icon-xs"
            title="放大"
            aria-label="Zoom in"
            onClick={() => {
              setFit(false);
              setZoom(Math.min(2, zoom + 0.1));
            }}
          >
            <Plus />
          </Button>
          <Button
            variant="ghost"
            size="icon-xs"
            title="下载 PDF"
            aria-label="Download PDF"
            disabled={!bytes}
            onClick={download}
          >
            <Download />
          </Button>
        </div>
      </div>
      {error && (
        <p role="alert" className="manuscript-error">
          {error}
        </p>
      )}
      {bytes ? (
        <>
          <div className="manuscript-paper-scroll" ref={paper}>
            <canvas ref={canvas} aria-label={`PDF page ${page}`} />
          </div>
          <div className="manuscript-pagination">
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Previous page"
              disabled={page <= 1}
              onClick={() => setPage(page - 1)}
            >
              <ChevronLeft />
            </Button>
            <span>
              {page} / {document?.numPages ?? "…"}
            </span>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Next page"
              disabled={!document || page >= document.numPages}
              onClick={() => setPage(page + 1)}
            >
              <ChevronRight />
            </Button>
          </div>
        </>
      ) : (
        <div className="manuscript-preview-empty">
          <FileText size={36} strokeWidth={1} />
          <h3>从源文件到论文</h3>
          <p>编译 LaTeX 后，PDF 将显示在这里。</p>
          <small>Tectonic · XeTeX / BibTeX</small>
        </div>
      )}
    </section>
  );
}
