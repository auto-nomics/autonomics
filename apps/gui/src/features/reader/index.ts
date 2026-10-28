// Reader domain public surface: attachment readers opened from the library
// view. Everything else (annotation inspectors, reading mode, note composer)
// is internal to this domain.
export { openPdfReader } from "./pdf-reader";
export { openTxtReader } from "./txt-reader";
export type { ReaderSearchTarget } from "./reader-target";
