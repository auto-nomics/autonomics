export const LANGUAGE_STORAGE_KEY = "autonomics.lang";

export type Language = "zh" | "en";

export const languages: ReadonlyArray<{ value: Language; label: string }> = [
  { value: "zh", label: "中文" },
  { value: "en", label: "English" },
];

export function isLanguage(value: unknown): value is Language {
  return value === "zh" || value === "en";
}

export function resolveLanguage(): Language {
  try {
    const stored = localStorage.getItem(LANGUAGE_STORAGE_KEY);
    if (isLanguage(stored)) return stored;
  } catch {
    // ignore unavailable storage
  }
  const nav = typeof navigator !== "undefined" ? navigator.language : "zh";
  return nav?.toLowerCase().startsWith("zh") ? "zh" : "en";
}
