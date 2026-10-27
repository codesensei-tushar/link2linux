import { useEffect, useState } from "react";

/**
 * The nearest ancestor that scrolls, or null for the viewport. The observer
 * watches it, since a margin around the viewport would be cut off by the
 * scrolling page area.
 */
function scrollParent(element: Element): Element | null {
  for (let parent = element.parentElement; parent; parent = parent.parentElement) {
    const { overflowY } = getComputedStyle(parent);
    if (overflowY === "auto" || overflowY === "scroll") return parent;
  }
  return null;
}

/**
 * Whether an element is on screen or within `margin` of it, followed as the
 * user scrolls. Pass the returned callback as the element's `ref`. Where
 * IntersectionObserver is missing, every element counts as on screen.
 */
export function useInView(margin = "300px"): [(element: Element | null) => void, boolean] {
  const [element, setElement] = useState<Element | null>(null);
  const [inView, setInView] = useState(() => !("IntersectionObserver" in globalThis));

  useEffect(() => {
    if (!element || !("IntersectionObserver" in globalThis)) return;
    const observer = new IntersectionObserver(
      (entries) => {
        const latest = entries.at(-1);
        if (latest) setInView(latest.isIntersecting);
      },
      { root: scrollParent(element), rootMargin: margin },
    );
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, [element, margin]);

  return [setElement, inView];
}
