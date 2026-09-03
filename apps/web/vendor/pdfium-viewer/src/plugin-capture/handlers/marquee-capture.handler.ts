import { Position, Rect, Size } from '../../models';
import { clamp } from '../../core';
import {
  EmbedPdfPointerEvent,
  PointerEventHandlersWithLifecycle,
} from '../../plugin-interaction-manager';

export function createMarqueeHandler(opts: {
  pageSize: Size;
  scale: number;
  minDragPx?: number;
  onPreview?: (rect: Rect | null) => void;
  onCommit?: (rect: Rect) => void;
}): PointerEventHandlersWithLifecycle<EmbedPdfPointerEvent> {
  const { pageSize, scale, minDragPx = 5, onPreview, onCommit } = opts;

  let start: Position | null = null;
  let last: Rect | null = null;

  return {
    onPointerDown: (pos, evt) => {
      start = pos;
      last = { origin: { x: pos.x, y: pos.y }, size: { width: 0, height: 0 } };
      onPreview?.(last);
      evt.setPointerCapture?.();
    },
    onPointerMove: (pos) => {
      if (!start) return;
      const x = clamp(pos.x, 0, pageSize.width);
      const y = clamp(pos.y, 0, pageSize.height);
      last = {
        origin: { x: Math.min(start.x, x), y: Math.min(start.y, y) },
        size: { width: Math.abs(x - start.x), height: Math.abs(y - start.y) },
      };
      onPreview?.(last);
    },
    onPointerUp: (_pos, evt) => {
      if (last) {
        const dragPx = Math.max(last.size.width, last.size.height) * scale;
        if (dragPx > minDragPx) {
          onCommit?.(last);
        }
      }
      start = null;
      last = null;
      onPreview?.(null);
      evt.releasePointerCapture?.();
    },
    onPointerCancel: (_pos, evt) => {
      start = null;
      last = null;
      onPreview?.(null);
      evt.releasePointerCapture?.();
    },
  };
}
