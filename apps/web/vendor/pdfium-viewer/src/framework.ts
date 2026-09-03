export {
  Fragment,
  useEffect,
  useRef,
  useState,
  useCallback,
  useMemo,
  useLayoutEffect,
  useReducer,
  memo,
  forwardRef,
  createContext,
  useContext,
  createElement,
} from 'react';

export {
  createPortal,
} from 'react-dom';

export type {
  ReactNode,
  HTMLAttributes,
  CSSProperties,
  MouseEvent,
  PointerEvent,
  TouchEvent,
  ComponentType,
  RefObject,
  ChangeEvent,
  FormEvent,
  KeyboardEvent,
  DragEvent,
  JSX,
} from 'react';

export const dblClickProp = 'onDoubleClick' as const;

export const suppressContentEditableWarningProps = {
  suppressContentEditableWarning: true,
};

export const selectProps = (isMultiple: boolean, selectedValues: string[]) => ({
  value: isMultiple ? selectedValues : selectedValues[0] || '',
});

export const optionProps = (
  isMultiple: boolean,
  selectedValues: string[],
  optionValue: string,
) => ({});
