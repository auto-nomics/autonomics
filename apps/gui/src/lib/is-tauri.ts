/** True inside the Tauri desktop shell (gateway.ts keeps its own copy so
 *  this module stays dependency-free). */
export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
