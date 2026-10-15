import { useContext, useLayoutEffect, useRef } from "preact/hooks";
import { type Runtime, RuntimeContext } from "./runtime.ts";

export function useRuntime(): Runtime {
  const runtime = useContext(RuntimeContext);
  if (runtime === null) {
    throw new Error("components render inside <RuntimeContext.Provider>");
  }
  return runtime;
}

/**
 * Runs `callback` every frame with the session time. Time-driven styles are written here,
 * straight to the DOM through refs, so animation never goes through a re-render.
 */
export function useFrame(callback: (at: number) => void): void {
  const runtime = useRuntime();
  const latest = useRef(callback);
  latest.current = callback;
  useLayoutEffect(() => runtime.onFrame((at) => latest.current(at)), [runtime]);
}

/** Sets a style property only when its text changes; returns the setter for frame callbacks. */
export function styleWriter(): (
  element: HTMLElement | SVGElement | null,
  name: string,
  value: string,
) => void {
  const last = new WeakMap<Element, Map<string, string>>();
  return (element, name, value) => {
    if (element === null) {
      return;
    }
    let values = last.get(element);
    if (values === undefined) {
      values = new Map();
      last.set(element, values);
    }
    if (values.get(name) !== value) {
      values.set(name, value);
      element.style.setProperty(name, value);
    }
  };
}
