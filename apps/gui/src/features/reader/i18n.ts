// Reader namespace registration: the shared bootstrap in @/i18n only wires
// the "common" namespace, so the reader catalog registers itself here. Any
// module that calls i18next.t("reader:…") imports this file for the side
// effect; registration is idempotent.
import i18next from "i18next";
import "@/i18n";
import zhReader from "@/i18n/locales/zh/reader.json";
import enReader from "@/i18n/locales/en/reader.json";

if (!i18next.hasResourceBundle("zh", "reader")) {
  i18next.addResourceBundle("zh", "reader", zhReader);
  i18next.addResourceBundle("en", "reader", enReader);
}

export { i18next };
