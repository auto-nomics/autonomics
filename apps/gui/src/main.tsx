import ReactDOM from "react-dom/client";
import "./app.css";
import { App } from "@/app/app-view";

const container = document.querySelector<HTMLDivElement>("#app");
if (!container) throw new Error("#app missing");
document.documentElement.classList.add("dark");
ReactDOM.createRoot(container).render(<App />);
