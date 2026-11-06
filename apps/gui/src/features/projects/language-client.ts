import * as monaco from "monaco-editor";
import { gatewayRequest } from "@/gateway";
import { parseWorkspaceEdit, type FileEdits } from "./workspace-edit";
import { subscribeProjectChanges, type ProjectFileChange } from "./watch-events";
import picomatch from "picomatch";

type Message = { jsonrpc?: string; id?: number | string; method?: string; params?: any; result?: any; error?: { code?: number; message?: string } };
type Range = { start: { line: number; character: number }; end: { line: number; character: number } };
const range = (r: Range) => new monaco.Range(r.start.line + 1, r.start.character + 1, r.end.line + 1, r.end.character + 1);
const position = (p: monaco.Position) => ({ line: p.lineNumber - 1, character: p.column - 1 });
const completionKinds = [monaco.languages.CompletionItemKind.Text, monaco.languages.CompletionItemKind.Method, monaco.languages.CompletionItemKind.Function, monaco.languages.CompletionItemKind.Constructor, monaco.languages.CompletionItemKind.Field, monaco.languages.CompletionItemKind.Variable, monaco.languages.CompletionItemKind.Class, monaco.languages.CompletionItemKind.Interface, monaco.languages.CompletionItemKind.Module, monaco.languages.CompletionItemKind.Property, monaco.languages.CompletionItemKind.Unit, monaco.languages.CompletionItemKind.Value, monaco.languages.CompletionItemKind.Enum, monaco.languages.CompletionItemKind.Keyword, monaco.languages.CompletionItemKind.Snippet, monaco.languages.CompletionItemKind.Color, monaco.languages.CompletionItemKind.File, monaco.languages.CompletionItemKind.Reference, monaco.languages.CompletionItemKind.Folder, monaco.languages.CompletionItemKind.EnumMember, monaco.languages.CompletionItemKind.Constant, monaco.languages.CompletionItemKind.Struct, monaco.languages.CompletionItemKind.Event, monaco.languages.CompletionItemKind.Operator, monaco.languages.CompletionItemKind.TypeParameter];

export class LanguageClient {
  private readonly base: string;
  private readonly uri: string;
  private client = "";
  private stopped = false;
  private timer = 0;
  private offset = 0;
  private id = 0;
  private queue: Promise<void> = Promise.resolve();
  private pending = new Map<number, { resolve: (value: any) => void; reject: (reason: Error) => void; timer: number }>();
  private disposables: monaco.IDisposable[] = [];
  private version = 1;
  private ready = false;
  private saveOptions: boolean | { includeText?: boolean } = false;
  private canRename = false;
  private watchers = new Map<string, { match: (path: string) => boolean; kind: number }[]>();

  constructor(private readonly project: string, private readonly language: string, private readonly path: string, private readonly model: monaco.editor.ITextModel, private readonly status: (message: string) => void, private readonly applyEdits: (edits: FileEdits[], valid: () => boolean) => Promise<void>) {
    this.base = `/plugin-projects/${encodeURIComponent(project)}/language/${encodeURIComponent(language)}`;
    this.uri = monaco.Uri.file(`/work/${path}`).toString();
  }

  async start() {
    try {
      this.status("正在连接语言服务…");
      const opened = await gatewayRequest("POST", this.base);
      if (typeof opened.client !== "string" || !opened.client) throw new Error("服务器未返回语言服务会话");
      this.client = opened.client;
      if (this.stopped) { void gatewayRequest("DELETE", `${this.base}/${this.client}`).catch(() => {}); return; }
      this.disposables.push({ dispose: subscribeProjectChanges(this.project, changes => this.filesChanged(changes)) });
      void this.poll();
      const init = await this.request("initialize", { processId: null, rootUri: "file:///work", workspaceFolders: [{ uri: "file:///work", name: "workspace" }], capabilities: { textDocument: { synchronization: { didSave: true }, completion: { completionItem: { snippetSupport: true } }, hover: { contentFormat: ["markdown", "plaintext"] }, rename: {}, codeAction: { codeActionLiteralSupport: { codeActionKind: { valueSet: ["", "quickfix", "refactor", "source"] } } }, publishDiagnostics: { relatedInformation: false } }, workspace: { configuration: true, applyEdit: true, workspaceEdit: { documentChanges: true }, didChangeWatchedFiles: { dynamicRegistration: true, relativePatternSupport: true } } }, clientInfo: { name: "Autonomics" } });
      if (this.stopped) return;
      await this.notify("initialized", {});
      const openedText = this.model.getValue();
      const languageId = /\.tsx$/i.test(this.path) && this.language === "typescript" ? "typescriptreact" : /\.jsx$/i.test(this.path) && this.language === "javascript" ? "javascriptreact" : this.language;
      await this.notify("textDocument/didOpen", { textDocument: { uri: this.uri, languageId, version: this.version, text: openedText } });
      if (this.stopped) return;
      this.disposables.push(this.model.onDidChangeContent(() => { this.version++; void this.notify("textDocument/didChange", { textDocument: { uri: this.uri, version: this.version }, contentChanges: [{ text: this.model.getValue() }] }).catch(error => this.fail(error)); }));
      if (this.model.getValue() !== openedText) { this.version++; await this.notify("textDocument/didChange", { textDocument: { uri: this.uri, version: this.version }, contentChanges: [{ text: this.model.getValue() }] }); }
      if (this.stopped) return;
      this.registerProviders(init?.capabilities ?? {});
      this.saveOptions = typeof init?.capabilities?.textDocumentSync === "object" ? init.capabilities.textDocumentSync.save ?? false : false;
      this.ready = true;
      this.status(`${this.language} 语言服务已连接`);
    } catch (error) { this.fail(error); }
  }

  private send(message: Message): Promise<void> {
    if (this.stopped || !this.client) return Promise.reject(new Error("语言服务未连接"));
    const result = this.queue.then(() => gatewayRequest("POST", `${this.base}/${this.client}`, message)).then(() => {});
    this.queue = result.catch(() => {});
    return result;
  }
  private notify(method: string, params: any) { return this.send({ jsonrpc: "2.0", method, params }); }
  private request(method: string, params: any): Promise<any> {
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      const timer = window.setTimeout(() => { this.pending.delete(id); reject(new Error(`${method} 超时`)); }, 15000);
      this.pending.set(id, { resolve, reject, timer });
      void this.send({ jsonrpc: "2.0", id, method, params }).catch(error => { window.clearTimeout(timer); this.pending.delete(id); reject(error); });
    });
  }
  private async poll() {
    try {
      const result = await gatewayRequest("GET", `${this.base}/${this.client}?offset=${this.offset}`);
      if (this.stopped) return;
      if (result.truncated) throw new Error("语言服务消息队列溢出，请重新连接");
      this.offset = result.offset;
      for (const message of result.messages as Message[]) this.handle(message);
      if (result.exited) throw new Error("语言服务器已退出");
      this.timer = window.setTimeout(() => void this.poll(), 120);
    } catch (error) { this.fail(error); }
  }
  private handle(message: Message) {
    if (message.method === "textDocument/publishDiagnostics" && message.params?.uri && monaco.Uri.parse(message.params.uri).path === monaco.Uri.parse(this.uri).path) {
      const severities = [monaco.MarkerSeverity.Error, monaco.MarkerSeverity.Warning, monaco.MarkerSeverity.Info, monaco.MarkerSeverity.Hint];
      monaco.editor.setModelMarkers(this.model, "container-lsp", (message.params.diagnostics ?? []).map((d: any) => ({ ...range(d.range), message: String(d.message), severity: severities[(d.severity ?? 1) - 1] ?? monaco.MarkerSeverity.Error, source: d.source, code: d.code === undefined ? undefined : String(d.code) })));
    }
    if (message.method && message.id !== undefined) {
      if (message.method === "workspace/applyEdit") {
        void this.applyWorkspaceEdit(message.params?.edit).then(() => this.send({ jsonrpc: "2.0", id: message.id, result: { applied: true } }), error => this.send({ jsonrpc: "2.0", id: message.id, result: { applied: false, failureReason: String(error) } })).catch(error => this.fail(error));
        return;
      }
      if (message.method === "client/registerCapability") {
        try { this.registerWatchers(message.params?.registrations ?? []); void this.send({ jsonrpc: "2.0", id: message.id, result: null }).catch(error => this.fail(error)); }
        catch (error) { void this.send({ jsonrpc: "2.0", id: message.id, error: { code: -32602, message: String(error) } }).catch(cause => this.fail(cause)); }
        return;
      }
      if (message.method === "client/unregisterCapability") {
        for (const item of message.params?.unregisterations ?? message.params?.unregistrations ?? []) if (typeof item?.id === "string") this.watchers.delete(item.id);
        void this.send({ jsonrpc: "2.0", id: message.id, result: null }).catch(error => this.fail(error));
        return;
      }
      const supported = ["workspace/configuration", "workspace/workspaceFolders", "client/registerCapability", "client/unregisterCapability", "window/workDoneProgress/create", "workspace/applyEdit"].includes(message.method);
      const result = message.method === "workspace/configuration" ? (message.params?.items ?? []).map(() => ({})) : message.method === "workspace/workspaceFolders" ? [{ uri: "file:///work", name: "workspace" }] : null;
      void this.send(supported ? { jsonrpc: "2.0", id: message.id, result } : { jsonrpc: "2.0", id: message.id, error: { message: "Method not supported", code: -32601 } }).catch(error => this.fail(error));
    } else if (!message.method && typeof message.id === "number") {
      const pending = this.pending.get(message.id);
      if (pending) { this.pending.delete(message.id); window.clearTimeout(pending.timer); message.error ? pending.reject(new Error(message.error.message ?? "语言服务请求失败")) : pending.resolve(message.result); }
    }
  }
  private registerWatchers(registrations: any[]) {
    if (!Array.isArray(registrations)) throw new Error("文件监控注册无效");
    const updates: [string, { match: (path: string) => boolean; kind: number }[]][] = [];
    for (const registration of registrations) {
      if (registration?.method !== "workspace/didChangeWatchedFiles") continue;
      if (typeof registration.id !== "string" || !Array.isArray(registration.registerOptions?.watchers) || registration.registerOptions.watchers.length > 100) throw new Error("文件监控注册无效");
      const watchers = registration.registerOptions.watchers.map((watcher: any) => {
        const glob = watcher?.globPattern;
        const pattern = typeof glob === "string" ? glob : glob?.pattern;
        if (typeof pattern !== "string" || !pattern || pattern.length > 512) throw new Error("文件监控 glob 无效");
        const match = picomatch(pattern, { dot: true });
        let base = "";
        if (typeof glob !== "string") {
          const uri = typeof glob?.baseUri === "string" ? glob.baseUri : glob?.baseUri?.uri;
          if (typeof uri !== "string") throw new Error("文件监控根目录无效");
          const parsed = monaco.Uri.parse(uri);
          if (parsed.scheme !== "file" || parsed.authority || parsed.query || parsed.fragment || !(parsed.path === "/work" || parsed.path.startsWith("/work/")) || parsed.path.split("/").some(part => part === "." || part === "..")) throw new Error("文件监控根目录超出项目");
          base = parsed.path.slice(5).replace(/^\/+|\/+$/g, "");
        }
        const kind = watcher.kind ?? 7;
        if (!Number.isInteger(kind) || kind < 1 || kind > 7) throw new Error("文件监控类型无效");
        return { kind, match: (path: string) => {
          if (base && !path.startsWith(`${base}/`)) return false;
          const relative = base ? path.slice(base.length + 1) : path;
          return match(relative) || (typeof glob === "string" && match(`/work/${path}`));
        } };
      });
      updates.push([registration.id, watchers]);
    }
    for (const [id, watchers] of updates) this.watchers.set(id, watchers);
  }
  private filesChanged(changes: ProjectFileChange[]) {
    if (this.stopped || !this.watchers.size) return;
    const watchers = [...this.watchers.values()].flat();
    const selected = changes.filter(change => watchers.some(watcher => (watcher.kind & (1 << (change.type - 1))) !== 0 && watcher.match(change.path))).map(change => ({ uri: monaco.Uri.file(`/work/${change.path}`).toString(), type: change.type }));
    if (selected.length) void this.notify("workspace/didChangeWatchedFiles", { changes: selected }).catch(error => this.fail(error));
  }
  private registerProviders(capabilities: any) {
    const selector = this.language;
    const doc = { uri: this.uri };
    this.canRename = !!capabilities.renameProvider;
    if (capabilities.codeActionProvider) {
      const commandId = `container-lsp-action:${this.client}`;
      this.disposables.push(monaco.editor.registerCommand(commandId, (_accessor, action: any, version: number) => {
        void (async () => {
          if (version !== this.version) throw new Error("文件已更改，请重新获取代码操作");
          if (capabilities.codeActionProvider?.resolveProvider && action.data !== undefined) action = await this.request("codeAction/resolve", action);
          if (version !== this.version) throw new Error("文件已更改，请重新获取代码操作");
          if (action.edit) await this.applyWorkspaceEdit(action.edit, version);
          const command = typeof action.command === "string" ? action : action.command;
          if (command?.command) await this.request("workspace/executeCommand", { command: command.command, arguments: command.arguments ?? [] });
        })().catch(error => this.status(`代码操作失败：${String(error)}`));
      }));
      this.disposables.push(monaco.languages.registerCodeActionProvider(selector, {
        provideCodeActions: async (model, selection, context) => {
          if (model !== this.model || this.stopped) return { actions: [], dispose() {} };
          const diagnostics = context.markers.map(marker => ({ range: { start: { line: marker.startLineNumber - 1, character: marker.startColumn - 1 }, end: { line: marker.endLineNumber - 1, character: marker.endColumn - 1 } }, message: marker.message, severity: marker.severity === monaco.MarkerSeverity.Error ? 1 : marker.severity === monaco.MarkerSeverity.Warning ? 2 : 3 }));
          const version = this.version;
          let actions;
          try { actions = await this.request("textDocument/codeAction", { textDocument: doc, range: { start: { line: selection.startLineNumber - 1, character: selection.startColumn - 1 }, end: { line: selection.endLineNumber - 1, character: selection.endColumn - 1 } }, context: { diagnostics, ...(context.only ? { only: [context.only] } : {}) } }); }
          catch (error) { if (this.stopped) return { actions: [], dispose() {} }; throw error; }
          return { actions: (actions ?? []).map((action: any) => ({ title: action.title, kind: action.kind, isPreferred: action.isPreferred, disabled: action.disabled?.reason, command: { id: commandId, title: action.title, arguments: [action, version] } })), dispose() {} };
        },
      }));
    }
    if (capabilities.completionProvider) this.disposables.push(monaco.languages.registerCompletionItemProvider(selector, {
      triggerCharacters: capabilities.completionProvider.triggerCharacters,
      provideCompletionItems: async (model, pos) => {
        if (model !== this.model || this.stopped) return { suggestions: [] };
        const value = await this.request("textDocument/completion", { textDocument: doc, position: position(pos) });
        const word = model.getWordUntilPosition(pos);
        const fallback = new monaco.Range(pos.lineNumber, word.startColumn, pos.lineNumber, word.endColumn);
        const items = Array.isArray(value) ? value : value?.items ?? [];
        return { incomplete: value?.isIncomplete === true, suggestions: items.map((item: any) => ({ label: item.label, kind: completionKinds[(item.kind ?? 1) - 1] ?? monaco.languages.CompletionItemKind.Text, insertText: item.textEdit?.newText ?? item.insertText ?? item.label, range: item.textEdit?.range ? range(item.textEdit.range) : item.textEdit?.replace ? range(item.textEdit.replace) : fallback, insertTextRules: item.insertTextFormat === 2 ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet : undefined, additionalTextEdits: item.additionalTextEdits?.map((edit: any) => ({ range: range(edit.range), text: edit.newText })), detail: item.detail, documentation: typeof item.documentation === "string" ? item.documentation : item.documentation?.value, sortText: item.sortText, filterText: item.filterText })) };
      },
    }));
    if (capabilities.hoverProvider) this.disposables.push(monaco.languages.registerHoverProvider(selector, {
      provideHover: async (model, pos) => {
        if (model !== this.model || this.stopped) return null;
        const value = await this.request("textDocument/hover", { textDocument: doc, position: position(pos) });
        if (!value?.contents) return null;
        const contents = (Array.isArray(value.contents) ? value.contents : [value.contents]).map((item: any) => ({ value: typeof item === "string" ? item : item.value ?? "" }));
        return { contents, range: value.range ? range(value.range) : undefined };
      },
    }));
    if (capabilities.documentFormattingProvider) this.disposables.push(monaco.languages.registerDocumentFormattingEditProvider(selector, {
      provideDocumentFormattingEdits: async (model, options) => {
        if (model !== this.model || this.stopped) return [];
        const edits = await this.request("textDocument/formatting", { textDocument: doc, options: { tabSize: options.tabSize, insertSpaces: options.insertSpaces } });
        return (edits ?? []).map((edit: any) => ({ range: range(edit.range), text: edit.newText }));
      },
    }));
    if (capabilities.signatureHelpProvider) this.disposables.push(monaco.languages.registerSignatureHelpProvider(selector, {
      signatureHelpTriggerCharacters: capabilities.signatureHelpProvider.triggerCharacters,
      signatureHelpRetriggerCharacters: capabilities.signatureHelpProvider.retriggerCharacters,
      provideSignatureHelp: async (model, pos) => {
        if (model !== this.model || this.stopped) return null;
        const value = await this.request("textDocument/signatureHelp", { textDocument: doc, position: position(pos) });
        if (!value) return null;
        return { value: { activeSignature: value.activeSignature ?? 0, activeParameter: value.activeParameter ?? 0, signatures: (value.signatures ?? []).map((signature: any) => ({ label: signature.label, documentation: typeof signature.documentation === "string" ? signature.documentation : signature.documentation?.value, parameters: (signature.parameters ?? []).map((parameter: any) => ({ label: parameter.label, documentation: typeof parameter.documentation === "string" ? parameter.documentation : parameter.documentation?.value })) })) }, dispose() {} };
      },
    }));
    const locations = (value: any): monaco.languages.Location[] => (Array.isArray(value) ? value : value ? [value] : []).flatMap((item: any) => {
      const target = item.targetUri ?? item.uri;
      const targetRange = item.targetSelectionRange ?? item.range;
      if (typeof target !== "string" || !targetRange) return [];
      const uri = monaco.Uri.parse(target);
      if (uri.scheme !== "file" || !uri.path.startsWith("/work/")) return [];
      return [{ uri: monaco.Uri.from({ scheme: "inmemory", authority: "plugin-projects", path: `/${this.project}/${uri.path.slice(6)}` }), range: range(targetRange) }];
    });
    if (capabilities.definitionProvider) this.disposables.push(monaco.languages.registerDefinitionProvider(selector, {
      provideDefinition: async (model, pos) => model === this.model && !this.stopped ? locations(await this.request("textDocument/definition", { textDocument: doc, position: position(pos) })) : [],
    }));
    if (capabilities.referencesProvider) this.disposables.push(monaco.languages.registerReferenceProvider(selector, {
      provideReferences: async (model, pos, context) => model === this.model && !this.stopped ? locations(await this.request("textDocument/references", { textDocument: doc, position: position(pos), context })) : [],
    }));
  }
  private async applyWorkspaceEdit(value: unknown, expectedVersion = this.version) {
    const edits = parseWorkspaceEdit(value);
    if (edits.some(edit => edit.version !== null && (edit.path !== this.path || edit.version !== expectedVersion))) throw new Error("语言服务文件版本已过期或无法验证");
    await this.applyEdits(edits, () => !this.stopped && this.version === expectedVersion);
  }
  async rename(positionValue: monaco.Position, name: string) {
    if (!this.ready || !this.canRename || this.stopped) { this.status("当前语言服务不支持重命名"); return; }
    const version = this.version;
    try {
      const edit = await this.request("textDocument/rename", { textDocument: { uri: this.uri }, position: position(positionValue), newName: name });
      if (edit) { await this.applyWorkspaceEdit(edit, version); this.status("重命名已应用，请保存修改的文件"); }
    } catch (error) { this.status(`重命名失败：${String(error)}`); }
  }
  private fail(error: unknown) {
    if (this.stopped) return;
    this.status(`语言服务不可用：${String(error)}`);
    this.dispose();
  }
  saved() {
    if (this.ready && !this.stopped && this.saveOptions) void this.notify("textDocument/didSave", { textDocument: { uri: this.uri }, ...(typeof this.saveOptions === "object" && this.saveOptions.includeText ? { text: this.model.getValue() } : {}) }).catch(error => this.fail(error));
  }
  dispose() {
    if (this.stopped) return;
    this.stopped = true;
    window.clearTimeout(this.timer);
    for (const disposable of this.disposables) disposable.dispose();
    for (const pending of this.pending.values()) { window.clearTimeout(pending.timer); pending.reject(new Error("语言服务已关闭")); }
    this.pending.clear();
    if (!this.model.isDisposed()) monaco.editor.setModelMarkers(this.model, "container-lsp", []);
    if (this.client) void gatewayRequest("DELETE", `${this.base}/${this.client}`).catch(() => {});
  }
}
