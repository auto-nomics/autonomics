import { createPortal } from "react-dom";
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
        `本地草稿备份失败：${message(error)}。当前编辑仍保留，请保存到服务器后再关闭。`,
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
      setRecoveryError(`无法读取本地草稿：${message(error)}`);
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
        setError("文件已被其他协作者更新。本地编辑已保留，请对照版本后合并。");
      } else setError(message(error));
      return false;
    }
  };
  const save = () =>
    void run("保存中", async () => {
      await saveFile(path);
    });
  const compile = () =>
    void run("编译中", async () => {
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
              void run("创建中", async () => {
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
            aria-label="Manuscript title"
            placeholder="新手稿标题"
            value={title}
            onChange={(event) => setTitle(event.target.value)}
          />
          <Button type="submit" disabled={!title.trim() || !!busy}>
            <Plus />
            新建 LaTeX 手稿
          </Button>
        </form>
        {error && (
          <p role="alert" className="manuscript-error">
            {error}
          </p>
        )}
        <div className="manuscript-list-heading">
          <span>我的手稿</span>
          <span>{projects.length} 个项目</span>
        </div>
        {loading ? (
          <p className="manuscript-muted">正在加载手稿…</p>
        ) : projects.length ? (
          <div className="manuscript-projects">
            {projects.map((project) => (
              <button
                key={project.id}
                className="manuscript-project"
                onClick={() => void run("打开中", () => open(project))}
                disabled={!!busy}
              >
                <FileText size={24} strokeWidth={1.4} />
                <div>
                  <strong>{project.title}</strong>
                  <small>
                    {project.main_file} · {project.article_ids.length}{" "}
                    篇参考文献
                  </small>
                </div>
                <ChevronRight size={16} />
              </button>
            ))}
          </div>
        ) : (
          <div className="manuscript-home-empty">
            <Code2 size={32} strokeWidth={1} />
            <h2>第一篇手稿，从这里开始</h2>
            <p>每个项目包含独立的源文件、编译记录与可审阅的修改建议。</p>
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
          已恢复此项目的本地草稿。保存时将校验原版本，避免覆盖其他协作者的修改。
        </div>
      )}
      {error && (
        <div role="alert" className="manuscript-error">
          {error}
          <button aria-label="Dismiss error" onClick={() => setError("")}>
            <X size={14} />
          </button>
        </div>
      )}
      {conflict && (
        <div className="manuscript-conflict">
          <strong>版本冲突 · {conflict.path}</strong>
          <p>
            左侧编辑器保留你的内容；下方对照显示服务器版本与本地草稿。合并后重新保存。
          </p>
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
            已对照，保留草稿并更新基线
          </Button>
        </div>
      )}
      <div className="manuscript-main">
        {(!externalSidebar || sidebarSlot) && (externalSidebar ? createPortal(
          <aside className="manuscript-files">
          <div className="manuscript-pane-heading">
            <span>项目文件</span>
            <div className="manuscript-inline">
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Upload figure"
                title="上传图片或 PDF"
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
                  void run("上传附件", async () => {
                    if (upload.size > 16 * 1024 * 1024)
                      throw new Error("附件不能超过 16 MiB");
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
                      throw new Error("此附件已存在，请先重命名上传文件。");
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
                aria-label="New source file"
                title="新建文件"
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
                  void run("创建文件", async () => {
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
                aria-label="New file path"
                value={newFile}
                onChange={(event) => setNewFile(event.target.value)}
                placeholder="sections/methods.tex"
              />
              <Button type="submit" size="xs" disabled={!!busy}>
                创建
              </Button>
            </form>
          )}
          <nav aria-label="Project files">
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
                title="插入图像路径"
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
                aria-label={`Delete ${asset.path}`}
                disabled={!!busy}
                onClick={() =>
                  void run("删除附件", async () => {
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
                文档大纲
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
              <p>章节标题将显示在这里。</p>
            )}
          </div>
          <button
            className="manuscript-library-button"
            onClick={() => toggle("library")}
          >
            <BookOpen size={15} />
            文献库与引用<span>{detail.project.article_ids.length}</span>
          </button>
        </aside>, sidebarSlot!
        ) : <aside className="manuscript-files">
          <div className="manuscript-pane-heading">
            <span>项目文件</span>
            <div className="manuscript-inline">
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Upload figure"
                title="上传图片或 PDF"
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
                  void run("上传附件", async () => {
                    if (upload.size > 16 * 1024 * 1024)
                      throw new Error("附件不能超过 16 MiB");
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
                      throw new Error("此附件已存在，请先重命名上传文件。");
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
                aria-label="New source file"
                title="新建文件"
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
                  void run("创建文件", async () => {
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
                aria-label="New file path"
                value={newFile}
                onChange={(event) => setNewFile(event.target.value)}
                placeholder="sections/methods.tex"
              />
              <Button type="submit" size="xs" disabled={!!busy}>
                创建
              </Button>
            </form>
          )}
          <nav aria-label="Project files">
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
                title="插入图像路径"
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
                aria-label={`Delete ${asset.path}`}
                disabled={!!busy}
                onClick={() =>
                  void run("删除附件", async () => {
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
                文档大纲
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
              <p>章节标题将显示在这里。</p>
            )}
          </div>
          <button
            className="manuscript-library-button"
            onClick={() => toggle("library")}
          >
            <BookOpen size={15} />
            文献库与引用<span>{detail.project.article_ids.length}</span>
          </button>
        </aside>)}
        <section className="manuscript-source">
          <div className="manuscript-pane-heading">
            <div className="manuscript-inline min-w-0">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Back to manuscripts"
          disabled={!!busy}
          onClick={() => {
            if (dirty) {
              setError("请先保存本地编辑，再返回项目列表。");
              return;
            }
            setDetail(null);
          }}
        >
          <ArrowLeft />
        </Button>
              <span className="truncate" title={detail.project.title}>{detail.project.title} · {path}</span>
              <span className={`manuscript-status-dot ${dirty ? "is-dirty" : ""}`} title={busy || (dirty ? "有未保存的修改" : "所有修改已保存")} aria-label={busy || (dirty ? "有未保存的修改" : "所有修改已保存")} />
            </div>
            <div className="manuscript-inline">
              <span>LaTeX · r{draft?.base.revision ?? file?.revision ?? 0}</span>
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label="论文操作" />}><Ellipsis /></DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-40">
          <DropdownMenuItem
            aria-label="Download source ZIP"
            title="下载源文件 ZIP"
            disabled={!!busy}
            onClick={() =>
              void run("导出源文件", async () => {
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
            <Download /> 下载源文件 ZIP
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={save}
            disabled={!dirtyPath(path) || !!busy}
          >
            <Save />
            保存
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={!!busy}
            onClick={compile}
          >
            {busy === "编译中" ? (
              <Loader2 className="animate-spin" />
            ) : (
              <Play />
            )}
            编译
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={() => toggle("review")}
          >
            <Sparkles />
            审阅
            {pending.length > 0 && (
              <span className="manuscript-count">{pending.length}</span>
            )}
          </DropdownMenuItem>
          <DropdownMenuItem
            aria-label="Comments"
            title="批注"
            onClick={() => toggle("comments")}
          >
            <MessageSquare /> 批注
          </DropdownMenuItem>
          <DropdownMenuItem
            aria-label="History"
            title="版本历史"
            onClick={() => toggle("history")}
          >
            <History /> 版本历史
          </DropdownMenuItem>
          <DropdownMenuItem onClick={openManuscriptLog}><FileText /> 编译日志</DropdownMenuItem>
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
            <p className="manuscript-muted">请选择源文件。</p>
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
                    review: "修改审阅",
                    comments: "批注",
                    history: "版本历史",
                    library: "插入引用",
                  }[panel]
                }
              </span>
              <div className="manuscript-inline">
                {panel === "review" && (
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label="Refresh proposals"
                    onClick={() =>
                      void run("刷新中", () => refresh(detail.project.id))
                    }
                  >
                    <RefreshCw />
                  </Button>
                )}
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label="Close panel"
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
                    智能体的修改先作为建议保留。检查差异后，由你决定是否写入源文件。
                  </p>
                  {pending.length === 0 && (
                    <div className="manuscript-empty">
                      <Check size={24} />
                      <p>暂无待审阅建议</p>
                      <small>
                        将源文件或选区加入会话，请智能体提交修改建议。
                      </small>
                    </div>
                  )}
                  {detail.proposals.map((proposal) => (
                    <article
                      className="manuscript-review-card"
                      key={proposal.id}
                    >
                      <header>
                        <strong>{proposal.summary || "源文件修改"}</strong>
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
                                ? "先保存此文件的本地修改"
                                : ""
                            }
                            onClick={() =>
                              void run("接受建议", async () => {
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
                            接受修改
                          </Button>
                          <Button
                            size="xs"
                            variant="outline"
                            disabled={!!busy}
                            onClick={() =>
                              void run("拒绝建议", async () => {
                                await gatewayRequest(
                                  "POST",
                                  `${projectPath(detail.project.id)}/proposals/${encodeURIComponent(proposal.id)}/resolve`,
                                  { action: "reject" },
                                );
                                await refresh(detail.project.id);
                              })
                            }
                          >
                            拒绝
                          </Button>
                        </div>
                      ) : (
                        <small className="manuscript-muted">
                          {proposal.status === "accepted" ? "已接受" : "已拒绝"}
                        </small>
                      )}
                      {dirtyPath(proposal.path) &&
                        proposal.status === "pending" && (
                          <small>此文件有本地编辑，请先保存再接受建议。</small>
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
                        void run("添加批注", async () => {
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
                      {path} · 第 {selection.startLine}–{selection.endLine} 行
                    </small>
                    <textarea
                      aria-label="Comment text"
                      placeholder="对选中的段落添加批注…"
                      value={comment}
                      onChange={(event) => setComment(event.target.value)}
                    />
                    <Button
                      size="xs"
                      type="submit"
                      disabled={!comment.trim() || !!busy || dirtyPath(path)}
                    >
                      {dirtyPath(path) ? "保存后添加批注" : "添加批注"}
                    </Button>
                  </form>
                  {detail.comments.length === 0 && (
                    <p className="manuscript-muted">
                      选择源文件中的段落，开始讨论。
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
                          ? " · 原文已更新，需核对位置"
                          : ""}
                      </small>
                      <p>{item.body}</p>
                      <Button
                        variant="ghost"
                        size="xs"
                        disabled={!!busy}
                        onClick={() =>
                          void run("更新批注", async () => {
                            await gatewayRequest(
                              "PATCH",
                              `${projectPath(detail.project.id)}/comments/${encodeURIComponent(item.id)}`,
                              { resolved: !item.resolved },
                            );
                            await refresh(detail.project.id);
                          })
                        }
                      >
                        {item.resolved ? "重新打开" : "标为已解决"}
                      </Button>
                    </article>
                  ))}
                </>
              )}
              {panel === "history" && (
                <>
                  <p className="manuscript-panel-intro">
                    {path} 的保存记录。恢复会创建一个新版本，保留完整历史。
                  </p>
                  {history.map((revision) => (
                    <article className="manuscript-revision" key={revision.id}>
                      <div>
                        <strong>版本 {revision.revision}</strong>
                        <small>
                          {revision.actor} · {revision.created_at}
                        </small>
                      </div>
                      <p>{revision.message}</p>
                      <details>
                        <summary>查看源文件</summary>
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
                          void run("恢复版本", async () => {
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
                        恢复此版本
                      </Button>
                    </article>
                  ))}
                </>
              )}
              {panel === "library" && (
                <>
                  <Input
                    aria-label="Search citation library"
                    placeholder="搜索文献库…"
                    value={query}
                    onChange={(event) => setQuery(event.target.value)}
                  />
                  <p className="manuscript-panel-intro">
                    插入引用会同步项目的 references.bib。使用 \bibliography
                    {"{references}"} 输出参考文献。
                  </p>
                  {articles.map((article) => (
                    <button
                      className="manuscript-citation"
                      key={article.id}
                      disabled={!!busy || dirtyPath("references.bib")}
                      onClick={() =>
                        void run("同步引用", async () => {
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
                          if (!key) throw new Error("未返回此文献的引用键");
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
                        插入引用
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
