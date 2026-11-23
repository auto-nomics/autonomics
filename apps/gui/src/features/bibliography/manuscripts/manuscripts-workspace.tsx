import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem } from "@/components/ui/dropdown-menu";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  Ellipsis,
  BookOpen,
  Check,
  ChevronRight,
  Code2,
  Download,
  FilePlus2,
  FileText,
  History,
  ListTree,
  Loader2,
  MessageSquare,
  Play,
  Plus,
  RefreshCw,
  Save,
  Sparkles,
  Upload,
  Image,
  Trash2,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  gatewayRequest,
  fetchManuscriptArtifact,
  GatewayHttpError,
  saveBinaryFile,
  probeGateway,
} from "@/gateway";
import type { BibArticle } from "@/types";
import {
  SourceEditor,
  ProposalDiff,
  type SourceSelection,
} from "./source-editor";
import { PdfPreview } from "./pdf-preview";
import {
  draftStorageKey,
  getDraft,
  readDrafts,
  writeDrafts,
  type Draft,
} from "./draft-recovery";
import {
  projectPath,
  type Project,
  type Detail,
  type Revision,
  type SourceFile,
} from "./types";
import "./manuscripts.css";
import { publishManuscriptLog, openManuscriptLog } from "./manuscript-log";
type Build = {
  main_file: string;
  id: string;
  status: string;
  log: string;
  diagnostics: Array<{
    severity: string;
    message: string;
    file?: string;
    line?: number;
  }>;
  source_revisions: Record<string, number>;
};
type Panel = "review" | "comments" | "history" | "library" | null;
const message = (error: unknown) =>
  error instanceof Error ? error.message : String(error);
export function ManuscriptsWorkspace({
  active = true,
  initialArticleIds = [],
  sidebarSlot, externalSidebar = false, onOpenSidebar,
}: {
  active?: boolean;
  initialArticleIds?: string[];
  sidebarSlot?: HTMLElement | null;
  externalSidebar?: boolean;
  onOpenSidebar?: () => void;
}) {
  const { t } = useTranslation("bibliography");
  const [projects, setProjects] = useState<Project[]>([]),
    [detail, setDetail] = useState<Detail | null>(null),
    [path, setPath] = useState(""),
    [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [title, setTitle] = useState(""),
    [newFile, setNewFile] = useState(""),
    [showNewFile, setShowNewFile] = useState(false),
    [loading, setLoading] = useState(true),
    [busy, setBusy] = useState(""),
    [error, setError] = useState(""),
    [conflict, setConflict] = useState<SourceFile | null>(null);
  const [panel, setPanel] = useState<Panel>(null),
    [selection, setSelection] = useState<SourceSelection>({
      startLine: 1,
      endLine: 1,
      text: "",
    }),
    [navigation, setNavigation] = useState<{ line: number; id: number }>(),
    [insertion, setInsertion] = useState<{ text: string; id: number }>();
  const [history, setHistory] = useState<Revision[]>([]),
    [comment, setComment] = useState(""),
    [query, setQuery] = useState(""),
    [articles, setArticles] = useState<BibArticle[]>([]),
    [build, setBuild] = useState<Build | null>(null),
    [pdf, setPdf] = useState<Uint8Array | null>(null);
  useEffect(() => {
    if (!active || !detail) return;
    return publishManuscriptLog({ build, title: detail.project.title, navigate: (file, line) => {
      if (file) setPath(file);
      if (line) setNavigation({ line, id: Date.now() });
    } });
  }, [active, detail?.project.id, detail?.project.title, build]);
  const [recoveryKey, setRecoveryKey] = useState("");
  const [recoveryError, setRecoveryError] = useState("");
  const [recovered, setRecovered] = useState(false);
  const uploadInput = useRef<HTMLInputElement>(null);
  const mutation = useRef({ epoch: 0, busy: false });
  const state = useRef({ detail, drafts, path });
  state.current = { detail, drafts, path };
  const file = detail?.files.find((file) => file.path === path),
    draft = getDraft(drafts, path),
    value =
      draft && draft.content !== draft.base.content
        ? draft.content
        : (file?.content ?? "");
  const dirty = Object.values(drafts).some(
    (draft) => draft.content !== draft.base.content,
  );
  const dirtyPath = (target: string) =>
    Boolean(
      getDraft(drafts, target) &&
      getDraft(drafts, target)!.content !==
        getDraft(drafts, target)!.base.content,
    );
  const run = async (label: string, action: () => Promise<void>) => {
    if (mutation.current.busy) return;
    mutation.current.busy = true;
    mutation.current.epoch++;
    setBusy(label);
    setError("");
    try {
      await action();
    } catch (error) {
      setError(message(error));
    } finally {
      mutation.current.busy = false;
      setBusy("");
    }
  };
  const refresh = useCallback(async (id: string) => {
    const epoch = mutation.current.epoch;
    const next: Detail = await gatewayRequest("GET", projectPath(id));
    if (
      state.current.detail?.project.id !== id ||
      epoch !== mutation.current.epoch
    )
      return;
    // Retain locally edited files even if another collaborator deleted them.
    for (const draft of Object.values(state.current.drafts)) {
      if (
        draft.content !== draft.base.content &&
        !next.files.some((file) => file.path === draft.base.path)
      )
        next.files.push(draft.base);
    }
    setDetail(next);
  }, []);
  useEffect(() => {
    if (!active) return;
    let live = true;
    setLoading(true);
    gatewayRequest("GET", "/bib/manuscripts")
      .then((result) => {
        if (live) setProjects(result);
      })
      .catch((error) => {
        if (live) setError(message(error));
      })
      .finally(() => {
        if (live) setLoading(false);
      });
    return () => {
      live = false;
    };
  }, [active]);
  useEffect(() => {
    if (!recoveryKey || !detail) return;
    try {
      writeDrafts(recoveryKey, drafts);
      setRecoveryError("");
    } catch (error) {
      setRecoveryError(
        t("manuscripts.workspace.recovery.backupFailed", {
          message: message(error),
        }),
      );
    }
  }, [recoveryKey, drafts, detail?.project.id]);
  useEffect(() => {
    if (!dirty) return;
    const protect = (event: BeforeUnloadEvent) => {
      event.preventDefault();
    };
    window.addEventListener("beforeunload", protect);
    return () => window.removeEventListener("beforeunload", protect);
  }, [dirty]);
  useEffect(() => {
    if (!detail || !active) return;
    const id = detail.project.id;
    const timer = setInterval(() => {
      if (!mutation.current.busy) void refresh(id).catch(() => {});
    }, 10000);
    return () => clearInterval(timer);
  }, [detail?.project.id, active, refresh]);
  useEffect(() => {
    if (panel !== "history" || !detail) return;
    let live = true;
    gatewayRequest(
      "GET",
      `${projectPath(detail.project.id)}/history?path=${encodeURIComponent(path)}`,
    )
      .then((result) => {
        if (live) setHistory(result);
      })
      .catch((error) => {
        if (live) setError(message(error));
      });
    return () => {
      live = false;
    };
  }, [panel, path, detail?.project.id, file?.revision]);
  useEffect(() => {
    if (panel !== "library") return;
    let live = true;
    const timer = setTimeout(() => {
      gatewayRequest(
        "GET",
        `/bib/articles?query=${encodeURIComponent(query)}&limit=50&offset=0`,
      )
        .then((result) => {
          if (live) setArticles(result.articles ?? []);
        })
        .catch((error) => {
          if (live) setError(message(error));
        });
    }, 200);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [panel, query]);
  const open = async (project: Project) => {
    if (externalSidebar) onOpenSidebar?.();
    const next: Detail = await gatewayRequest("GET", projectPath(project.id));
    let restored: Record<string, Draft> = {};
    let key = "";
    try {
      const gateway = await probeGateway();
      key = draftStorageKey(
        gateway.addr,
        next.project.id,
        next.project.created_at,
      );
      restored = readDrafts(key);
    } catch (error) {
      key = ""; // Preserve unreadable recovery data for manual recovery.
      setRecoveryError(
        t("manuscripts.workspace.recovery.readFailed", {
          message: message(error),
        }),
      );
    }
    for (const draft of Object.values(restored)) {
      if (!next.files.some((file) => file.path === draft.base.path))
        next.files.push(draft.base);
    }
    setRecoveryKey(key);
    setRecovered(Object.keys(restored).length > 0);
    setDetail(next);
    setPath(Object.keys(restored)[0] ?? next.project.main_file);
    setDrafts(restored);
    setConflict(null);
    setPdf(null);
    setBuild(null);
    setPanel(null);
    try {
      const latest: Build = await gatewayRequest(
        "GET",
        `${projectPath(project.id)}/builds/latest`,
      );
      setBuild(latest);
      if (latest.status === "succeeded")
        setPdf(await fetchManuscriptArtifact(project.id, latest.id, "pdf"));
    } catch {
      /* A project may have no build yet. */
    }
  };
  const saveFile = async (target: string): Promise<boolean> => {
    const snapshot = state.current;
    const pending = getDraft(snapshot.drafts, target);
    if (
      !snapshot.detail ||
      !pending ||
      pending.content === pending.base.content
    )
      return true;
    const id = snapshot.detail.project.id;
    try {
      const saved: SourceFile = await gatewayRequest(
        "PUT",
        `${projectPath(id)}/files`,
        {
          path: target,
          content: pending.content,
          expected_revision: pending.base.revision,
        },
      );
      if (state.current.detail?.project.id !== id) return false;
      setDetail((current) =>
        current
          ? {
              ...current,
              files: current.files.map((file) =>
                file.path === target ? saved : file,
              ),
            }
          : current,
      );
      setDrafts((current) => {
        const latest = getDraft(current, target);
        if (!latest) return current;
        const next = { ...current };
        if (latest.content === saved.content) delete next[target];
        else next[target] = { content: latest.content, base: saved };
        return next;
      });
      return true;
    } catch (error) {
      if (
        (error instanceof GatewayHttpError && error.status === 409) ||
        message(error).includes("409") ||
        message(error).toLowerCase().includes("conflict")
      ) {
        const remote: Detail = await gatewayRequest("GET", projectPath(id));
        setConflict(
          remote.files.find((file) => file.path === target) ?? {
            ...pending.base,
            content: "",
            revision: 0,
          },
        );
        setError(t("manuscripts.workspace.recovery.conflictError"));
      } else setError(message(error));
      return false;
    }
  };
  const save = () =>
    void run(t("manuscripts.workspace.busy.saving"), async () => {
      await saveFile(path);
    });
  const compile = () =>
    void run(t("manuscripts.workspace.busy.compiling"), async () => {
      for (const target of Object.keys(state.current.drafts)) {
        if (!(await saveFile(target))) return;
      }
      const id = state.current.detail!.project.id;
      const result: Build = await gatewayRequest(
        "POST",
        `${projectPath(id)}/compile`,
        {},
      );
      setBuild(result);
      if (result.status === "succeeded")
        setPdf(await fetchManuscriptArtifact(id, result.id, "pdf"));
      else openManuscriptLog();
    });
  const update = (content: string) => {
    if (!file) return;
    setDrafts((current) => {
      const base = getDraft(current, path)?.base ?? file;
      const next = { ...current };
      if (content === base.content) delete next[path];
      else next[path] = { content, base };
      return next;
    });
  };
  if (!detail)
    return (
      <div className="manuscripts-home">
        <form
          className="manuscript-create"
          onSubmit={(event) => {
            event.preventDefault();
            if (title.trim())
              void run(t("manuscripts.workspace.busy.creating"), async () => {
                const project: Project = await gatewayRequest(
                  "POST",
                  "/bib/manuscripts",
                  { title: title.trim(), article_ids: initialArticleIds },
                );
                if (initialArticleIds.length) {
                  const created: Detail = await gatewayRequest(
                    "GET",
                    projectPath(project.id),
                  );
                  await gatewayRequest(
                    "POST",
                    `${projectPath(project.id)}/references`,
                    {
                      article_ids: initialArticleIds,
                      expected_revision:
                        created.files.find(
                          (file) => file.path === "references.bib",
                        )?.revision ?? 0,
                    },
                  );
                }
                setProjects((current) => [project, ...current]);
                await open(project);
              });
          }}
        >
          <Input
            aria-label={t("manuscripts.workspace.home.titleAria")}
            placeholder={t("manuscripts.workspace.home.titlePlaceholder")}
            value={title}
            onChange={(event) => setTitle(event.target.value)}
          />
          <Button type="submit" disabled={!title.trim() || !!busy}>
            <Plus />
            {t("manuscripts.workspace.home.createButton")}
          </Button>
        </form>
        {error && (
          <p role="alert" className="manuscript-error">
            {error}
          </p>
        )}
        <div className="manuscript-list-heading">
          <span>{t("manuscripts.workspace.home.myManuscripts")}</span>
          <span>{t("manuscripts.workspace.home.projectCount", { n: projects.length })}</span>
        </div>
        {loading ? (
          <p className="manuscript-muted">{t("manuscripts.workspace.home.loading")}</p>
        ) : projects.length ? (
          <div className="manuscript-projects">
            {projects.map((project) => (
              <button
                key={project.id}
                className="manuscript-project"
                onClick={() => void run(t("manuscripts.workspace.busy.opening"), () => open(project))}
                disabled={!!busy}
              >
                <FileText size={24} strokeWidth={1.4} />
                <div>
                  <strong>{project.title}</strong>
                  <small>
                    {project.main_file} ·{" "}
                    {t("manuscripts.workspace.home.referenceCount", {
                      n: project.article_ids.length,
                    })}
                  </small>
                </div>
                <ChevronRight size={16} />
              </button>
            ))}
          </div>
        ) : (
          <div className="manuscript-home-empty">
            <Code2 size={32} strokeWidth={1} />
            <h2>{t("manuscripts.workspace.home.emptyTitle")}</h2>
            <p>{t("manuscripts.workspace.home.emptyBody")}</p>
          </div>
        )}
      </div>
    );
  const pending = detail.proposals.filter(
    (proposal) => proposal.status === "pending",
  );
  const outline = [
    ...value.matchAll(
      /\\(part|chapter|section|subsection|subsubsection)\*?\{([^}]+)\}/g,
    ),
  ].map((match) => ({
    title: match[2],
    depth: match[1].startsWith("sub") ? 1 : 0,
    line: value.slice(0, match.index).split("\n").length,
  }));
  const toggle = (next: Panel) => setPanel(panel === next ? null : next);
  return (
    <div className="manuscript-workspace">
      {recoveryError && (
        <div role="alert" className="manuscript-error">
          {recoveryError}
        </div>
      )}
      {recovered && dirty && (
        <div className="manuscript-recovered" role="status">
          {t("manuscripts.workspace.recovery.restoredBanner")}
        </div>
      )}
      {error && (
        <div role="alert" className="manuscript-error">
          {error}
          <button aria-label={t("manuscripts.workspace.editorPane.dismissErrorAria")} onClick={() => setError("")}>
            <X size={14} />
          </button>
        </div>
      )}
      {conflict && (
        <div className="manuscript-conflict">
          <strong>{t("manuscripts.workspace.recovery.conflictTitle", { path: conflict.path })}</strong>
          <p>{t("manuscripts.workspace.recovery.conflictBody")}</p>
          <ProposalDiff
            original={conflict.content}
            proposed={getDraft(drafts, conflict.path)?.content ?? ""}
          />
          <Button
            size="sm"
            onClick={() => {
              setDrafts((current) => ({
                ...current,
                [conflict.path]: {
                  content:
                    getDraft(current, conflict.path)?.content ??
                    conflict.content,
                  base: conflict,
                },
              }));
              setDetail((current) =>
                current
                  ? {
                      ...current,
                      files: current.files.map((file) =>
                        file.path === conflict.path ? conflict : file,
                      ),
                    }
                  : current,
              );
              setConflict(null);
              setError("");
            }}
          >
            {t("manuscripts.workspace.recovery.conflictResolve")}
          </Button>
        </div>
      )}
      <div className="manuscript-main">
        {(!externalSidebar || sidebarSlot) && (externalSidebar ? createPortal(
          <aside className="manuscript-files">
          <div className="manuscript-pane-heading">
            <span>{t("manuscripts.workspace.files.heading")}</span>
            <div className="manuscript-inline">
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t("manuscripts.workspace.files.uploadAria")}
                title={t("manuscripts.workspace.files.uploadTitle")}
                disabled={!!busy}
                onClick={() => uploadInput.current?.click()}
              >
                <Upload />
              </Button>
              <input
                hidden
                ref={uploadInput}
                type="file"
                accept="image/png,image/jpeg,application/pdf"
                onChange={(event) => {
                  const upload = event.target.files?.[0];
                  event.target.value = "";
                  if (!upload) return;
                  void run(t("manuscripts.workspace.busy.uploadingAsset"), async () => {
                    if (upload.size > 16 * 1024 * 1024)
                      throw new Error(t("manuscripts.workspace.files.assetTooLarge"));
                    const bytes = new Uint8Array(await upload.arrayBuffer());
                    let binary = "";
                    for (let offset = 0; offset < bytes.length; offset += 8192)
                      binary += String.fromCharCode(
                        ...bytes.subarray(offset, offset + 8192),
                      );
                    const assetPath = `figures/${upload.name}`;
                    const existing = detail.assets?.find(
                      (asset) => asset.path === assetPath,
                    );
                    if (existing)
                      throw new Error(t("manuscripts.workspace.files.assetExists"));
                    await gatewayRequest(
                      "PUT",
                      `${projectPath(detail.project.id)}/assets`,
                      {
                        path: assetPath,
                        data_base64: btoa(binary),
                        expected_revision: 0,
                      },
                    );
                    await refresh(detail.project.id);
                  });
                }}
              />
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t("manuscripts.workspace.files.newFileAria")}
                title={t("manuscripts.workspace.files.newFileTitle")}
                onClick={() => setShowNewFile(!showNewFile)}
              >
                <FilePlus2 />
              </Button>
            </div>
          </div>
          {showNewFile && (
            <form
              className="manuscript-new-file"
              onSubmit={(event) => {
                event.preventDefault();
                if (newFile.trim())
                  void run(t("manuscripts.workspace.busy.creatingFile"), async () => {
                    const created: SourceFile = await gatewayRequest(
                      "PUT",
                      `${projectPath(detail.project.id)}/files`,
                      {
                        path: newFile.trim(),
                        content: "",
                        expected_revision: 0,
                      },
                    );
                    setDetail((current) =>
                      current
                        ? { ...current, files: [...current.files, created] }
                        : current,
                    );
                    setPath(created.path);
                    setNewFile("");
                    setShowNewFile(false);
                  });
              }}
            >
              <Input
                aria-label={t("manuscripts.workspace.files.newFileAria")}
                value={newFile}
                onChange={(event) => setNewFile(event.target.value)}
                placeholder="sections/methods.tex"
              />
              <Button type="submit" size="xs" disabled={!!busy}>
                {t("manuscripts.workspace.files.createButton")}
              </Button>
            </form>
          )}
          <nav aria-label={t("manuscripts.workspace.files.filesNavAria")}>
            {detail.files.map((source) => (
              <button
                key={source.path}
                className={path === source.path ? "is-selected" : ""}
                onClick={() => {
                  setPath(source.path);
                  setSelection({ startLine: 1, endLine: 1, text: "" });
                }}
              >
                <FileText size={14} />
                <span>{source.path}</span>
                {dirtyPath(source.path) && (
                  <span className="manuscript-dirty-mark">●</span>
                )}
              </button>
            ))}
          </nav>
          {detail.assets?.map((asset) => (
            <div className="manuscript-asset" key={asset.path}>
              <button
                title={t("manuscripts.workspace.files.insertAssetTitle")}
                onClick={() =>
                  setInsertion({
                    text: `\\includegraphics{${asset.path}}`,
                    id: Date.now(),
                  })
                }
              >
                <Image size={14} />
                <span>{asset.path}</span>
              </button>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label={t("manuscripts.workspace.files.deleteAssetAria", { path: asset.path })}
                disabled={!!busy}
                onClick={() =>
                  void run(t("manuscripts.workspace.busy.deletingAsset"), async () => {
                    await gatewayRequest(
                      "DELETE",
                      `${projectPath(detail.project.id)}/assets`,
                      { path: asset.path, expected_revision: asset.revision },
                    );
                    await refresh(detail.project.id);
                  })
                }
              >
                <Trash2 />
              </Button>
            </div>
          ))}
          <div className="manuscript-outline">
            <div className="manuscript-pane-heading">
              <span>
                <ListTree size={14} />
                {t("manuscripts.workspace.files.outline")}
              </span>
            </div>
            {outline.length ? (
              outline.map((item, index) => (
                <button
                  key={index}
                  style={{ paddingLeft: 14 + item.depth * 12 }}
                  onClick={() =>
                    setNavigation({ line: item.line, id: Date.now() })
                  }
                >
                  {item.title}
                </button>
              ))
            ) : (
              <p>{t("manuscripts.workspace.files.outlineEmpty")}</p>
            )}
          </div>
          <button
            className="manuscript-library-button"
            onClick={() => toggle("library")}
          >
            <BookOpen size={15} />
            {t("manuscripts.workspace.files.libraryButton")}<span>{detail.project.article_ids.length}</span>
          </button>
        </aside>, sidebarSlot!
        ) : <aside className="manuscript-files">
          <div className="manuscript-pane-heading">
            <span>{t("manuscripts.workspace.files.heading")}</span>
            <div className="manuscript-inline">
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t("manuscripts.workspace.files.uploadAria")}
                title={t("manuscripts.workspace.files.uploadTitle")}
                disabled={!!busy}
                onClick={() => uploadInput.current?.click()}
              >
                <Upload />
              </Button>
              <input
                hidden
                ref={uploadInput}
                type="file"
                accept="image/png,image/jpeg,application/pdf"
                onChange={(event) => {
                  const upload = event.target.files?.[0];
                  event.target.value = "";
                  if (!upload) return;
                  void run(t("manuscripts.workspace.busy.uploadingAsset"), async () => {
                    if (upload.size > 16 * 1024 * 1024)
                      throw new Error(t("manuscripts.workspace.files.assetTooLarge"));
                    const bytes = new Uint8Array(await upload.arrayBuffer());
                    let binary = "";
                    for (let offset = 0; offset < bytes.length; offset += 8192)
                      binary += String.fromCharCode(
                        ...bytes.subarray(offset, offset + 8192),
                      );
                    const assetPath = `figures/${upload.name}`;
                    const existing = detail.assets?.find(
                      (asset) => asset.path === assetPath,
                    );
                    if (existing)
                      throw new Error(t("manuscripts.workspace.files.assetExists"));
                    await gatewayRequest(
                      "PUT",
                      `${projectPath(detail.project.id)}/assets`,
                      {
                        path: assetPath,
                        data_base64: btoa(binary),
                        expected_revision: 0,
                      },
                    );
                    await refresh(detail.project.id);
                  });
                }}
              />
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label={t("manuscripts.workspace.files.newFileAria")}
                title={t("manuscripts.workspace.files.newFileTitle")}
                onClick={() => setShowNewFile(!showNewFile)}
              >
                <FilePlus2 />
              </Button>
            </div>
          </div>
          {showNewFile && (
            <form
              className="manuscript-new-file"
              onSubmit={(event) => {
                event.preventDefault();
                if (newFile.trim())
                  void run(t("manuscripts.workspace.busy.creatingFile"), async () => {
                    const created: SourceFile = await gatewayRequest(
                      "PUT",
                      `${projectPath(detail.project.id)}/files`,
                      {
                        path: newFile.trim(),
                        content: "",
                        expected_revision: 0,
                      },
                    );
                    setDetail((current) =>
                      current
                        ? { ...current, files: [...current.files, created] }
                        : current,
                    );
                    setPath(created.path);
                    setNewFile("");
                    setShowNewFile(false);
                  });
              }}
            >
              <Input
                aria-label={t("manuscripts.workspace.files.newFileAria")}
                value={newFile}
                onChange={(event) => setNewFile(event.target.value)}
                placeholder="sections/methods.tex"
              />
              <Button type="submit" size="xs" disabled={!!busy}>
                {t("manuscripts.workspace.files.createButton")}
              </Button>
            </form>
          )}
          <nav aria-label={t("manuscripts.workspace.files.filesNavAria")}>
            {detail.files.map((source) => (
              <button
                key={source.path}
                className={path === source.path ? "is-selected" : ""}
                onClick={() => {
                  setPath(source.path);
                  setSelection({ startLine: 1, endLine: 1, text: "" });
                }}
              >
                <FileText size={14} />
                <span>{source.path}</span>
                {dirtyPath(source.path) && (
                  <span className="manuscript-dirty-mark">●</span>
                )}
              </button>
            ))}
          </nav>
          {detail.assets?.map((asset) => (
            <div className="manuscript-asset" key={asset.path}>
              <button
                title={t("manuscripts.workspace.files.insertAssetTitle")}
                onClick={() =>
                  setInsertion({
                    text: `\\includegraphics{${asset.path}}`,
                    id: Date.now(),
                  })
                }
              >
                <Image size={14} />
                <span>{asset.path}</span>
              </button>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label={t("manuscripts.workspace.files.deleteAssetAria", { path: asset.path })}
                disabled={!!busy}
                onClick={() =>
                  void run(t("manuscripts.workspace.busy.deletingAsset"), async () => {
                    await gatewayRequest(
                      "DELETE",
                      `${projectPath(detail.project.id)}/assets`,
                      { path: asset.path, expected_revision: asset.revision },
                    );
                    await refresh(detail.project.id);
                  })
                }
              >
                <Trash2 />
              </Button>
            </div>
          ))}
          <div className="manuscript-outline">
            <div className="manuscript-pane-heading">
              <span>
                <ListTree size={14} />
                {t("manuscripts.workspace.files.outline")}
              </span>
            </div>
            {outline.length ? (
              outline.map((item, index) => (
                <button
                  key={index}
                  style={{ paddingLeft: 14 + item.depth * 12 }}
                  onClick={() =>
                    setNavigation({ line: item.line, id: Date.now() })
                  }
                >
                  {item.title}
                </button>
              ))
            ) : (
              <p>{t("manuscripts.workspace.files.outlineEmpty")}</p>
            )}
          </div>
          <button
            className="manuscript-library-button"
            onClick={() => toggle("library")}
          >
            <BookOpen size={15} />
            {t("manuscripts.workspace.files.libraryButton")}<span>{detail.project.article_ids.length}</span>
          </button>
        </aside>)}
        <section className="manuscript-source">
          <div className="manuscript-pane-heading">
            <div className="manuscript-inline min-w-0">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("manuscripts.workspace.editorPane.backAria")}
          disabled={!!busy}
          onClick={() => {
            if (dirty) {
              setError(t("manuscripts.workspace.recovery.backDirtyError"));
              return;
            }
            setDetail(null);
          }}
        >
          <ArrowLeft />
        </Button>
              <span className="truncate" title={detail.project.title}>{detail.project.title} · {path}</span>
              <span className={`manuscript-status-dot ${dirty ? "is-dirty" : ""}`} title={busy || (dirty ? t("manuscripts.workspace.editorPane.dirtyTitle") : t("manuscripts.workspace.editorPane.savedTitle"))} aria-label={busy || (dirty ? t("manuscripts.workspace.editorPane.dirtyTitle") : t("manuscripts.workspace.editorPane.savedTitle"))} />
            </div>
            <div className="manuscript-inline">
              <span>LaTeX · r{draft?.base.revision ?? file?.revision ?? 0}</span>
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label={t("manuscripts.workspace.editorPane.menuAria")} />}><Ellipsis /></DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-40">
          <DropdownMenuItem
            aria-label={t("manuscripts.workspace.editorPane.downloadAria")}
            title={t("manuscripts.workspace.editorPane.downloadTitle")}
            disabled={!!busy}
            onClick={() =>
              void run(t("manuscripts.workspace.busy.exporting"), async () => {
                for (const target of Object.keys(state.current.drafts)) {
                  if (!(await saveFile(target))) return;
                }
                const exported: { filename: string; data_base64: string } =
                  await gatewayRequest(
                    "GET",
                    `${projectPath(detail.project.id)}/export`,
                  );
                await saveBinaryFile(
                  exported.filename,
                  Uint8Array.from(atob(exported.data_base64), (character) =>
                    character.charCodeAt(0),
                  ),
                  "application/zip",
                );
              })
            }
          >
            <Download /> {t("manuscripts.workspace.editorPane.downloadItem")}
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={save}
            disabled={!dirtyPath(path) || !!busy}
          >
            <Save />
            {t("manuscripts.workspace.editorPane.save")}
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={!!busy}
            onClick={compile}
          >
            {busy === t("manuscripts.workspace.busy.compiling") ? (
              <Loader2 className="animate-spin" />
            ) : (
              <Play />
            )}
            {t("manuscripts.workspace.editorPane.compile")}
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={() => toggle("review")}
          >
            <Sparkles />
            {t("manuscripts.workspace.editorPane.review")}
            {pending.length > 0 && (
              <span className="manuscript-count">{pending.length}</span>
            )}
          </DropdownMenuItem>
          <DropdownMenuItem
            aria-label={t("manuscripts.workspace.editorPane.commentsAria")}
            title={t("manuscripts.workspace.editorPane.commentsTitle")}
            onClick={() => toggle("comments")}
          >
            <MessageSquare /> {t("manuscripts.workspace.editorPane.commentsItem")}
          </DropdownMenuItem>
          <DropdownMenuItem
            aria-label={t("manuscripts.workspace.editorPane.historyAria")}
            title={t("manuscripts.workspace.editorPane.historyTitle")}
            onClick={() => toggle("history")}
          >
            <History /> {t("manuscripts.workspace.editorPane.historyItem")}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={openManuscriptLog}><FileText /> {t("manuscripts.workspace.editorPane.logItem")}</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
            </div>
          </div>
          {file ? (
            <SourceEditor
              key={detail.project.id}
              path={path}
              value={value}
              onChange={update}
              onSave={save}
              onSelection={setSelection}
              navigation={navigation}
              insertion={insertion}
            />
          ) : (
            <p className="manuscript-muted">{t("manuscripts.workspace.editorPane.selectFile")}</p>
          )}

        </section>
        <div className="manuscript-output">
          <PdfPreview bytes={pdf} title={detail.project.title} />

        </div>
        {panel && (
          <aside className="manuscript-activity">
            <div className="manuscript-pane-heading">
              <span>
                {
                  {
                    review: t("manuscripts.workspace.panels.review"),
                    comments: t("manuscripts.workspace.panels.comments"),
                    history: t("manuscripts.workspace.panels.history"),
                    library: t("manuscripts.workspace.panels.library"),
                  }[panel]
                }
              </span>
              <div className="manuscript-inline">
                {panel === "review" && (
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label={t("manuscripts.workspace.panels.refreshAria")}
                    onClick={() =>
                      void run(t("manuscripts.workspace.busy.refreshing"), () => refresh(detail.project.id))
                    }
                  >
                    <RefreshCw />
                  </Button>
                )}
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label={t("manuscripts.workspace.panels.closeAria")}
                  onClick={() => setPanel(null)}
                >
                  <X />
                </Button>
              </div>
            </div>
            <div className="manuscript-activity-scroll">
              {panel === "review" && (
                <>
                  <p className="manuscript-panel-intro">
                    {t("manuscripts.workspace.panels.reviewIntro")}
                  </p>
                  {pending.length === 0 && (
                    <div className="manuscript-empty">
                      <Check size={24} />
                      <p>{t("manuscripts.workspace.panels.reviewEmpty")}</p>
                      <small>
                        {t("manuscripts.workspace.panels.reviewEmptyHint")}
                      </small>
                    </div>
                  )}
                  {detail.proposals.map((proposal) => (
                    <article
                      className="manuscript-review-card"
                      key={proposal.id}
                    >
                      <header>
                        <strong>{proposal.summary || t("manuscripts.workspace.panels.proposalFallback")}</strong>
                        <small>
                          {proposal.author} · {proposal.path} · r
                          {proposal.base_revision}
                        </small>
                      </header>
                      <ProposalDiff
                        original={proposal.original}
                        proposed={proposal.proposed}
                      />
                      {proposal.status === "pending" ? (
                        <div className="manuscript-inline">
                          <Button
                            size="xs"
                            disabled={!!busy || dirtyPath(proposal.path)}
                            title={
                              dirtyPath(proposal.path)
                                ? t("manuscripts.workspace.panels.saveFirstTitle")
                                : ""
                            }
                            onClick={() =>
                              void run(t("manuscripts.workspace.busy.accepting"), async () => {
                                await gatewayRequest(
                                  "POST",
                                  `${projectPath(detail.project.id)}/proposals/${encodeURIComponent(proposal.id)}/resolve`,
                                  { action: "accept" },
                                );
                                await refresh(detail.project.id);
                              })
                            }
                          >
                            <Check />
                            {t("manuscripts.workspace.panels.accept")}
                          </Button>
                          <Button
                            size="xs"
                            variant="outline"
                            disabled={!!busy}
                            onClick={() =>
                              void run(t("manuscripts.workspace.busy.rejecting"), async () => {
                                await gatewayRequest(
                                  "POST",
                                  `${projectPath(detail.project.id)}/proposals/${encodeURIComponent(proposal.id)}/resolve`,
                                  { action: "reject" },
                                );
                                await refresh(detail.project.id);
                              })
                            }
                          >
                            {t("manuscripts.workspace.panels.reject")}
                          </Button>
                        </div>
                      ) : (
                        <small className="manuscript-muted">
                          {proposal.status === "accepted" ? t("manuscripts.workspace.panels.accepted") : t("manuscripts.workspace.panels.rejected")}
                        </small>
                      )}
                      {dirtyPath(proposal.path) &&
                        proposal.status === "pending" && (
                          <small>{t("manuscripts.workspace.panels.dirtyProposalNote")}</small>
                        )}
                    </article>
                  ))}
                </>
              )}
              {panel === "comments" && (
                <>
                  <form
                    className="manuscript-comment-form"
                    onSubmit={(event) => {
                      event.preventDefault();
                      if (comment.trim())
                        void run(t("manuscripts.workspace.busy.addingComment"), async () => {
                          await gatewayRequest(
                            "POST",
                            `${projectPath(detail.project.id)}/comments`,
                            {
                              path,
                              start_line: selection.startLine,
                              end_line: selection.endLine,
                              expected_revision: file?.revision,
                              body: comment.trim(),
                            },
                          );
                          setComment("");
                          await refresh(detail.project.id);
                        });
                    }}
                  >
                    <small>
                      {path} · {t("manuscripts.workspace.panels.commentLines", { start: selection.startLine, end: selection.endLine })}
                    </small>
                    <textarea
                      aria-label={t("manuscripts.workspace.panels.commentAria")}
                      placeholder={t("manuscripts.workspace.panels.commentPlaceholder")}
                      value={comment}
                      onChange={(event) => setComment(event.target.value)}
                    />
                    <Button
                      size="xs"
                      type="submit"
                      disabled={!comment.trim() || !!busy || dirtyPath(path)}
                    >
                      {dirtyPath(path) ? t("manuscripts.workspace.panels.addCommentAfterSave") : t("manuscripts.workspace.panels.addComment")}
                    </Button>
                  </form>
                  {detail.comments.length === 0 && (
                    <p className="manuscript-muted">
                      {t("manuscripts.workspace.panels.commentsEmpty")}
                    </p>
                  )}
                  {detail.comments.map((item) => (
                    <article className="manuscript-comment" key={item.id}>
                      <button
                        onClick={() => {
                          setPath(item.path);
                          setNavigation({
                            line: item.start_line,
                            id: Date.now(),
                          });
                        }}
                      >
                        {item.path} · L{item.start_line}–{item.end_line}
                      </button>
                      <small>
                        {item.author}
                        {item.revision &&
                        detail.files.find((file) => file.path === item.path)
                          ?.revision !== item.revision
                          ? t("manuscripts.workspace.panels.commentStale")
                          : ""}
                      </small>
                      <p>{item.body}</p>
                      <Button
                        variant="ghost"
                        size="xs"
                        disabled={!!busy}
                        onClick={() =>
                          void run(t("manuscripts.workspace.busy.updatingComment"), async () => {
                            await gatewayRequest(
                              "PATCH",
                              `${projectPath(detail.project.id)}/comments/${encodeURIComponent(item.id)}`,
                              { resolved: !item.resolved },
                            );
                            await refresh(detail.project.id);
                          })
                        }
                      >
                        {item.resolved ? t("manuscripts.workspace.panels.reopen") : t("manuscripts.workspace.panels.resolve")}
                      </Button>
                    </article>
                  ))}
                </>
              )}
              {panel === "history" && (
                <>
                  <p className="manuscript-panel-intro">
                    {t("manuscripts.workspace.panels.historyIntro", { path })}
                  </p>
                  {history.map((revision) => (
                    <article className="manuscript-revision" key={revision.id}>
                      <div>
                        <strong>{t("manuscripts.workspace.panels.revisionLabel", { revision: revision.revision })}</strong>
                        <small>
                          {revision.actor} · {revision.created_at}
                        </small>
                      </div>
                      <p>{revision.message}</p>
                      <details>
                        <summary>{t("manuscripts.workspace.panels.viewSource")}</summary>
                        <pre>{revision.content}</pre>
                      </details>
                      <Button
                        size="xs"
                        variant="outline"
                        disabled={
                          !!busy ||
                          dirtyPath(path) ||
                          revision.revision === file?.revision
                        }
                        onClick={() =>
                          void run(t("manuscripts.workspace.busy.restoring"), async () => {
                            await gatewayRequest(
                              "POST",
                              `${projectPath(detail.project.id)}/restore`,
                              {
                                path,
                                revision: revision.revision,
                                expected_revision: file?.revision,
                              },
                            );
                            await refresh(detail.project.id);
                          })
                        }
                      >
                        {t("manuscripts.workspace.panels.restore")}
                      </Button>
                    </article>
                  ))}
                </>
              )}
              {panel === "library" && (
                <>
                  <Input
                    aria-label={t("manuscripts.workspace.panels.librarySearchAria")}
                    placeholder={t("manuscripts.workspace.panels.librarySearchPlaceholder")}
                    value={query}
                    onChange={(event) => setQuery(event.target.value)}
                  />
                  <p className="manuscript-panel-intro">
                    {t("manuscripts.workspace.panels.libraryIntro")}
                  </p>
                  {articles.map((article) => (
                    <button
                      className="manuscript-citation"
                      key={article.id}
                      disabled={!!busy || dirtyPath("references.bib")}
                      onClick={() =>
                        void run(t("manuscripts.workspace.busy.syncingCitation"), async () => {
                          const ids = [
                            ...new Set([
                              ...detail.project.article_ids,
                              article.id,
                            ]),
                          ];
                          const result: {
                            file: SourceFile;
                            keys: Record<string, string>;
                          } = await gatewayRequest(
                            "POST",
                            `${projectPath(detail.project.id)}/references`,
                            {
                              article_ids: ids,
                              expected_revision:
                                detail.files.find(
                                  (file) => file.path === "references.bib",
                                )?.revision ?? 0,
                            },
                          );
                          await refresh(detail.project.id);
                          const key = result.keys[article.id];
                          if (!key) throw new Error(t("manuscripts.workspace.panels.citationKeyMissing"));
                          if (path === "references.bib") {
                            setPath(detail.project.main_file);
                            setTimeout(
                              () =>
                                setInsertion({
                                  text: `\\cite{${key}}`,
                                  id: Date.now(),
                                }),
                              0,
                            );
                          } else
                            setInsertion({
                              text: `\\cite{${key}}`,
                              id: Date.now(),
                            });
                        })
                      }
                    >
                      <strong>{article.title}</strong>
                      <small>
                        {article.authors
                          ?.map((author) => author.last_name ?? author.name)
                          .join(", ")}{" "}
                        · {article.year ?? "—"}
                      </small>
                      <span>
                        <Plus size={12} />
                        {t("manuscripts.workspace.panels.insertCitation")}
                      </span>
                    </button>
                  ))}
                </>
              )}

            </div>
          </aside>
        )}
      </div>
    </div>
  );
}
