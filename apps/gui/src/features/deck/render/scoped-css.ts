/**
 * Parse before scoping so broken CSS cannot escape the slide wrapper.
 * Keyframe and @font-face names are document-global even inside @scope;
 * rewrite definitions and references per rendered slide to keep PDF pages
 * and split workbench editors independent.
 */
export function scopeSlideCss(theme: string, override: string, scopeId: string): string {
  const parse = (css: string): CSSStyleSheet => {
    const sheet = new CSSStyleSheet();
    try { sheet.replaceSync(css); } catch { /* invalid CSS renders as empty */ }
    return sheet;
  };
  const sheets = [parse(theme), parse(override)];
  const animations = new Map<string, string>();
  const fonts = new Map<string, string>();
  const walk = (rules: CSSRuleList, visit: (rule: CSSRule) => void) => {
    for (const rule of Array.from(rules)) {
      visit(rule);
      if ("cssRules" in rule) walk((rule as CSSGroupingRule).cssRules, visit);
    }
  };
  for (const sheet of sheets) walk(sheet.cssRules, rule => {
    if (rule instanceof CSSStyleRule && /^:(?:root|scope)(?:\s*,\s*:(?:root|scope))*$/.test(rule.selectorText)) {
      // Theme-level root variables must be inherited by the slide subtree,
      // while the actual document root remains outside the CSS scope.
      rule.selectorText = rule.selectorText.replace(/:root/g, ":scope");
    }
    if (rule.type === CSSRule.KEYFRAMES_RULE) {
      const keyframes = rule as CSSKeyframesRule;
      const old = keyframes.name;
      if (!animations.has(old)) animations.set(old, `deck-${scopeId}-anim-${animations.size}`);
      keyframes.name = animations.get(old)!;
    } else if (rule.type === CSSRule.FONT_FACE_RULE) {
      const font = (rule as CSSFontFaceRule).style;
      const old = font.getPropertyValue("font-family").trim();
      const bare = old.replace(/^(["'])(.*)\1$/, "$2");
      if (!bare) return;
      if (!fonts.has(bare)) fonts.set(bare, `deck-${scopeId}-font-${fonts.size}`);
      font.setProperty("font-family", `"${fonts.get(bare)}"`);
    }
  });

  const escapeRegex = (value: string) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const replaceNames = (value: string, names: Map<string, string>, quoted: boolean) => {
    for (const [old, replacement] of names) {
      const bare = old.replace(/^(["'])(.*)\1$/, "$2");
      const name = escapeRegex(bare);
      const pattern = new RegExp(`(^|[\\s,])(?:"${name}"|'${name}'|${name})(?=$|[\\s,])`, "g");
      value = value.replace(pattern, (_, prefix: string) => `${prefix}${quoted ? `"${replacement}"` : replacement}`);
    }
    return value;
  };
  // Follow custom properties used by animation/font declarations. CSS
  // variables otherwise hide the global name from the declaration itself.
  const animationVars = new Set<string>();
  const fontVars = new Set<string>();
  const customProperties: CSSStyleDeclaration[] = [];
  const collectVars = (value: string, into: Set<string>) => {
    for (const match of value.matchAll(/var\(\s*(--[\w-]+)/g)) into.add(match[1]);
  };
  for (const sheet of sheets) walk(sheet.cssRules, rule => {
    if (!(rule instanceof CSSStyleRule)) return;
    const style = rule.style;
    customProperties.push(style);
    collectVars(`${style.getPropertyValue("animation")} ${style.getPropertyValue("animation-name")}`, animationVars);
    collectVars(`${style.getPropertyValue("font")} ${style.getPropertyValue("font-family")}`, fontVars);
  });
  for (const variables of [animationVars, fontVars]) {
    let previousSize = -1;
    while (previousSize !== variables.size) {
      previousSize = variables.size;
      for (const style of customProperties) for (const name of variables) collectVars(style.getPropertyValue(name), variables);
    }
  }
  for (const sheet of sheets) walk(sheet.cssRules, rule => {
    if (!(rule instanceof CSSStyleRule)) return;
    const style = rule.style;
    for (const [property, names, quoted] of [
      ["animation", animations, false], ["animation-name", animations, false],
      ["font", fonts, true], ["font-family", fonts, true],
    ] as const) {
      const value = style.getPropertyValue(property);
      if (!value) continue;
      const next = replaceNames(value, names, quoted);
      if (next !== value) style.setProperty(property, next, style.getPropertyPriority(property));
    }
    for (const name of new Set([...animationVars, ...fontVars])) {
      const value = style.getPropertyValue(name);
      if (!value) continue;
      let next = value;
      if (animationVars.has(name)) next = replaceNames(next, animations, false);
      if (fontVars.has(name)) next = replaceNames(next, fonts, true);
      if (next !== value) style.setProperty(name, next, style.getPropertyPriority(name));
    }
  });
  const serialize = (sheet: CSSStyleSheet) => Array.from(sheet.cssRules).map(rule => rule.cssText).join("\n");
  return `@layer deck-theme { @scope ([data-deck-style="${scopeId}"]) {\n${serialize(sheets[0])}\n} }\n@layer deck-override { @scope ([data-deck-style="${scopeId}"]) {\n${serialize(sheets[1])}\n} }`;
}
