import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Maximize2, Minus, X } from "lucide-react";
import { isTauri } from "@/lib/is-tauri";
import { Button } from "@/components/ui/button";

export function WindowControls({ vertical = false }: { vertical?: boolean }) {
  const { t } = useTranslation("workbench");
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | null = null;
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const window = getCurrentWindow();
        setMaximized(await window.isMaximized());
        const promise = window.onResized(async () => setMaximized(await window.isMaximized()));
        unlisten = () => void promise.then((fn) => fn());
      } catch {
        /* non-Tauri build: controls are not rendered anyway */
      }
    })();
    return () => unlisten?.();
  }, []);

  const withWindow = (action: (window: import("@tauri-apps/api/window").Window) => void) => () => {
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        action(getCurrentWindow());
      } catch { /* not in Tauri */ }
    })();
  };

  return (
    <div className={vertical ? "flex flex-col items-center" : "flex items-center"}>
      <Button size="icon-sm" variant="ghost" aria-label={t("windowControls.minimize")} title={t("windowControls.minimize")} onClick={withWindow((w) => w.minimize())}><Minus /></Button>
      <Button size="icon-sm" variant="ghost" aria-label={maximized ? t("windowControls.restore") : t("windowControls.maximize")} title={maximized ? t("windowControls.restore") : t("windowControls.maximize")} onClick={withWindow((w) => w.toggleMaximize())}>
        {maximized ? <MinimizeIcon /> : <Maximize2 />}
      </Button>
      <Button size="icon-sm" variant="ghost" aria-label={t("windowControls.closeAria")} title={t("windowControls.close")} className="hover:bg-destructive hover:text-white" onClick={withWindow((w) => w.close())}><X /></Button>
    </div>
  );
}

function MinimizeIcon() {
  // Restore (unmaximize): two overlapping rectangles.
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="size-4" aria-hidden>
      <path d="M8 8h12v12H8z" />
      <path d="M4 16V4h12" />
    </svg>
  );
}

