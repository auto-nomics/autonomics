import {
  ChevronDown,
  ChevronRight,
  FileImage,
  History,
  Link2,
  Loader2,
  Upload,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  attachmentRawUrl,
  detachAttachment,
  formatTime,
  getLinks,
  getMentions,
  listAttachments,
  listVersions,
  restoreVersion,
  uploadAttachment,
  type AttachmentMeta,
  type LinkGraph,
  type VersionSummary,
  type WikiEntry,
} from "@/lib/wiki-api";

type SideTab = "links" | "attachments" | "versions";

const TABS: { value: SideTab; label: string }[] = [
  { value: "links", label: "Links" },
  { value: "attachments", label: "Files" },
  { value: "versions", label: "History" },
];

/**
 * Right dock panel (SiYuan 侧栏 analog): backlinks + mentions, image
 * attachments, version history — one tab per domain, per-entry state.
 */
export function SidePanel({ entry }: { entry: WikiEntry | null }) {
  const [tab, setTab] = useState<SideTab>("links");
  return (
    <div className="flex h-full min-h-0 flex-col rounded-md border">
      <div className="flex overflow-hidden rounded-t-md border-b text-xs">
        {TABS.map((item) => (
          <button
            key={item.value}
            type="button"
            className={`flex-1 px-2 py-1.5 transition-colors ${
              tab === item.value ? "bg-accent font-medium" : "hover:bg-accent"
            }`}
            onClick={() => setTab(item.value)}
          >
            {item.label}
          </button>
        ))}
      </div>
      <ScrollArea className="min-h-0 flex-1 p-2">
        {entry === null ? (
          <p className="p-4 text-xs text-muted-foreground">No entry selected.</p>
        ) : tab === "links" ? (
          <LinksTab entry={entry} />
        ) : tab === "attachments" ? (
          <AttachmentsTab entry={entry} />
        ) : (
          <VersionsTab entry={entry} />
        )}
      </ScrollArea>
    </div>
  );
}

/** 反链 + 提及(SiYuan 反链面板的两节:links / mentions)。 */
function LinksTab({ entry }: { entry: WikiEntry }) {
  const [graph, setGraph] = useState<LinkGraph | null>(null);
  const [mentions, setMentions] = useState<string[] | null>(null);

  useEffect(() => {
    setGraph(null);
    setMentions(null);
    void getLinks(entry.id)
      .then(setGraph)
      .catch((error: Error) => toast.error(error.message));
    void getMentions(entry.id)
      .then(setMentions)
      .catch(() => setMentions([]));
  }, [entry.id]);

  if (graph === null) return <Loader2 className="m-4 h-4 w-4 animate-spin" />;
  return (
    <div className="space-y-3 text-xs">
      <Section title={`Out-links (${graph.out_links.length})`}>
        {graph.out_links.map((id) => (
          <LinkRow key={id} id={id} />
        ))}
      </Section>
      <Section title={`In-links (${graph.in_links.length})`}>
        {graph.in_links.map((id) => (
          <LinkRow key={id} id={id} />
        ))}
      </Section>
      <Section title={`Unlinked mentions (${mentions?.length ?? 0})`}>
        {mentions === null ? (
          <Loader2 className="h-3 w-3 animate-spin" />
        ) : mentions.length === 0 ? (
          <p className="text-muted-foreground">None — add [[links]] in other entries.</p>
        ) : (
          mentions.map((id) => <LinkRow key={id} id={id} />)
        )}
      </Section>
    </div>
  );
}

function LinkRow({ id }: { id: string }) {
  return (
    <button
      type="button"
      className="flex w-full items-center gap-1.5 truncate rounded px-1.5 py-1 text-left hover:bg-accent"
      title={id}
      onClick={() => {
        // Route through the hash so the app shell can open it later;
        // for now open in the same panel is out of scope — show the id.
        window.dispatchEvent(new CustomEvent("wiki:open-entry", { detail: id }));
      }}
    >
      <Link2 className="h-3 w-3 shrink-0 text-muted-foreground" />
      <span className="truncate font-mono text-[11px]">{id}</span>
    </button>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(true);
  return (
    <div>
      <button
        type="button"
        className="flex w-full items-center gap-1 py-1 font-medium"
        onClick={() => setOpen((value) => !value)}
      >
        {open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
        {title}
      </button>
      {open ? <div className="space-y-0.5 pl-2">{children}</div> : null}
    </div>
  );
}

function AttachmentsTab({ entry }: { entry: WikiEntry }) {
  const [items, setItems] = useState<AttachmentMeta[] | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  async function reload() {
    setItems(await listAttachments(entry.id).catch(() => []));
  }

  useEffect(() => {
    setItems(null);
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entry.id]);

  return (
    <div className="space-y-2 text-xs">
      <input
        ref={fileInput}
        type="file"
        accept="image/*"
        className="hidden"
        onChange={(event) => {
          const file = event.target.files?.[0];
          event.target.value = "";
          if (!file) return;
          void (async () => {
            try {
              await uploadAttachment(entry.id, file);
              toast.success(`Attached ${file.name}`);
              await reload();
            } catch (error) {
              toast.error((error as Error).message);
            }
          })();
        }}
      />
      <Button size="sm" variant="outline" className="w-full" onClick={() => fileInput.current?.click()}>
        <Upload className="h-3.5 w-3.5" /> Upload image
      </Button>
      {items === null ? (
        <Loader2 className="h-4 w-4 animate-spin" />
      ) : items.length === 0 ? (
        <p className="text-muted-foreground">No attachments.</p>
      ) : (
        <ul className="grid grid-cols-2 gap-2">
          {items.map((item) => {
            const id = item.attachment_id ?? item.id ?? "";
            return (
              <li key={id} className="rounded-md border p-1.5">
                <a href={attachmentRawUrl(entry.id, id)} target="_blank" rel="noreferrer">
                  <img
                    src={attachmentRawUrl(entry.id, id)}
                    alt={item.filename}
                    className="h-20 w-full rounded object-contain"
                    loading="lazy"
                  />
                </a>
                <div className="mt-1 flex items-center justify-between gap-1">
                  <span className="truncate text-[11px]">{item.filename}</span>
                  <button
                    type="button"
                    className="rounded p-0.5 hover:bg-accent"
                    title="Detach"
                    onClick={() =>
                      void (async () => {
                        await detachAttachment(entry.id, id);
                        await reload();
                      })()
                    }
                  >
                    ✕
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function VersionsTab({ entry }: { entry: WikiEntry }) {
  const [versions, setVersions] = useState<VersionSummary[] | null>(null);

  useEffect(() => {
    setVersions(null);
    void listVersions(entry.id)
      .then(setVersions)
      .catch(() => setVersions([]));
  }, [entry.id]);

  if (versions === null) return <Loader2 className="m-4 h-4 w-4 animate-spin" />;
  if (!versions.length) {
    return <p className="text-xs text-muted-foreground">No versions recorded.</p>;
  }
  return (
    <ul className="space-y-1 text-xs">
      {versions.map((version) => (
        <li key={version.version} className="flex items-center gap-2 rounded px-1 py-1 hover:bg-accent">
          <span className="font-mono">v{version.version}</span>
          <span className="flex-1 text-muted-foreground">{formatTime(version.created_at)}</span>
          <Button
            size="sm"
            variant="ghost"
            className="h-6 px-2"
            onClick={() =>
              void (async () => {
                try {
                  await restoreVersion(entry.id, version.version);
                  toast.success(`Rolled back to v${version.version}`);
                } catch (error) {
                  toast.error((error as Error).message);
                }
              })()
            }
          >
            Roll back
          </Button>
        </li>
      ))}
    </ul>
  );
}
