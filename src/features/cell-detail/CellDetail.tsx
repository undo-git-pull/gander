import { useState } from "react";
import { Check, Copy, Pin, X } from "lucide-react";
import type { Cell } from "@/lib/api";
import { MAX_FORMAT_BYTES, asJson, copyText, prettyJson, rawCopy } from "@/lib/format";
import { JsonView } from "./JsonView";
import { cn } from "@/lib/utils";

/** Header plus pretty-printed value. Used by the hover popover and the pinned panel. */
export function CellDetail({
  column,
  type,
  value,
  onPin,
  onClose,
  className,
}: {
  column: string;
  type: string;
  value: Cell;
  onPin?: () => void;
  onClose?: () => void;
  className?: string;
}) {
  const [force, setForce] = useState(false);
  const [copied, setCopied] = useState(false);
  const raw = rawCopy(value);
  const huge = raw.length > MAX_FORMAT_BYTES;
  const isJson = value !== null && (typeof value === "object" || (typeof value === "string" && /^\s*[[{]/.test(value)));
  let text = raw;
  let json = false;
  if (isJson && (!huge || force)) {
    const pretty = huge && force ? safePretty(raw) : prettyJson(value, type);
    if (pretty != null) {
      text = pretty;
      json = true;
    }
  }
  const copy = async () => {
    await copyText(json ? text : raw);
    setCopied(true);
    setTimeout(() => setCopied(false), 1000);
  };
  return (
    <div className={cn("flex min-h-0 flex-col", className)}>
      <div className="flex h-[var(--bar-h)] shrink-0 items-center gap-1 border-b px-1.5">
        <span className="truncate font-medium">{column}</span>
        <span className="truncate text-[var(--fs-sm)] text-muted-foreground">{type}</span>
        {value === null && <span className="text-muted-foreground">NULL</span>}
        <span className="ml-auto" />
        {huge && isJson && !force && (
          <button className="rounded px-1 text-[var(--fs-sm)] hover:bg-accent" onClick={() => setForce(true)}>
            Format anyway
          </button>
        )}
        <IconBtn title={json ? "Copy pretty JSON" : "Copy value"} onClick={copy}>
          {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
        </IconBtn>
        {onPin && (
          <IconBtn title="Pin to side panel (Enter)" onClick={onPin}>
            <Pin className="size-3" />
          </IconBtn>
        )}
        {onClose && (
          <IconBtn title="Close" onClick={onClose}>
            <X className="size-3" />
          </IconBtn>
        )}
      </div>
      <JsonView key={`${force}`} text={text} isJson={json} className="min-h-0 flex-1 overflow-auto" />
    </div>
  );
}

function safePretty(s: string): string | null {
  try {
    return JSON.stringify(JSON.parse(s), null, 2);
  } catch {
    return null;
  }
}

export function IconBtn({ title, onClick, children }: { title: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      title={title}
      onClick={onClick}
      className="inline-flex size-5 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
    >
      {children}
    </button>
  );
}

export const isJsonCell = (v: Cell, type: string) => asJson(v, type) !== undefined;
