export { cn } from "cn"

/**
 * crypto.randomUUID 只在安全上下文(HTTPS / localhost)里存在:网关以
 * 纯 HTTP 跑在局域网 IP 上时它是 undefined,裸调会在渲染期抛错掀翻
 * 整棵 React 树。getRandomValues 无此限制,作为回退即可。
 */
export function newId(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
