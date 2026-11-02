// Visibility is a scheduling hint; it never starts a second network check.
export function watchVisibleServers(root: Element, publish: (names: string[]) => void) {
  const tracked = new Set<Element>(), visible = new Set<Element>();
  let last = "";
  const update = () => {
    const names = [...root.querySelectorAll<HTMLElement>("[data-server-name]")]
      .filter(card => visible.has(card)).map(card => card.dataset.serverName!).slice(0, 32);
    const key = JSON.stringify(names);
    if (key !== last) { last = key; publish(names); }
  };
  const observer = new IntersectionObserver(entries => {
    for (const entry of entries) {
      if (entry.isIntersecting) visible.add(entry.target); else visible.delete(entry.target);
    }
    update();
  }, { rootMargin: "100px" });
  const reconcile = () => {
    const cards = new Set(root.querySelectorAll("[data-server-name]"));
    for (const card of tracked) if (!cards.has(card)) {observer.unobserve(card);tracked.delete(card);visible.delete(card);}
    for (const card of cards) if (!tracked.has(card)) {tracked.add(card);observer.observe(card);}
    update();
  };
  const changes = new MutationObserver(reconcile);
  changes.observe(root, { childList: true, subtree: true });
  reconcile();
  return () => { changes.disconnect(); observer.disconnect(); publish([]); };
}
