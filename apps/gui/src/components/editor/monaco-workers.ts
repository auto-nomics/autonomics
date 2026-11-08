/** Worker configuration shared by all Monaco editors in the workbench. */
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker.js?worker&inline";
import JsonWorker from "monaco-editor/esm/vs/language/json/json.worker.js?worker&inline";
import CssWorker from "monaco-editor/esm/vs/language/css/css.worker.js?worker&inline";
import HtmlWorker from "monaco-editor/esm/vs/language/html/html.worker.js?worker&inline";

self.MonacoEnvironment = {
  getWorker(_moduleId, label) {
    if (label === "json") return new JsonWorker();
    if (["css", "scss", "less"].includes(label)) return new CssWorker();
    if (["html", "handlebars", "razor"].includes(label)) return new HtmlWorker();
    return new EditorWorker();
  },
};
