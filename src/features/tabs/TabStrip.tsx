import { useState } from "react";
import { Plus, X } from "lucide-react";
import { api } from "@/lib/api";
import { useStore } from "@/store";
import { cn } from "@/lib/utils";

export function TabStrip() {
  const order = useStore((s) => s.order);
  const tabs = useStore((s) => s.tabs);
  const active = useStore((s) => s.active);
  const [editing, setEditing] = useState<string | null>(null);
  const [drag, setDrag] = useState<string | null>(null);

  return (
    <div className="flex h-[var(--bar-h)] shrink-0 items-stretch overflow-x-auto border-b bg-muted/40 [&::-webkit-scrollbar]:h-0">
      {order.map((id, i) => {
        const t = tabs[id];
        if (!t) return null;
        const isActive = id === active;
        return (
          <div
            key={id}
            draggable={editing !== id}
            onDragStart={() => setDrag(id)}
            onDragOver={(e) => e.preventDefault()}
            onDrop={() => {
              if (drag && drag !== id) void api.moveTab(drag, i);
              setDrag(null);
            }}
            onMouseDown={(e) => {
              if (e.button === 1) {
                e.preventDefault();
                void api.closeTab(id);
              } else if (e.button === 0 && !isActive) void api.activateTab(id);
            }}
            onDoubleClick={() => setEditing(id)}
            title={`${t.title}${t.truncated ? " (truncated)" : ""}\nDouble click to rename. ${i < 9 ? `Cmd+${i + 1}` : ""}`}
            className={cn(
              "group flex max-w-52 min-w-0 shrink-0 items-center gap-1 border-r pr-0.5 pl-2",
              isActive ? "bg-background text-foreground" : "text-muted-foreground hover:bg-accent/60",
            )}
          >
            {t.status === "running" && <span className="size-1.5 shrink-0 animate-pulse rounded-full bg-[var(--selection-border)]" />}
            {t.status === "error" && <span className="size-1.5 shrink-0 rounded-full bg-destructive" />}
            {editing === id ? (
              <input
                autoFocus
                defaultValue={t.custom_title ? t.title : ""}
                placeholder={t.title}
                className="h-4 w-28 rounded-sm border bg-background px-1 text-[var(--fs-sm)] outline-none"
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    void api.renameTab(id, (e.target as HTMLInputElement).value);
                    setEditing(null);
                  } else if (e.key === "Escape") setEditing(null);
                }}
                onBlur={(e) => {
                  void api.renameTab(id, e.target.value);
                  setEditing(null);
                }}
              />
            ) : (
              <span className="truncate">
                {t.title}
                {t.truncated && <sup className="ml-px text-[var(--warning)]">+</sup>}
                {t.dirty && t.has_result && <span className="text-muted-foreground">*</span>}
              </span>
            )}
            <button
              title="Close (Cmd+W)"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => void api.closeTab(id)}
              className={cn(
                "ml-0.5 inline-flex size-4 shrink-0 items-center justify-center rounded-sm hover:bg-accent",
                isActive ? "opacity-70" : "opacity-0 group-hover:opacity-70",
              )}
            >
              <X className="size-3" />
            </button>
          </div>
        );
      })}
      <button
        title="New tab (Cmd+T)"
        onClick={() => void api.newTab()}
        className="inline-flex w-6 shrink-0 items-center justify-center text-muted-foreground hover:bg-accent hover:text-foreground"
      >
        <Plus className="size-3.5" />
      </button>
    </div>
  );
}
