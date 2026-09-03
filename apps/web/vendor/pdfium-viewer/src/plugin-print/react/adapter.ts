import { ReactNode } from 'react';
import { createRoot } from 'react-dom/client';

export { Fragment, useEffect, useRef, useState, createContext, useContext, ReactNode } from 'react';
export type { CSSProperties, HTMLAttributes } from 'react';

const rootMap = new WeakMap<Element, any>();

export function render(vnode: ReactNode | null, container: Element | null): void {
  if (!container) return;

  if (vnode === null) {
    // Unmounting
    const root = rootMap.get(container);
    if (root) {
      root.unmount();
      rootMap.delete(container);
    }
  } else {
    // Mounting/updating
    let root = rootMap.get(container);
    if (!root) {
      root = createRoot(container);
      rootMap.set(container, root);
    }
    root.render(vnode);
  }
}
