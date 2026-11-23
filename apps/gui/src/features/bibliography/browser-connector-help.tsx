import { Puzzle } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { useState } from "react";
import { SearchDialog } from "./search-dialog";

export function BrowserConnectorHelp() {
  const { t } = useTranslation("bibliography");
  const [open, setOpen] = useState(false);
  const gateway = /^https?:$/.test(window.location.protocol) && !("__TAURI_INTERNALS__" in window)
    ? window.location.origin : "http://127.0.0.1:8765";
  return <>
    <Button size="icon-sm" variant="ghost" title={t("browserConnector.buttonTitle")} aria-label={t("browserConnector.buttonTitle")} onClick={() => setOpen(true)}><Puzzle /></Button>
    {open && <SearchDialog title={t("browserConnector.title")} onClose={() => setOpen(false)}>
      <div className="space-y-4 text-sm">
        <p>{t("browserConnector.intro")}</p>
        <ol className="list-decimal space-y-2 pl-5">
          <li>{t("browserConnector.step1")}</li>
          <li>{t("browserConnector.step2")}</li>
          <li>{t("browserConnector.step3")}</li>
          <li>{t("browserConnector.step4")}</li>
        </ol>
        <div className="rounded border p-3"><div className="font-medium">{t("browserConnector.gatewayAddress")}</div><code className="break-all">{gateway}</code><p className="mt-1 text-xs text-muted-foreground">{t("browserConnector.gatewayHint")}</p></div>
        <p>{t("browserConnector.tokenNote")}</p>
        <p className="text-muted-foreground">{t("browserConnector.scopeNote")}</p>
      </div>
    </SearchDialog>}
  </>;
}
