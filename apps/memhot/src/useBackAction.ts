import { useLayoutEffect, useRef } from "react";
import { backLayer, backNavigation } from "./backNavigation";

/** Register only visible surfaces; explicit layers also cover portals and hidden drafts. */
export function useBackAction(active: boolean, action: () => void, layer: number = backLayer.screen) {
  const current = useRef(action);
  useLayoutEffect(() => { current.current = action; });
  useLayoutEffect(() => {
    if (active) return backNavigation.register(layer, () => current.current());
  }, [active, layer]);
}
