import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { TextCodeEditor } from "@/components/editor/text-code-editor";
import { fetchDefaultDeckTheme, saveDeckTheme, saveSlideCss, type DeckPayload } from "./deck-api";

interface Props {
  payload: DeckPayload;
  currentSlide: number;
  refresh: () => Promise<DeckPayload>;
}

/** CSS drafts stay mounted while the user switches between preview and source. */
export function DeckCssEditor({ payload, currentSlide, refresh }: Props): React.ReactNode {
  const { t } = useTranslation("deck");
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
      setError(t("cssEditor.saveFailed", { error: e instanceof Error ? e.message : String(e) }));
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
    } catch (e) { setError(t("cssEditor.loadFailed", { error: e instanceof Error ? e.message : String(e) })); }
    finally { setSaving(false); }
  }

  async function loadDefault() {
    setSaving(true); setError(null);
    try { setDraft((await fetchDefaultDeckTheme(payload.deck.id)).css); }
    catch (e) { setError(t("cssEditor.defaultFailed", { error: e instanceof Error ? e.message : String(e) })); }
    finally { setSaving(false); }
  }

  return <div className="deck-source">
    <div className="deck-css-toolbar">
      <label htmlFor="deck-css-target">{t("cssEditor.label")}</label>
      <select id="deck-css-target" value={target} disabled={dirty || saving} onChange={e => choose(e.target.value)}>
        <option value="theme">{t("cssEditor.themeOption")}</option>
        {payload.slides.map(s => <option key={s.index} value={s.index + 1}>{t("cssEditor.slideOption", { n: s.index + 1, title: s.title || t("sidebar.untitled") })}{s.index + 1 === currentSlide ? t("cssEditor.current") : ""}</option>)}
      </select>
      {slideNumber === null && <button type="button" className="deck-btn" disabled={dirty || saving} onClick={() => void loadDefault()}>{t("cssEditor.loadDefault")}</button>}
      <button type="button" className="deck-btn" disabled={!dirty || saving || !revision} onClick={() => void save()}>{saving ? t("cssEditor.processing") : t("cssEditor.save")}</button>
      <button type="button" className="deck-btn" disabled={saving || !dirty} onClick={() => void discard()}>{t("cssEditor.discard")}</button>
      <span>{dirty ? t("cssEditor.unsaved") : t("cssEditor.saved")}</span>
    </div>
    <p className="deck-css-help">{t("cssEditor.help")}</p>
    {error && <div role="alert">{error}</div>}
    <TextCodeEditor documentKey={`${payload.deck.id}/${target}.css`} language="css" label={slideNumber === null ? t("cssEditor.themeLabel") : t("cssEditor.slideLabel", { n: slideNumber })} value={draft} readOnly={saving || (slideNumber !== null && !slide)} onChange={setDraft} onSave={() => { if (dirty && !saving && revision) void save(); }} />
  </div>;
}
