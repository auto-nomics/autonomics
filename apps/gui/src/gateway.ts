import { invoke } from "@tauri-apps/api/core";

export type GatewayProbe = {
  running: boolean;
  addr: string;
  token_found: boolean;
};

export async function gatewayRequest(
  method: string,
  path: string,
  body?: unknown,
): Promise<any> {
  const result = await invoke<{ data: any }>("api", {
    method,
    path,
    body: body === undefined ? null : body,
  });
  return result.data;
}

export async function gatewayRequestOptional(path: string): Promise<any | null> {
  try {
    return await gatewayRequest("GET", path);
  } catch (error) {
    if (String(error).includes("404")) return null;
    throw error;
  }
}

export async function probeGateway(): Promise<GatewayProbe> {
  return invoke<GatewayProbe>("probe");
}

export async function startGatewayEvents(lastSeq: number): Promise<boolean> {
  return invoke<boolean>("start_events", { lastSeq });
}

export async function uploadBibFulltext(
  articleId: string,
  path: string,
): Promise<any> {
  return invoke("upload_bib_fulltext", {
    request: { articleId, path },
  });
}

export async function saveBibExport(
  path: string,
  contents: string,
): Promise<number> {
  return invoke<number>("save_bib_export", {
    request: { path, contents },
  });
}

export function reportFrontendReady(stage: string, detail: Record<string, unknown>): void {
  void invoke("frontend_ready", { stage, detail }).catch(() => {});
}
