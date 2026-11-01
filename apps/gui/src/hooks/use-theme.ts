import { useSyncExternalStore } from "react";

import { getThemeSnapshot, subscribeTheme } from "@/lib/theme";

/** Reactive `{ mode, resolved }` for the app color theme. */
export function useTheme() {
  return useSyncExternalStore(subscribeTheme, getThemeSnapshot, getThemeSnapshot);
}
