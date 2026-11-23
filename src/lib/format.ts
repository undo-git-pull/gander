import type { Cell, Column, RowFormat } from "@/lib/api";

const NUMERIC = /^(TINYINT|SMALLINT|INTEGER|BIGINT|HUGEINT|UTINYINT|USMALLINT|UINTEGER|UBIGINT|UHUGEINT|FLOAT|DOUBLE|REAL|DECIMAL.*|NUMERIC.*)$/;

export const isNumericType = (t: string) => NUMERIC.test(t);

export const isNestedType = (t: string) =>
  t === "JSON" || t.startsWith("STRUCT") || t.startsWith("MAP") || t.startsWith("UNION") || t.endsWith("]");

export const MAX_FORMAT_BYTES = 1024 * 1024;

const jsonCache = new Map<string, unknown>();
const NOT_JSON = Symbol("not-json");

/** Parsed JSON for a cell, or undefined when the cell is not JSON. */
export function asJson(v: Cell, type: string): unknown {
  if (v === null) return undefined;
  if (typeof v === "object") return v; // nested types arrive parsed
  if (typeof v !== "string") return undefined;
  if (!(type === "VARCHAR" || type === "JSON" || isNestedType(type))) return undefined;
  const t = v.trimStart();
  if (!(t.startsWith("{") || t.startsWith("["))) return undefined;
  if (v.length > MAX_FORMAT_BYTES) return NOT_JSON; // too big to parse eagerly
  const hit = jsonCache.get(v);
  if (hit !== undefined) return hit === NOT_JSON ? undefined : hit;
  let parsed: unknown = NOT_JSON;
  try {
    parsed = JSON.parse(v);
  } catch {
    /* plain text */
  }
  if (jsonCache.size > 5000) jsonCache.clear();
  jsonCache.set(v, parsed);
  return parsed === NOT_JSON ? undefined : parsed;
}

export const isHugeJson = (v: Cell, type: string) => asJson(v, type) === NOT_JSON;

/** Single-line text for a cell. */
export function cellText(v: Cell): string {
  if (v === null) return "NULL";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

/** Raw value for copying: unformatted, NULL as empty string. */
export function rawCopy(v: Cell): string {
  if (v === null) return "";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

export function prettyJson(v: Cell, type: string): string | null {
  const j = asJson(v, type);
  if (j === undefined || j === NOT_JSON) return null;
  return JSON.stringify(j, null, 2);
}

/** Pretty JSON when possible, else raw text. */
export function prettyOrRaw(v: Cell, type: string): string {
  return prettyJson(v, type) ?? rawCopy(v);
}

const csvField = (s: string) => (/[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);
const tsvField = (s: string) => s.replace(/[\t\n\r]/g, " ");
const mdField = (s: string) => s.replace(/\|/g, "\\|").replace(/\n/g, " ");

export function formatRows(columns: Column[], rows: Cell[][], format: RowFormat): string {
  const names = columns.map((c) => c.name);
  switch (format) {
    case "json": {
      const objs = rows.map((r) => Object.fromEntries(names.map((n, i) => [n, r[i]])));
      return JSON.stringify(objs.length === 1 ? objs[0] : objs, null, 2);
    }
    case "csv":
      return [names.map(csvField).join(","), ...rows.map((r) => r.map((v) => csvField(rawCopy(v))).join(","))].join("\n");
    case "tsv":
      return [names.map(tsvField).join("\t"), ...rows.map((r) => r.map((v) => tsvField(rawCopy(v))).join("\t"))].join("\n");
    case "markdown":
      return [
        `| ${names.map(mdField).join(" | ")} |`,
        `|${names.map(() => "---").join("|")}|`,
        ...rows.map((r) => `| ${r.map((v) => mdField(v === null ? "NULL" : rawCopy(v))).join(" | ")} |`),
      ].join("\n");
  }
}

export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    /* fall back */
  }
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  document.execCommand("copy");
  ta.remove();
}

export type JsonToken = { t: string; c: string };

const TOKEN = /("(?:\\.|[^"\\])*")(\s*:)?|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|\b(true|false|null)\b|([{}[\],:])/g;

/** Tokens for muted syntax colouring of compact JSON in a grid cell. */
export function jsonTokens(text: string): JsonToken[] {
  const out: JsonToken[] = [];
  let last = 0;
  for (const m of text.matchAll(TOKEN)) {
    const i = m.index ?? 0;
    if (i > last) out.push({ t: text.slice(last, i), c: "" });
    if (m[1] !== undefined) {
      out.push({ t: m[1], c: m[2] ? "j-key" : "j-str" });
      if (m[2]) out.push({ t: m[2], c: "j-punc" });
    } else if (m[3] !== undefined) out.push({ t: m[3], c: "j-num" });
    else if (m[4] !== undefined) out.push({ t: m[4], c: "j-lit" });
    else out.push({ t: m[5], c: "j-punc" });
    last = i + m[0].length;
  }
  if (last < text.length) out.push({ t: text.slice(last), c: "" });
  return out;
}

export const fmtInt = (n: number) => n.toLocaleString("en-US");

export function fmtMs(ms: number | null | undefined): string {
  if (ms == null) return "";
  return ms < 1000 ? `${ms.toFixed(ms < 10 ? 1 : 0)} ms` : `${(ms / 1000).toFixed(2)} s`;
}
