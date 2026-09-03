import { Reducer } from '../core';
import { FullscreenState } from './types';
import { FullscreenAction, SET_FULLSCREEN } from './actions';

export const initialState: FullscreenState = {
  isFullscreen: false,
};

export const reducer: Reducer<FullscreenState, FullscreenAction> = (state, action) => {
  switch (action.type) {
    case SET_FULLSCREEN:
      return { ...state, isFullscreen: action.payload };
    default:
      return state;
  }
};
