import { Footer } from "@excalidraw/excalidraw";
import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import "./canvas-bottom-actions.css";

/** Desktop uses the public Footer slot; mobile adapts the native bottom bar.
 * Reuse one portal host across layout changes, preserving open dialog state. */
export function CanvasBottomActions({ surface, children }: { surface: RefObject<HTMLDivElement | null>; children: ReactNode }) {
  const desktopSlot = useRef<HTMLDivElement>(null);
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  useEffect(() => {
    const canvas = surface.current;
    if (!canvas) return;
    const node = document.createElement("div");
    node.className = "canvas-native-footer-host";
    setHost(node);
    const attach = () => {
      const desktop = desktopSlot.current;
      const mobile = canvas.querySelector<HTMLElement>(".App-toolbar-content");
      const target = desktop?.isConnected ? desktop : mobile;
      if (!target || node.parentElement === target) return;
      if (target === mobile) target.insertBefore(node, target.lastElementChild);
      else target.appendChild(node);
    };
    attach();
    const observer = new MutationObserver(attach);
    observer.observe(canvas, { childList: true, subtree: true });
    return () => { observer.disconnect(); node.remove(); };
  }, [surface]);
  return <><Footer><div ref={desktopSlot} className="min-w-0 flex-1" /></Footer>{host && createPortal(children, host)}</>;
}
