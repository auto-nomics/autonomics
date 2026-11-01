/**
 * App-wide color theme: light / dark / follow-system.
 *
 * The resolved theme is applied as the `dark` class on
 * `<html>` (tailwind `@custom-variant dark` keys off it). The choice is
 * persisted in localStorage; "system" tracks `prefers-color-scheme` live.
 */

export type ThemeMode = "light" | "dark" | "system";

const STORAGE_KEY = "autonomics.theme";

const listeners = new Set<() => void>();
let mode: ThemeMode = "dark";
let media: MediaQueryList | null = null;
/** Stable snapshot — `useSyncExternalStore` compares by reference. */
let snapshot: { mode: ThemeMode; resolved: "light" | "dark" } = {
  mode,
  resolved: "dark",
};

function refresh() {
  snapshot = { mode, resolved: getResolvedTheme() };
  apply();
  emit();
}

function isMode(value: unknown): value is ThemeMode {
  return value === "light" || value === "dark" || value === "system";
}

function systemPrefersDark(): boolean {
  return media?.matches ?? false;
}

export function getThemeMode(): ThemeMode {
  return mode;
}

export function getResolvedTheme(): "light" | "dark" {
  return mode === "system" ? (systemPrefersDark() ? "dark" : "light") : mode;
}

function apply() {
  document.documentElement.classList.toggle("dark", getResolvedTheme() === "dark");
}

function emit() {
  for (const listener of listeners) listener();
}

export function setThemeMode(next: ThemeMode): void {
  if (!isMode(next) || next === mode) return;
  mode = next;
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    /* storage unavailable — keep the in-memory choice */
  }
  refresh();
}

/** Wire persistence, the system listener and the initial class. Call once. */
export function initTheme(): void {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isMode(stored)) mode = stored;
  } catch {
    /* storage unavailable */
  }
  media = window.matchMedia("(prefers-color-scheme: dark)");
  const onSystemChange = () => {
    if (mode === "system") refresh();
  };
  // Chromium-only target (Electron/browser) — addEventListener is universal
  // there, and the legacy addListener API is deprecated.
  media.addEventListener("change", onSystemChange);
  apply();
}

export function subscribeTheme(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Snapshot for `useSyncExternalStore` — must be referentially stable. */
export function getThemeSnapshot(): { mode: ThemeMode; resolved: "light" | "dark" } {
  return snapshot;
}

const CYCLE: readonly ThemeMode[] = ["light", "dark", "system"];

export function cycleThemeMode(): ThemeMode {
  const next = CYCLE[(CYCLE.indexOf(mode) + 1) % CYCLE.length];
  setThemeMode(next);
  return next;
}
