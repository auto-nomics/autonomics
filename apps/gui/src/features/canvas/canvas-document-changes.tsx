import type { FieldChange } from "./canvas-proposal-diff";

export function CanvasDocumentChanges({ fields }: { fields: FieldChange[] }) {
  if (!fields.length) return null;
  return <section aria-label="文档属性变化" className="mb-2 rounded border p-2">
    <p className="mb-1 font-medium">文档属性变化</p>
    <p>这些属性不属于单个画布对象，请核对来源和说明是否仍符合本次修改范围。</p>
    <table className="w-full table-fixed"><thead><tr><th className="w-1/4 text-left">字段</th><th className="text-left">修改前</th><th className="text-left">候选值</th></tr></thead>
      <tbody>{fields.map(field => <tr key={field.field}>
        <td className="break-all align-top">{field.field}</td>
        <td className="whitespace-pre-wrap break-all align-top">{field.before}</td>
        <td className="whitespace-pre-wrap break-all align-top">{field.after}</td>
      </tr>)}</tbody>
    </table>
  </section>;
}
