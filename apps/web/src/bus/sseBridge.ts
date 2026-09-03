/**
 * SSE → Notifier 桥接工具
 *
 * 将后端 Server-Sent Events 的事件语义转换为 TransactionNotifier 的通知。
 * 使 SSE 推送也能享受去重、排序、观察者优先级等能力。
 *
 * SSE 事件与 notifier 映射：
 * - 论文解析完成 (done) → paper.modify (parseStatus: 'completed')
 * - 论文解析失败 (error) → paper.modify (parseStatus: 'error')
 * - 论文上传成功       → paper.add
 * - 论文删除成功       → paper.delete
 * - 集合创建           → folder.add
 * - 集合更新           → folder.modify
 * - 集合删除           → folder.delete
 */

import { notifier } from './index';

/** 将 SSE 论文解析完成事件桥接到 notifier */
export async function bridgeParseDone(
  paperId: string | number,
  parseEngine: string,
): Promise<void> {
  await notifier.trigger('modify', 'paper', [String(paperId)], {
    parseStatus: 'completed',
    parseEngine,
  });
}

/** 将 SSE 论文解析失败事件桥接到 notifier */
export async function bridgeParseError(
  paperId: string | number,
  errorMessage: string,
): Promise<void> {
  await notifier.trigger('modify', 'paper', [String(paperId)], {
    parseStatus: 'error',
    error: errorMessage,
  });
}

/** 将论文上传成功事件桥接到 notifier */
export async function bridgePaperAdded(
  paperId: string | number,
  extra?: Record<string, unknown>,
): Promise<void> {
  await notifier.trigger('add', 'paper', [String(paperId)], extra);
}

/** 将论文删除成功事件桥接到 notifier */
export async function bridgePaperDeleted(
  paperId: string | number,
): Promise<void> {
  await notifier.trigger('delete', 'paper', [String(paperId)]);
}

/** 将集合创建事件桥接到 notifier */
export async function bridgeFolderAdded(
  folderId: string | number,
  extra?: Record<string, unknown>,
): Promise<void> {
  await notifier.trigger('add', 'folder', [String(folderId)], extra);
}

/** 将集合更新事件桥接到 notifier */
export async function bridgeFolderModified(
  folderId: string | number,
  extra?: Record<string, unknown>,
): Promise<void> {
  await notifier.trigger('modify', 'folder', [String(folderId)], extra);
}

/** 将集合删除事件桥接到 notifier */
export async function bridgeFolderDeleted(
  folderId: string | number,
): Promise<void> {
  await notifier.trigger('delete', 'folder', [String(folderId)]);
}
