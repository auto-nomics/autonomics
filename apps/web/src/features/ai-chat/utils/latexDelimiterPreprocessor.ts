/**
 * latexDelimiterPreprocessor.ts
 * ============================================================
 * 在 markdown 解析前把 LaTeX 风格的 \(...\) / \[...\] 转换为
 * remark-math 能识别的 $...$ / $$...$$
 *
 * 为什么必须在解析前做字符串预处理：
 * - remark-math@6 只识别 $...$ 和 $$...$$，不识别 \(...\) / \[...\]
 * - CommonMark 解析时会把 \( 当作"反斜杠转义的 ("，吞掉反斜杠只留 (
 *   （反斜杠后跟 ASCII 标点属于可转义字符）
 * - 一旦解析为 mdast，text 节点的 value 已经丢失反斜杠，
 *   后续 remark/rehype 阶段再也找不回 \(...\) 边界
 *
 * 代码段保护：
 * - fenced code block（```...``` 或 ~~~...~~~）原样保留
 * - inline code（`...`）原样保留
 * 这样 AI 在代码示例中讲解 LaTeX 语法时贴的 \(...\) 文本不会被误转换
 *
 * @module ai-chat/utils/latexDelimiterPreprocessor
 */

/**
 * 同时匹配以下五种结构（按优先级，第一个匹配的胜出）：
 * 1. 反引号 fenced code block：```...```
 * 2. 波浪号 fenced code block：~~~...~~~
 * 3. inline code：`...`（单行，不含反引号和换行）
 * 4. LaTeX 行内公式：\( ... \)（非贪婪）
 * 5. LaTeX 块级公式：\[ ... \]（非贪婪）
 *
 * fenced code 的结束标记至少 3 个字符即可，这里用 ``` 字面匹配
 * （实际 markdown 中 ``` 和 ```` 都合法，但闭合标记是开头 fence 的同种字符）
 */
const TOKEN_RE =
  /```[\s\S]*?```|~~~[\s\S]*?~~~|`[^`\n]+`|\\\(([\s\S]+?)\\\)|\\\[([\s\S]+?)\\\]/g;

/**
 * 把 markdown 源串中的 LaTeX 风格分隔符转换为 remark-math 风格分隔符，
 * 同时保护 fenced code block 和 inline code 不被误处理。
 *
 * @param content 原始 markdown 字符串
 * @returns 转换后的 markdown 字符串（长度可能与输入不同）
 */
export function preprocessLatexDelimiters(content: string): string {
  if (!content || (!content.includes('\\(') && !content.includes('\\['))) {
    return content;
  }

  let result = '';
  let lastIdx = 0;
  let m: RegExpExecArray | null;
  TOKEN_RE.lastIndex = 0;

  while ((m = TOKEN_RE.exec(content)) !== null) {
    result += content.slice(lastIdx, m.index);

    if (m[1] !== undefined) {
      // \( ... \) → $ ... $
      result += `$${m[1]}$`;
    } else if (m[2] !== undefined) {
      // \[ ... \] → $$ ... $$
      result += `$$${m[2]}$$`;
    } else {
      // fenced code 或 inline code，原样保留
      result += m[0];
    }

    lastIdx = TOKEN_RE.lastIndex;
  }

  result += content.slice(lastIdx);
  return result;
}
