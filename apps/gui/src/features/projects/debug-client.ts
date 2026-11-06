import { gatewayRequest } from "@/gateway";

type Message = { type: string; seq?: number; request_seq?: number; command?: string; event?: string; success?: boolean; message?: string; body?: any };

/** A DAP connection whose adapter and debuggee both run in the project container. */
export class DebugClient {
  private readonly base: string;
  private client = "";
  private closed = false;
  private offset = 0;
  private sequence = 0;
  private timer = 0;
  private queue: Promise<void> = Promise.resolve();
  private pending = new Map<number, { resolve: (body: any) => void; reject: (error: Error) => void; timer: number }>();
  private initialized?: { resolve: () => void; reject: (error: Error) => void; timer: number };

  constructor(project: string, private readonly onEvent: (message: Message) => void, private readonly onError: (error: Error) => void) {
    this.base = `/plugin-projects/${encodeURIComponent(project)}/debug/python`;
  }

  async start(path: string, breakpoints: Record<string, number[]>) {
    const opened = await gatewayRequest("POST", this.base);
    if (typeof opened.client !== "string" || !opened.client) throw new Error("调试器未返回会话");
    this.client = opened.client;
    if (this.closed) { void gatewayRequest("DELETE", `${this.base}/${this.client}`).catch(() => {}); return; }
    void this.poll();
    await this.request("initialize", { clientID: "autonomics", adapterID: "python", pathFormat: "path", linesStartAt1: true, columnsStartAt1: true, supportsRunInTerminalRequest: false });
    const ready = new Promise<void>((resolve, reject) => {
      const timer = window.setTimeout(() => { this.initialized = undefined; reject(new Error("调试器初始化超时")); }, 20000);
      this.initialized = { resolve, reject, timer };
    });
    // debugpy responds to launch only after configurationDone.
    const launch = this.request("launch", { name: "Python", type: "python", request: "launch", program: `/work/${path}`, cwd: "/work", console: "internalConsole", justMyCode: true });
    // A launch error can arrive before the initialized event.
    await Promise.race([ready, launch.then(() => ready)]);
    for (const [file, lines] of Object.entries(breakpoints)) await this.setBreakpoints(file, lines);
    await this.request("configurationDone", {});
    await launch;
  }

  setBreakpoints(path: string, lines: number[]) {
    return this.request("setBreakpoints", { source: { path: `/work/${path}` }, breakpoints: lines.map(line => ({ line })) });
  }

  request(command: string, args: object): Promise<any> {
    const seq = ++this.sequence;
    return new Promise((resolve, reject) => {
      const timer = window.setTimeout(() => { this.pending.delete(seq); reject(new Error(`${command} 超时`)); }, 30000);
      this.pending.set(seq, { resolve, reject, timer });
      void this.send({ seq, type: "request", command, arguments: args }).catch(error => { window.clearTimeout(timer); this.pending.delete(seq); reject(error); });
    });
  }

  private send(message: object) {
    if (this.closed || !this.client) return Promise.reject(new Error("调试器未连接"));
    const result = this.queue.then(() => gatewayRequest("POST", `${this.base}/${this.client}`, message)).then(() => {});
    this.queue = result.catch(() => {});
    return result;
  }

  private async poll() {
    try {
      const result = await gatewayRequest("GET", `${this.base}/${this.client}?offset=${this.offset}`);
      if (this.closed) return;
      if (result.truncated) throw new Error("调试器消息队列溢出");
      this.offset = result.offset;
      for (const message of result.messages as Message[]) {
        if (message.type === "response" && typeof message.request_seq === "number") {
          const pending = this.pending.get(message.request_seq);
          if (pending) { this.pending.delete(message.request_seq); window.clearTimeout(pending.timer); message.success === false ? pending.reject(new Error(message.message ?? "调试请求失败")) : pending.resolve(message.body); }
        } else if (message.type === "event") {
          if (message.event === "initialized" && this.initialized) { window.clearTimeout(this.initialized.timer); this.initialized.resolve(); this.initialized = undefined; }
          this.onEvent(message);
        } else if (message.type === "request") {
          void this.send({ seq: ++this.sequence, type: "response", request_seq: message.seq, command: message.command, success: false, message: "不支持此调试器请求" });
        }
      }
      if (result.exited) throw new Error("调试器已退出");
      this.timer = window.setTimeout(() => void this.poll(), 120);
    } catch (cause) { if (!this.closed) { const error = cause instanceof Error ? cause : new Error(String(cause)); this.onError(error); this.dispose(); } }
  }

  dispose() {
    if (this.closed) return;
    this.closed = true;
    window.clearTimeout(this.timer);
    if (this.initialized) { window.clearTimeout(this.initialized.timer); this.initialized.reject(new Error("调试会话已关闭")); this.initialized = undefined; }
    for (const pending of this.pending.values()) { window.clearTimeout(pending.timer); pending.reject(new Error("调试会话已关闭")); }
    this.pending.clear();
    if (this.client) void gatewayRequest("DELETE", `${this.base}/${this.client}`).catch(() => {});
  }
}
