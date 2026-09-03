/**
 * AnnotationManager — PDF 标注撤销/重做系统（Command 模式）
 *
 * 职责：
 * - 维护 undo/redo 双栈，支持标注的创建、删除、更新三种操作
 * - 相邻同类型命令在 MERGE_WINDOW_MS 内自动合并（如连续拖拽产生多次 update）
 * - 栈深度限制为 MAX_UNDO_DEPTH，超出后丢弃最早的命令
 * - 防抖保存：操作后延迟 1 秒触发 onChange 回调，由父组件持久化
 *
 * @module AnnotationManager
 */

export interface Annotation {
  id: string;
  text: string;
  color?: string;
  rects?: Array<{ x: number; y: number; w: number; h: number }>;
  approximate_y?: number;
  /** 其他动态字段 */
  [key: string]: unknown;
}

export type AnnotationChangeType = 'create' | 'delete' | 'update';

export interface AnnotationCommand {
  type: AnnotationChangeType;
  annotation: Annotation;
  /** update 操作的先前状态，用于撤销 */
  previousState?: Annotation;
  timestamp: number;
}

const MAX_UNDO_DEPTH = 50;
const MERGE_WINDOW_MS = 500;

export class AnnotationManager {
  private undoStack: AnnotationCommand[] = [];
  private redoStack: AnnotationCommand[] = [];
  private lastCommandTime: number = 0;
  private pendingSaveTimer: ReturnType<typeof setTimeout> | null = null;
  private onChange?: (annotations: Annotation[]) => void;

  constructor(onChange?: (annotations: Annotation[]) => void) {
    this.onChange = onChange;
  }

  /**
   * 执行一条命令：推入 undo 栈并清空 redo 栈。
   * 如果与上一条命令类型相同且在合并窗口内，则替换栈顶而非追加。
   */
  execute(command: AnnotationCommand): void {
    const now = Date.now();

    // 与前一条命令合并（同类型 + 时间窗口内）
    if (
      now - this.lastCommandTime < MERGE_WINDOW_MS &&
      this.undoStack.length > 0 &&
      this.undoStack[this.undoStack.length - 1].type === command.type &&
      this.undoStack[this.undoStack.length - 1].annotation.id === command.annotation.id
    ) {
      this.undoStack[this.undoStack.length - 1] = command;
    } else {
      this.undoStack.push(command);
    }

    // 超过最大深度时丢弃最早命令
    if (this.undoStack.length > MAX_UNDO_DEPTH) {
      this.undoStack.shift();
    }

    this.redoStack = [];
    this.lastCommandTime = now;
    this.scheduleSave();
  }

  /** 撤销：弹出 undo 栈顶并推入 redo 栈 */
  undo(): AnnotationCommand | null {
    const command = this.undoStack.pop();
    if (!command) return null;
    this.redoStack.push(command);
    this.lastCommandTime = Date.now();
    this.scheduleSave();
    return command;
  }

  /** 重做：弹出 redo 栈顶并推入 undo 栈 */
  redo(): AnnotationCommand | null {
    const command = this.redoStack.pop();
    if (!command) return null;
    this.undoStack.push(command);
    this.lastCommandTime = Date.now();
    this.scheduleSave();
    return command;
  }

  canUndo(): boolean {
    return this.undoStack.length > 0;
  }

  canRedo(): boolean {
    return this.redoStack.length > 0;
  }

  /** 更新 redo 栈顶命令的 annotation（用于撤销删除后同步新 ID） */
  updateRedoTop(annotation: Annotation): void {
    if (this.redoStack.length > 0) {
      this.redoStack[this.redoStack.length - 1].annotation = annotation;
    }
  }

  /** 清空所有命令和待处理的保存计时器 */
  clear(): void {
    this.undoStack = [];
    this.redoStack = [];
    if (this.pendingSaveTimer) {
      clearTimeout(this.pendingSaveTimer);
      this.pendingSaveTimer = null;
    }
  }

  /** 防抖保存（1 秒延迟，避免频繁写盘） */
  private scheduleSave(): void {
    // 如果没有 onChange 回调，无需启动无用的定时器
    if (!this.onChange) return;

    if (this.pendingSaveTimer) {
      clearTimeout(this.pendingSaveTimer);
    }
    this.pendingSaveTimer = setTimeout(() => {
      this.pendingSaveTimer = null;
      // 实际持久化由父组件通过 onChange 回调处理
      if (this.onChange) {
        this.onChange(this.getUndoStackAnnotations());
      }
    }, 1000);
  }

  /** 获取当前 undo 栈中的所有标注（用于持久化） */
  private getUndoStackAnnotations(): Annotation[] {
    return this.undoStack.map(cmd => cmd.annotation);
  }
}
