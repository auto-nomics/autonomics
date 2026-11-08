import { useEffect, useState } from "react";
import { TextCodeEditor } from "@/components/editor/text-code-editor";
import { fetchDefaultDeckTheme, saveDeckTheme, saveSlideCss, type DeckPayload } from "./deck-api";

interface Props {
  payload: DeckPayload;
  currentSlide: number;
  refresh: () => Promise<DeckPayload>;
}

/** CSS drafts stay mounted while the user switches between preview and source. */
export function DeckCssEditor({ payload, currentSlide, refresh }: Props): React.ReactNode {
  const [target, setTarget] = useState<string>("theme");
  const [draft, setDraft] = useState("");
  const [saved, setSaved] = useState("");
  const [revision, setRevision] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const slideNumber = target === "theme" ? null : Number(target);
  const slide = slideNumber === null ? null : payload.slides[slideNumber - 1];
  const latest = slideNumber === null ? (payload.themeCss ?? "") : typeof slide?.frontmatter.slideCss === "string" ? slide.frontmatter.slideCss : "";
  const latestRevision = slideNumber === null ? (payload.themeRevision ?? "") : payload.deck.revision;
  const dirty = draft !== saved;

  // A clean editor follows remote changes; a dirty draft retains its own
  // revision so the server can reject conflicting saves.
  useEffect(() => {
    if (!dirty && !saving) { setDraft(latest); setSaved(latest); setRevision(latestRevision); }
  }, [latest, latestRevision, target, dirty, saving]);

  function choose(next: string) {
    setTarget(next);
    const page = next === "theme" ? null : payload.slides[Number(next) - 1];
    const css = next === "theme" ? payload.themeCss ?? "" : typeof page?.frontmatter.slideCss === "string" ? page.frontmatter.slideCss : "";
    setDraft(css); setSaved(css);
    setRevision(next === "theme" ? payload.themeRevision ?? "" : payload.deck.revision);
    setError(null);
  }

  async function save() {
    setSaving(true); setError(null);
    try {
      if (slideNumber === null) await saveDeckTheme(payload.deck.id, draft, revision);
      else await saveSlideCss(payload.deck.id, slideNumber, draft, revision);
      const next = await refresh();
      const nextSlide = slideNumber === null ? null : next.slides[slideNumber - 1];
      const nextCss = slideNumber === null ? next.themeCss ?? "" : typeof nextSlide?.frontmatter.slideCss === "string" ? nextSlide.frontmatter.slideCss : "";
      setDraft(nextCss); setSaved(nextCss);
      setRevision(slideNumber === null ? next.themeRevision ?? "" : next.deck.revision);
    } catch (e) {
      setError(`保存失败：${e instanceof Error ? e.message : String(e)}。若发生版本冲突，草稿仍保留；可复制后加载最新版本。`);
    } finally { setSaving(false); }
  }

  async function discard() {
    setSaving(true); setError(null);
    try {
      const next = await refresh();
      const nextSlide = slideNumber === null ? null : next.slides[slideNumber - 1];
      const nextCss = slideNumber === null ? next.themeCss ?? "" : typeof nextSlide?.frontmatter.slideCss === "string" ? nextSlide.frontmatter.slideCss : "";
      setDraft(nextCss); setSaved(nextCss);
      setRevision(slideNumber === null ? next.themeRevision ?? "" : next.deck.revision);
    } catch (e) { setError(`加载失败：${e instanceof Error ? e.message : String(e)}`); }
    finally { setSaving(false); }
  }

  async function loadDefault() {
    setSaving(true); setError(null);
    try { setDraft((await fetchDefaultDeckTheme(payload.deck.id)).css); }
    catch (e) { setError(`加载默认主题失败：${e instanceof Error ? e.message : String(e)}`); }
    finally { setSaving(false); }
  }

  return <div className="deck-source">
    <div className="deck-css-toolbar">
      <label htmlFor="deck-css-target">编辑样式</label>
      <select id="deck-css-target" value={target} disabled={dirty || saving} onChange={e => choose(e.target.value)}>
        <option value="theme">整套主题 · theme.css</option>
        {payload.slides.map(s => <option key={s.index} value={s.index + 1}>第 {s.index + 1} 页 · {s.title || "未命名"}{s.index + 1 === currentSlide ? "（当前页）" : ""}</option>)}
      </select>
      {slideNumber === null && <button type="button" className="deck-btn" disabled={dirty || saving} onClick={() => void loadDefault()}>载入默认主题</button>}
      <button type="button" className="deck-btn" disabled={!dirty || saving || !revision} onClick={() => void save()}>{saving ? "处理中…" : "保存 CSS"}</button>
      <button type="button" className="deck-btn" disabled={saving || !dirty} onClick={() => void discard()}>放弃更改并加载最新</button>
      <span>{dirty ? "未保存" : "已保存"}</span>
    </div>
    <p className="deck-css-help">主题应用于所有页面；单页样式写在该页的 slideCss frontmatter 中，后于主题加载。使用 :scope 设置页面背景，使用 .deck-prose h1 等选择器设置内容。保存草稿后可切换编辑对象；在主题的 :root 中同时设置 --deck-width 和 --deck-height（整数 px）可统一页面尺寸；不支持 @import。</p>
    {error && <div role="alert">{error}</div>}
    <TextCodeEditor documentKey={`${payload.deck.id}/${target}.css`} language="css" label={slideNumber === null ? "主题 CSS" : `第 ${slideNumber} 页 CSS`} value={draft} readOnly={saving || (slideNumber !== null && !slide)} onChange={setDraft} onSave={() => { if (dirty && !saving && revision) void save(); }} />
  </div>;
}
