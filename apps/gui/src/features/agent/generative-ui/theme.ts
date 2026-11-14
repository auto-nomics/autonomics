// Iframes cannot inherit CSS variables. Resolve the GUI palette at the boundary.
export function drawingTheme(host: HTMLElement): string {
  const css = getComputedStyle(host);
  const token = (name: string) => css.getPropertyValue(name).trim();
  const palette: Record<string, string> = {
    "--color-background-primary": token("--background"),
    "--color-background-secondary": token("--card"),
    "--color-background-tertiary": token("--muted"),
    "--color-text-primary": token("--foreground"),
    "--color-text-secondary": token("--muted-foreground"),
    "--color-text-tertiary": token("--muted-foreground"),
    "--color-border-primary": token("--ring"),
    "--color-border-secondary": token("--input"),
    "--color-border-tertiary": token("--border"),
    "--font-sans": token("--font-sans"),
  };
  for (const [kind, name] of Object.entries({ info: "--primary", danger: "--destructive", success: "--chart-2", warning: "--chart-4" })) {
    palette[`--color-text-${kind}`] = token(name);
    palette[`--color-border-${kind}`] = token(name);
    palette[`--color-background-${kind}`] = `color-mix(in srgb, ${token(name)} 12%, ${token("--background")})`;
  }
  for (const name of ["--background", "--foreground", "--primary", "--primary-foreground", "--muted", "--muted-foreground", "--border", "--ring", "--chart-1", "--chart-2", "--chart-3", "--chart-4", "--chart-5"]) palette[name] = token(name);
  return `:root{color-scheme:${document.documentElement.classList.contains("dark") ? "dark" : "light"};${Object.entries(palette).filter(([, v]) => v).map(([k, v]) => `${k}:${v} !important;`).join("")}}html{background:var(--color-background-primary) !important}`;
}

export function applyDrawingTheme(css: string): string {
  return `(() => { let style = document.getElementById('__gui_theme'); if (!style) { style = document.createElement('style'); style.id = '__gui_theme'; document.head.appendChild(style); } style.textContent = ${JSON.stringify(css)}; window.dispatchEvent(new Event('themechange')); })()`;
}
