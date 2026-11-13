import { createContext, useContext } from "react";
export const CanvasIllustratorContext = createContext("");
export const useCanvasIllustratorPath = () => useContext(CanvasIllustratorContext);
