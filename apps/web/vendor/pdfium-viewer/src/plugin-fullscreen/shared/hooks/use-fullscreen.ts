import { useCapability, usePlugin } from '../../../core/shared';
import { FullscreenPlugin, FullscreenState, initialState } from '../..';
import { useState, useEffect } from '../../../framework';

export const useFullscreenPlugin = () => usePlugin<FullscreenPlugin>(FullscreenPlugin.id);
export const useFullscreenCapability = () => useCapability<FullscreenPlugin>(FullscreenPlugin.id);

export const useFullscreen = () => {
  const { provides } = useFullscreenCapability();
  const [state, setState] = useState<FullscreenState>(initialState);

  useEffect(() => {
    return provides?.onStateChange((state) => {
      setState(state);
    });
  }, [provides]);

  return {
    provides,
    state,
  };
};
