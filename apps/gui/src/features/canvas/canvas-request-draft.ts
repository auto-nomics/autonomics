import { appendDraftContext } from "@/features/context/context-draft";
export function appendCanvasRequestDraft(previous: string, request: string, snapshot: { title: string; source: string; revision: string; prompt: string }): string {
  return appendDraftContext(previous, request.trim(), {
    kind: "editor", id: `canvas-request:${crypto.randomUUID()}`, title: snapshot.title,
    scope: "准备时的图稿与审阅快照；修改要求以消息正文为准", source: snapshot.source, revision: snapshot.revision,
    content: `本附件是准备时的上下文快照。prepared_request 只记录当时的旧要求，执行要求以本条消息正文为准；正文要求只读或取消修改时，不得沿用旧要求生成候选。\n\n回复沿用用户本条正文的语言。默认用一至三句说明实际修改、待审或已采用状态及必要的核对项；不另写英文过程播报或逐工具执行记录，用户要求审计细节时再展开。工具名、路径编号、哈希和内部版本用于执行与核验，除非用户要求或排障需要，不逐项展示。SVG 局部修改会生成新文件与新哈希，并保留旧文件；不能将未更换素材来源或未改几何表述为“素材文件未动”。渲染贡献只表示指定渲染条件下的像素差，不能据此断言科学含义或未经核验的因果。本次交接授权范围是正文所述绘图任务；除非用户另外要求，不额外写入持久记忆或技能库。\n\n${snapshot.prompt}`,
  });
}
