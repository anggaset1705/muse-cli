// Code editor with VS Code-style syntax highlighting (CodeMirror 6).
//
// Replaces the plain <textarea> in WorkspacePanel. The editor's source of
// truth stays the parent's `edited` string, so dirty tracking, Ctrl+S,
// concurrency check and Save behaviour are unchanged — only the rendering
// is coloured. Language is picked from the file extension; unknown
// extensions fall back to plain text. Files above HIGHLIGHT_LIMIT keep
// the same editor but without a language (highlighting a huge doc would
// stall the UI on every keystroke).

import { useMemo, useRef } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { oneDark } from "@codemirror/theme-one-dark";
import { EditorView, keymap } from "@codemirror/view";
import type { Extension } from "@codemirror/state";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { markdown } from "@codemirror/lang-markdown";
import { xml } from "@codemirror/lang-xml";
import { yaml } from "@codemirror/lang-yaml";
import { sql } from "@codemirror/lang-sql";

/// Above this size the language is dropped (plain editor) so typing in a
/// huge file stays responsive. The 2 MB Rust read limit still applies.
export const HIGHLIGHT_LIMIT = 500_000;

function languageFor(rel: string): Extension | null {
  const ext = rel.split(".").pop()?.toLowerCase() ?? "";
  switch (ext) {
    case "json":
    case "jsonc":
    case "json5":
      return json();
    case "js":
    case "mjs":
    case "cjs":
    case "jsx":
      return javascript({ jsx: true });
    case "ts":
    case "mts":
    case "cts":
    case "tsx":
      return javascript({ typescript: true, jsx: true });
    case "py":
      return python();
    case "rs":
      return rust();
    case "html":
    case "htm":
      return html();
    case "css":
    case "scss":
    case "less":
      return css();
    case "md":
    case "markdown":
      return markdown();
    case "xml":
    case "svg":
    case "xsl":
      return xml();
    case "yaml":
    case "yml":
      return yaml();
    case "sql":
      return sql();
    default:
      return null;
  }
}

// One Dark tokens, re-skinned onto the app palette (bg #17171D, panel
// #292A34, accent #3867F5) so the editor matches the rest of the UI.
const appTheme = EditorView.theme(
  {
    "&": {
      backgroundColor: "#17171D",
      color: "#F5F5F7",
      fontSize: "12px",
    },
    ".cm-content": {
      fontFamily:
        "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace",
      caretColor: "#F5F5F7",
    },
    ".cm-gutters": {
      backgroundColor: "#17171D",
      color: "#71717A",
      border: "none",
    },
    ".cm-activeLineGutter": { backgroundColor: "#292A34" },
    ".cm-activeLine": { backgroundColor: "#22232C" },
    ".cm-selectionBackground": { backgroundColor: "#3867F5" },
  },
  { dark: true },
);

interface CodeEditorProps {
  value: string;
  onChange: (value: string) => void;
  /** File path, used only to pick the language (e.g. "src/foo.ts"). */
  fileRel: string;
  /** Called on Ctrl/Cmd+S (same as the Save button). */
  onSave: () => void;
}

export function CodeEditor({ value, onChange, fileRel, onSave }: CodeEditorProps) {
  // The keymap closure must always call the latest onSave (it reads the
  // parent's current `edited` state), so it goes through a ref.
  const saveRef = useRef(onSave);
  saveRef.current = onSave;
  const saveKeymap = useMemo(
    () =>
      keymap.of([
        {
          key: "Mod-s",
          run: () => {
            saveRef.current();
            return true;
          },
        },
      ]),
    [],
  );

  const tooLarge = value.length > HIGHLIGHT_LIMIT;
  const extensions = useMemo(() => {
    const exts: Extension[] = [saveKeymap];
    if (!tooLarge) {
      const lang = languageFor(fileRel);
      if (lang) exts.push(lang);
    }
    return exts;
  }, [fileRel, tooLarge, saveKeymap]);

  return (
    <div className="h-64 w-full overflow-hidden bg-[#17171D]" aria-label={fileRel}>
      <CodeMirror
        value={value}
        height="256px"
        theme={[oneDark, appTheme]}
        extensions={extensions}
        onChange={(v) => onChange(v)}
        basicSetup={{
          foldGutter: true,
          dropCursor: false,
          allowMultipleSelections: true,
        }}
      />
    </div>
  );
}
