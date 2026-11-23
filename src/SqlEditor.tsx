import { useEffect, useRef } from "react";
// Only the editor core and SQL, not every language: keeps the bundle small.
import * as monaco from "../node_modules/monaco-editor/esm/vs/editor/editor.api.js";
// The SQL language, registered with Monaco. It ships without type declarations.
// @ts-ignore
import "../node_modules/monaco-editor/esm/vs/languages/definitions/sql/register.js";
// The shell language, for local bash scripts.
// @ts-ignore
import "../node_modules/monaco-editor/esm/vs/languages/definitions/shell/register.js";
import EditorWorker from "../node_modules/monaco-editor/esm/vs/editor/editor.worker.js?worker";

// Monaco (the editor inside VS Code) runs its worker from the app bundle, so nothing is fetched from a CDN.
(globalThis as unknown as { MonacoEnvironment: unknown }).MonacoEnvironment = {
  getWorker: () => new EditorWorker(),
};

function isDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

/** A code editor (Monaco) for SQL or shell. Ctrl/Cmd+Enter runs the statement or script. */
export function SqlEditor(props: {
  value: string;
  /** Monaco language: "sql" (default) or "shell". */
  language?: "sql" | "shell";
  readOnly?: boolean;
  onChange?: (value: string) => void;
  onRun?: () => void;
  onCursor?: (line: number, column: number) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  // The editor is created once; it reads the latest callbacks through this ref.
  const latest = useRef(props);
  latest.current = props;

  useEffect(() => {
    if (!host.current) return;
    const editor = monaco.editor.create(host.current, {
      value: props.value,
      language: props.language ?? "sql",
      readOnly: !!props.readOnly,
      theme: isDark() ? "vs-dark" : "vs",
      fontSize: 13,
      fontFamily: "ui-monospace, Menlo, monospace",
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      automaticLayout: true,
      wordWrap: "on",
      tabSize: 2,
      renderLineHighlight: "line",
      lineNumbersMinChars: 3,
    });
    editor.onDidChangeModelContent(() => latest.current.onChange?.(editor.getValue()));
    editor.onDidChangeCursorPosition((e) => latest.current.onCursor?.(e.position.lineNumber, e.position.column));
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => latest.current.onRun?.());
    editorRef.current = editor;
    return () => {
      editor.dispose();
      editorRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Text changed from outside (another tab, a saved statement loaded): show it.
  useEffect(() => {
    const editor = editorRef.current;
    if (editor && editor.getValue() !== props.value) editor.setValue(props.value);
  }, [props.value]);

  useEffect(() => {
    editorRef.current?.updateOptions({ readOnly: !!props.readOnly });
  }, [props.readOnly]);

  return <div ref={host} className="sql-editor" />;
}
