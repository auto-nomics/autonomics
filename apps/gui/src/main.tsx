import ReactDOM from "react-dom/client";
import "./app.css";
import { App } from "@/app/app-view";
import { Toaster } from "@/components/ui/sonner";

const container = document.querySelector<HTMLDivElement>("#app");
if (!container) throw new Error("#app missing");
document.documentElement.classList.add("dark");
ReactDOM.createRoot(container).render(
  <>
    <App />
    <Toaster position="bottom-center" />
  </>,
);
