import {
  KeyCode,
  KeyMod,
  MarkerSeverity,
  Uri,
  editor,
} from "monaco-editor/esm/vs/editor/editor.api.js";
import "monaco-editor/esm/vs/basic-languages/rust/rust.contribution.js";
import "monaco-editor/min/vs/editor/editor.main.css";
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker.js?worker";

type MonacoGlobal = typeof globalThis & {
  MonacoEnvironment?: {
    getWorker(moduleId: string, label: string): Worker;
  };
};

(globalThis as MonacoGlobal).MonacoEnvironment = {
  getWorker: () => new EditorWorker(),
};

export interface Disposable {
  dispose(): void;
}

export interface RustModel {
  readonly uri: { toString(): string };
  getValue(): string;
  setValue(value: string): void;
  getVersionId(): number;
  onDidChangeContent(listener: () => void): Disposable;
  dispose(): void;
}

export interface RustEditorInstance {
  layout(): void;
  onDidCompositionStart(listener: () => void): Disposable;
  onDidCompositionEnd(listener: () => void): Disposable;
  dispose(): void;
}

export interface RustMarker {
  severity: "error" | "warning" | "info" | "hint";
  message: string;
  code?: string;
  range: {
    startLineNumber: number;
    startColumn: number;
    endLineNumber: number;
    endColumn: number;
  };
}

const markerSeverity = {
  error: MarkerSeverity.Error,
  warning: MarkerSeverity.Warning,
  info: MarkerSeverity.Info,
  hint: MarkerSeverity.Hint,
} as const;

export function createModel(source: string, exerciseId: string): RustModel {
  const uri = Uri.parse(`rustlings:///exercises/${encodeURIComponent(exerciseId)}.rs`);
  return editor.createModel(source, "rust", uri);
}

export function createEditor(
  host: HTMLElement,
  model: RustModel,
  ariaLabel: string,
): RustEditorInstance {
  return editor.create(host, {
    model: model as editor.ITextModel,
    ariaLabel,
    accessibilitySupport: "auto",
    automaticLayout: false,
    minimap: { enabled: false },
    scrollBeyondLastLine: false,
  });
}

export function addRunCommand(editorInstance: RustEditorInstance, run: () => void): Disposable {
  return (editorInstance as editor.IStandaloneCodeEditor).addAction({
    id: "rustlings.run",
    label: "Run exercise",
    keybindings: [KeyMod.CtrlCmd | KeyCode.Enter],
    run,
  });
}

export function setMarkers(model: RustModel, owner: string, markers: readonly RustMarker[]): void {
  editor.setModelMarkers(
    model as editor.ITextModel,
    owner,
    markers.map(({ severity, message, code, range }) => ({
      ...range,
      severity: markerSeverity[severity],
      message,
      code,
    })),
  );
}
