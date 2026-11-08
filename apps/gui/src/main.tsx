import ReactDOM from "react-dom/client";
import "./app.css";
import { App } from "@/app/app-view";
import { ContextMenuPolicy } from "@/app/context-menu-policy";
import { Toaster } from "@/components/ui/sonner";
import { initTheme } from "@/lib/theme";

const container = document.querySelector<HTMLDivElement>("#app");
if (!container) throw new Error("#app missing");
// Light / dark / follow-system, persisted in localStorage. The shell
// defaults to dark until the user picks otherwise.
initTheme();
ReactDOM.createRoot(container).render(
  <>
    <ContextMenuPolicy />
    <App />
    <Toaster position="bottom-center" />
  </>,
);
