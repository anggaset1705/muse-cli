// Code editor with VS Code-style syntax highlighting (CodeMirror 6).
//
// Replaces the plain <textarea> in WorkspacePanel. The editor's source of
// truth stays the parent's `edited` string, so dirty tracking, Ctrl+S,
// concurrency check and Save behaviour are unchanged — only the rendering
// is coloured. Language is picked from the file extension; unknown
// extensions fall back to plain text. Files above HIGHLIGHT_LIMIT keep
// the same editor but without a language (highlighting a huge doc would
// stall the UI on every keystroke).

import { useLayoutEffect, useMemo, useRef } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { oneDark } from "@codemirror/theme-one-dark";
import { Decoration, EditorView, keymap } from "@codemirror/view";
import { EditorState, type Extension } from "@codemirror/state";
import { reconcileScrollFollow } from "@/lib/scrollFollow";
import type { ChangedLineMarkers } from "@/lib/liveEditDiff";
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
    ".cm-scroller": { overscrollBehavior: "contain" },
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
  /** Read-only native agent edit preview; never changes the user's editor buffer. */
  readOnly?: boolean;
  markers?: ChangedLineMarkers;
  followLatest?: boolean;
  /** Follow the REAL currently modified line, not the bottom of the file. */
  followLine?: number;
}

const liveDiffTheme = EditorView.theme({
  ".cm-live-added-line": { backgroundColor: "rgba(32, 185, 119, 0.17)", borderLeft: "3px solid #28BC7A" },
  ".cm-live-modified-line": { backgroundColor: "rgba(235, 178, 49, 0.13)", borderLeft: "3px solid #D9A441" },
});

export function CodeEditor({ value, onChange, fileRel, onSave, readOnly = false, markers, followLatest = false, followLine }: CodeEditorProps) {
  // The keymap closure must always call the latest onSave (it reads the
  // parent's current `edited` state), so it goes through a ref.
  const saveRef = useRef(onSave);
  saveRef.current = onSave;
  const viewRef = useRef<EditorView | null>(null);
  const followingRef = useRef(true);
  const manualRef = useRef(false);
  useLayoutEffect(() => {
    if (!followLatest || !followingRef.current) return;
    // CM6 applies external value after React props change. Scroll in the
    // next animation frame to the actual changed line in the updated doc.
    const request = window.requestAnimationFrame(() => {
      const view = viewRef.current;
      if (!view || !followingRef.current) return;
      if (followLine && Number.isFinite(followLine)) {
        const line = Math.min(view.state.doc.lines, Math.max(1, Math.floor(followLine)));
        view.dispatch({
          effects: EditorView.scrollIntoView(view.state.doc.line(line).from, { y: "center" }),
        });
      } else {
        view.scrollDOM.scrollTop = view.scrollDOM.scrollHeight;
      }
    });
    return () => window.cancelAnimationFrame(request);
  }, [value, followLatest, followLine]);
  const onEditorScroll = () => {
    const el = viewRef.current?.scrollDOM;
    if (!el || !followLatest) return;
    followingRef.current = reconcileScrollFollow(followingRef.current, manualRef.current, el, 24);
    manualRef.current = false;
  };
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
    const exts: Extension[] = [EditorState.readOnly.of(readOnly), EditorView.editable.of(!readOnly)];
    if (!readOnly) exts.push(saveKeymap);
    if (markers) {
      exts.push(liveDiffTheme, EditorView.decorations.of((view) => {
        const max = view.state.doc.lines;
        const decorations = [
          ...markers.added.filter((line) => line >= 1 && line <= max).map((line) =>
            Decoration.line({ class: "cm-live-added-line" }).range(view.state.doc.line(line).from)),
          ...markers.modified.filter((line) => line >= 1 && line <= max).map((line) =>
            Decoration.line({ class: "cm-live-modified-line" }).range(view.state.doc.line(line).from)),
        ];
        return Decoration.set(decorations.sort((a, b) => a.from - b.from), true);
      }));
    }
    if (!tooLarge) {
      const lang = languageFor(fileRel);
      if (lang) exts.push(lang);
    }
    return exts;
  }, [fileRel, tooLarge, saveKeymap, readOnly, markers]);

  return (
    <div
      className="h-64 w-full overflow-hidden bg-[#17171D]"
      aria-label={fileRel}
      onScrollCapture={onEditorScroll}
      onWheelCapture={(e) => {
        if (!followLatest) return;
        manualRef.current = true;
        if (e.deltaY < 0) followingRef.current = false;
      }}
      onTouchMoveCapture={() => {
        if (!followLatest) return;
        manualRef.current = true;
        followingRef.current = false;
      }}
      onPointerDownCapture={(e) => {
        if (!followLatest) return;
        const el = viewRef.current?.scrollDOM;
        if (el && e.clientX >= el.getBoundingClientRect().right - 22) {
          manualRef.current = true;
          followingRef.current = false;
        }
      }}
      onKeyDownCapture={(e) => {
        if (!followLatest || !["ArrowUp", "PageUp", "Home", "ArrowDown", "PageDown", "End"].includes(e.key)) return;
        manualRef.current = true;
        if (["ArrowUp", "PageUp", "Home"].includes(e.key)) followingRef.current = false;
      }}
    >
      <CodeMirror
        value={value}
        height="256px"
        theme={[oneDark, appTheme]}
        extensions={extensions}
        onCreateEditor={(view) => { viewRef.current = view; }}
        onChange={(v) => { if (!readOnly) onChange(v); }}
        basicSetup={{
          foldGutter: true,
          dropCursor: false,
          allowMultipleSelections: true,
        }}
      />
    </div>
  );
}
