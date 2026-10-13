export function createBackNavigation() {
  const handlers = new Map<symbol, { priority: number; handle: () => void }>();
  return {
    register(priority: number, handle: () => void): () => void {
      const id = Symbol();
      handlers.set(id, { priority, handle });
      return () => { handlers.delete(id); };
    },
    dispatch(): boolean {
      let top: { priority: number; handle: () => void } | undefined;
      for (const item of handlers.values()) {
        if (!top || item.priority >= top.priority) top = item;
      }
      if (!top) return false;
      top.handle();
      return true;
    },
  };
}

export const backNavigation = createBackNavigation();
export const backLayer = { screen: 0, detail: 10, editor: 20, menu: 30 } as const;

// A canceled event acknowledges one handled Back to the Android WebView host.
// No handler at the root lets the host background the task without killing audio.
if (typeof window !== "undefined") {
  const onBack = (event: Event) => {
    if (backNavigation.dispatch()) event.preventDefault();
  };
  window.addEventListener("memhot:navigate-back", onBack);
  import.meta.hot?.dispose(() => window.removeEventListener("memhot:navigate-back", onBack));
}
