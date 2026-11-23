import type { PanelImperativeHandle } from "react-resizable-panels";

/** Handle to the left sidebar panel, set by App. */
export const sidebarRef: { current: PanelImperativeHandle | null } = { current: null };

export function toggleSidebar() {
  const p = sidebarRef.current;
  if (!p) return;
  if (p.isCollapsed()) p.expand();
  else p.collapse();
}

export function expandSidebar() {
  const p = sidebarRef.current;
  if (p?.isCollapsed()) p.expand();
}
