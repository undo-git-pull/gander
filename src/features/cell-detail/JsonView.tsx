import { useEffect, useRef } from "react";
import { EditorState } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { json } from "@codemirror/lang-json";
import { foldGutter, foldKeymap, codeFolding } from "@codemirror/language";
import { keymap } from "@codemirror/view";
import { baseTheme, highlight } from "@/lib/cm";

/** Read-only CodeMirror view for pretty-printed JSON (or plain text). */
export function JsonView({ text, isJson, className }: { text: string; isJson: boolean; className?: string }) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);

  useEffect(() => {
    if (!host.current) return;
    const exts = [
      EditorState.readOnly.of(true),
      EditorView.editable.of(false),
      EditorView.lineWrapping,
      baseTheme,
      highlight,
      lineNumbers(),
    ];
    if (isJson) exts.push(json(), codeFolding(), foldGutter(), keymap.of(foldKeymap));
    view.current = new EditorView({ parent: host.current, state: EditorState.create({ doc: text, extensions: exts }) });
    return () => {
      view.current?.destroy();
      view.current = null;
    };
  }, [text, isJson]);

  return <div ref={host} className={className} />;
}
