/**
 * Ambient declarations for Vite's `?raw` query suffix.
 *
 * Vite inlines the file contents as a string when imported with `?raw`.
 * We don't depend on `vite/client` from this library package (consumers
 * like @autonomics/web have it), so declare the modules ambientally here.
 */

declare module '*?raw' {
    const content: string;
    export default content;
}

declare module '*.csl?raw' {
    const content: string;
    export default content;
}

declare module '*.xml?raw' {
    const content: string;
    export default content;
}
