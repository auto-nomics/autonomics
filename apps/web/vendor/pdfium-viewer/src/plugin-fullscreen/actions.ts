import { Action } from '../core';

export const SET_FULLSCREEN = 'SET_FULLSCREEN';

export interface SetFullscreenAction extends Action {
  type: typeof SET_FULLSCREEN;
  payload: boolean;
}

export type FullscreenAction = SetFullscreenAction;

export function setFullscreen(payload: boolean): SetFullscreenAction {
  return { type: SET_FULLSCREEN, payload };
}
