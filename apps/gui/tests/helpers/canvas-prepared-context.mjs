import assert from 'node:assert/strict';
export async function readCanvasPreparedPrompt(page, title="绘图请求上下文") {
 const section=page.getByRole('region',{name:'本次消息附件',exact:true});await section.waitFor();const items=section.locator('details').filter({has:page.locator('summary').filter({hasText:title})});assert.ok(await items.count()>0,'prepared drawing context is a visible attachment');return items.last().locator('[data-draft-context-content]').textContent();
}
export async function readCanvasPreparedContext(page) { const prompt=await readCanvasPreparedPrompt(page);return JSON.parse(prompt.slice(prompt.lastIndexOf('\n\n{')+2)); }


export async function readPlanPreparedPrompt(page) { return readCanvasPreparedPrompt(page,"计划审阅上下文"); }
export async function readPlanPreparedContext(page) { const prompt=await readPlanPreparedPrompt(page);return JSON.parse(prompt.slice(prompt.lastIndexOf('\n\n{')+2)); }
export async function readPathPreparedPrompt(page) { return readCanvasPreparedPrompt(page,"SVG 路径上下文"); }
export async function readPathPreparedContext(page) { const prompt=await readPathPreparedPrompt(page);return JSON.parse(prompt.slice(prompt.lastIndexOf('\n\n{')+2)); }
