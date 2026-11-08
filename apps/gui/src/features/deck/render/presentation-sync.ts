import type { DeckPayload, SlideSpec } from '../deck-api';
import { clickTotal } from './deck-model';

export interface NavState { slide: number; click: number }
export type PresentationMessage =
  | { type: 'ready' }
  | { type: 'state'; payload: DeckPayload; nav: NavState; dark: boolean }
  | { type: 'step'; delta: 1 | -1 }
  | { type: 'goto'; slide: number }
  | { type: 'closed' };

export function stepNav({ slide, click }: NavState, slides: SlideSpec[], delta: 1 | -1): NavState {
  const current = slides[slide - 1];
  const total = current ? clickTotal(current) : 0;
  const start = current?.clickPlan?.clicksStart ?? 0;
  const c = Math.min(Math.max(click, start), total);
  if (delta === 1) {
    if (c < total) return { slide, click: c + 1 };
    if (slide < slides.length) return { slide: slide + 1, click: 0 };
  } else {
    if (c > start) return { slide, click: c - 1 };
    if (slide > 1) return { slide: slide - 1, click: clickTotal(slides[slide - 2]) };
  }
  return { slide, click: c };
}
