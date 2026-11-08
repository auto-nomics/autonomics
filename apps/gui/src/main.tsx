import ReactDOM from "react-dom/client";
import "./app.css";
import { App } from "@/app/app-view";
import { ContextMenuPolicy } from "@/app/context-menu-policy";
import { Toaster } from "@/components/ui/sonner";
import { initTheme } from "@/lib/theme";

const container = document.querySelector<HTMLDivElement>("#app");
if (!container) throw new Error("#app missing");
if (new URLSearchParams(location.search).has("deckPresenter")) {
  void import("@/features/deck/presenter/presenter-main").then(m => m.bootPresenterPage(container));
} else if (location.pathname === "/deck/print") {
  void import("@/features/deck/print/print-main").then(m => m.bootPrintPage(container));
} else if (location.pathname === "/deck/presenter") {
  void import("@/features/deck/presenter/presenter-main").then(m => m.bootPresenterPage(container));
} else {
  initTheme();
  ReactDOM.createRoot(container).render(
    <>
      <ContextMenuPolicy />
      <App />
      <Toaster position="bottom-center" />
    </>,
  );
}
