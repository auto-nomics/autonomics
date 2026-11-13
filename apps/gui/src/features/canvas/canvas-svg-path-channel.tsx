import { createContext, useContext, useEffect, useId } from "react";
import type { SvgPathEvent, SvgPathSnapshot } from "./canvas-svg-path-context";

export const SvgPathChannel = createContext<((event: SvgPathEvent) => void) | null>(null);
export function useSvgPathInspection(snapshot: SvgPathSnapshot) {
  const publish = useContext(SvgPathChannel), owner = useId();
  const encoded = JSON.stringify(snapshot);
  useEffect(() => {
    if (!publish) return;
    publish({kind:"inspect",owner,snapshot:JSON.parse(encoded) as SvgPathSnapshot});
    return () => publish({kind:"hide",owner});
  },[publish,owner,encoded]);
}
