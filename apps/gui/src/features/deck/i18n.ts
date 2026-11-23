// Deck namespace registration: the shared bootstrap in @/i18n only wires
// the "common" namespace, so the deck catalog registers itself here. Any
// module that calls i18next.t("deck:…") imports this file for the side
// effect; registration is idempotent.
import i18next from "i18next";
import "@/i18n";
import zhDeck from "@/i18n/locales/zh/deck.json";
import enDeck from "@/i18n/locales/en/deck.json";

if (!i18next.hasResourceBundle("zh", "deck")) {
  i18next.addResourceBundle("zh", "deck", zhDeck);
  i18next.addResourceBundle("en", "deck", enDeck);
}

export { i18next };
