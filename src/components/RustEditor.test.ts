// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { createApp, h, nextTick, reactive, type App as VueApp } from "vue";
import cspSource from "../../src-tauri/tauri.conf.json?raw";
import setupSource from "../monaco/setup.ts?raw";
import RustEditor from "./RustEditor.vue";
import editorSource from "./RustEditor.vue?raw";

interface DiagnosticBatch {
  exerciseId: string;
  sourceDigest: string;
  modelVersion: number;
  markers: Array<{
    severity: "error" | "warning" | "info" | "hint";
    message: string;
    code?: string;
    range: {
      startLineNumber: number;
      startColumn: number;
      endLineNumber: number;
      endColumn: number;
    };
  }>;
}

const mock = vi.hoisted(() => ({
  models: [] as Array<{
    exerciseId: string;
    source: string;
    version: number;
    disposed: boolean;
    listeners: Set<() => void>;
    setValue(value: string): void;
  }>,
  editors: [] as Array<{
    disposed: boolean;
    compositionStart: Set<() => void>;
    compositionEnd: Set<() => void>;
  }>,
  commands: [] as Array<{ disposed: boolean; run: () => void }>,
  markerCalls: [] as Array<{ exerciseId: string; owner: string; count: number }>,
}));

vi.mock("../monaco/setup", () => ({
  createModel(source: string, exerciseId: string) {
    const listeners = new Set<() => void>();
    const state = {
      exerciseId,
      source,
      version: 1,
      disposed: false,
      listeners,
      setValue(value: string) {
        state.source = value;
        state.version += 1;
        for (const listener of listeners) listener();
      },
    };
    mock.models.push(state);
    return {
      uri: { toString: () => `rustlings:///exercises/${exerciseId}.rs` },
      getValue: () => state.source,
      setValue: (value: string) => state.setValue(value),
      getVersionId: () => state.version,
      onDidChangeContent(listener: () => void) {
        listeners.add(listener);
        return { dispose: () => listeners.delete(listener) };
      },
      dispose() {
        state.disposed = true;
      },
    };
  },
  createEditor(_host: HTMLElement, _model: unknown, _ariaLabel: string) {
    const compositionStart = new Set<() => void>();
    const compositionEnd = new Set<() => void>();
    const state = { disposed: false, compositionStart, compositionEnd };
    mock.editors.push(state);
    return {
      layout: vi.fn(),
      onDidCompositionStart(listener: () => void) {
        compositionStart.add(listener);
        return { dispose: () => compositionStart.delete(listener) };
      },
      onDidCompositionEnd(listener: () => void) {
        compositionEnd.add(listener);
        return { dispose: () => compositionEnd.delete(listener) };
      },
      dispose() {
        state.disposed = true;
      },
    };
  },
  addRunCommand(_editor: unknown, run: () => void) {
    const command = { disposed: false, run };
    mock.commands.push(command);
    return { dispose: () => (command.disposed = true) };
  },
  setMarkers(model: { uri: { toString(): string } }, owner: string, markers: unknown[]) {
    const exerciseId = model.uri.toString().split("/").at(-1)?.replace(/\.rs$/, "") ?? "";
    mock.markerCalls.push({ exerciseId, owner, count: markers.length });
  },
}));

class ResizeObserverMock {
  static active = 0;
  static observed = 0;
  private readonly callback: ResizeObserverCallback;

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    ResizeObserverMock.active += 1;
  }

  observe() {
    ResizeObserverMock.observed += 1;
    this.callback([], this);
  }

  unobserve() {}

  disconnect() {
    ResizeObserverMock.active -= 1;
  }
}

interface EditorProps {
  exerciseId: string;
  source: string;
  sourceDigest: string;
  diagnostics?: DiagnosticBatch;
}

const mountedApps: VueApp[] = [];

async function settle() {
  await nextTick();
  await new Promise((resolve) => setTimeout(resolve));
  await nextTick();
}

async function mountEditor(initial: EditorProps) {
  const props = reactive(initial);
  const runs: Array<[string, number]> = [];
  const changes: Array<[string, number]> = [];
  const host = document.createElement("div");
  document.body.append(host);
  const app = createApp({
    setup: () => () =>
      h(RustEditor, {
        ...props,
        onRun: (source: string, version: number) => runs.push([source, version]),
        onChange: (source: string, version: number) => changes.push([source, version]),
      }),
  });
  mountedApps.push(app);
  app.mount(host);
  await settle();
  return { app, host, props, runs, changes };
}

beforeEach(() => {
  mock.models.splice(0);
  mock.editors.splice(0);
  mock.commands.splice(0);
  mock.markerCalls.splice(0);
  ResizeObserverMock.active = 0;
  ResizeObserverMock.observed = 0;
  vi.stubGlobal("ResizeObserver", ResizeObserverMock);
});

afterEach(() => {
  for (const app of mountedApps.splice(0)) app.unmount();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

describe("RustEditor", () => {
  it("keeps Monaco, its one local editor worker, and CSP wiring build-safe", () => {
    expect(editorSource).toContain('await import("../monaco/setup")');
    expect(setupSource).toContain("monaco-editor/esm/vs/editor/editor.api.js");
    expect(setupSource).toContain("monaco-editor/esm/vs/basic-languages/rust/rust.contribution.js");
    expect(setupSource).toContain("monaco-editor/min/vs/editor/editor.main.css");
    expect(setupSource).toContain("rustlings:///exercises/${encodeURIComponent(exerciseId)}.rs");
    expect(setupSource.match(/\?worker/g)).toHaveLength(1);
    expect(setupSource).toContain("editor.worker.js?worker");
    expect(setupSource).not.toMatch(/(json|css|html|typescript)\.worker/);
    expect(setupSource).not.toMatch(/blob:|https?:\/\//);

    const config = JSON.parse(cspSource) as {
      app: { security: { csp: Record<string, string[]> } };
    };
    expect(config.app.security.csp["worker-src"]).toEqual(["'self'"]);
    expect(config.app.security.csp["script-src"]).toEqual(["'self'"]);
  });

  it("creates Rust source at a stable exercise URI and guards Command-Enter during IME", async () => {
    const mounted = await mountEditor({
      exerciseId: "intro1",
      source: "fn main() {}\n",
      sourceDigest: "digest-1",
    });

    expect(mock.models).toHaveLength(1);
    expect(mock.models[0]?.source).toBe("fn main() {}\n");
    expect(`rustlings:///exercises/${mock.models[0]?.exerciseId}.rs`).toBe(
      "rustlings:///exercises/intro1.rs",
    );

    mock.commands[0]?.run();
    expect(mounted.runs).toEqual([["fn main() {}\n", 1]]);

    for (const listener of mock.editors[0]?.compositionStart ?? []) listener();
    mock.commands[0]?.run();
    expect(mounted.runs).toHaveLength(1);

    for (const listener of mock.editors[0]?.compositionEnd ?? []) listener();
    mock.commands[0]?.run();
    expect(mounted.runs).toHaveLength(2);

    mounted.props.source = 'fn main() { println!("updated"); }\n';
    await settle();
    expect(mock.models).toHaveLength(1);
    expect(mock.models[0]?.source).toBe('fn main() { println!("updated"); }\n');
  });

  it("clears and disposes every exercise-owned resource on switch and unmount", async () => {
    const mounted = await mountEditor({
      exerciseId: "intro1",
      source: "old",
      sourceDigest: "old-digest",
    });
    const oldModel = mock.models[0]!;
    const oldEditor = mock.editors[0]!;
    const oldCommand = mock.commands[0]!;

    Object.assign(mounted.props, {
      exerciseId: "intro2",
      source: "new",
      sourceDigest: "new-digest",
    });
    await settle();

    expect(oldModel.disposed).toBe(true);
    expect(oldEditor.disposed).toBe(true);
    expect(oldCommand.disposed).toBe(true);
    expect(oldModel.listeners.size).toBe(0);
    expect(mock.markerCalls).toContainEqual({
      exerciseId: "intro1",
      owner: "rustlings-diagnostics:intro1",
      count: 0,
    });
    expect(mock.models[1]?.exerciseId).toBe("intro2");
    expect(ResizeObserverMock.active).toBe(1);

    mounted.app.unmount();
    mountedApps.splice(mountedApps.indexOf(mounted.app), 1);
    expect(mock.models[1]?.disposed).toBe(true);
    expect(mock.editors[1]?.disposed).toBe(true);
    expect(mock.commands[1]?.disposed).toBe(true);
    expect(ResizeObserverMock.active).toBe(0);
  });

  it("returns resource counts to baseline across remounts", async () => {
    for (let cycle = 0; cycle < 2; cycle += 1) {
      const mounted = await mountEditor({
        exerciseId: "variables1",
        source: `source-${cycle}`,
        sourceDigest: `digest-${cycle}`,
      });
      mounted.app.unmount();
      mountedApps.splice(mountedApps.indexOf(mounted.app), 1);
    }

    expect(mock.models.every((model) => model.disposed && model.listeners.size === 0)).toBe(true);
    expect(mock.editors.every((editor) => editor.disposed)).toBe(true);
    expect(mock.commands.every((command) => command.disposed)).toBe(true);
    expect(ResizeObserverMock.active).toBe(0);
  });

  it("applies only active exercise, digest, and model-version diagnostics", async () => {
    const mounted = await mountEditor({
      exerciseId: "intro1",
      source: "broken",
      sourceDigest: "current-digest",
    });
    const marker = {
      severity: "error" as const,
      message: "expected semicolon",
      range: {
        startLineNumber: 1,
        startColumn: 1,
        endLineNumber: 1,
        endColumn: 2,
      },
    };

    mounted.props.diagnostics = {
      exerciseId: "intro2",
      sourceDigest: "current-digest",
      modelVersion: 1,
      markers: [marker],
    };
    await settle();
    expect(mock.markerCalls.at(-1)?.count).toBe(0);

    mounted.props.diagnostics = {
      exerciseId: "intro1",
      sourceDigest: "stale-digest",
      modelVersion: 1,
      markers: [marker],
    };
    await settle();
    expect(mock.markerCalls.at(-1)?.count).toBe(0);

    mounted.props.diagnostics = {
      exerciseId: "intro1",
      sourceDigest: "current-digest",
      modelVersion: 1,
      markers: [marker],
    };
    await settle();
    expect(mock.markerCalls.at(-1)?.count).toBe(1);

    mock.models[0]?.setValue("edited");
    await settle();
    expect(mock.markerCalls.at(-1)?.count).toBe(0);

    mounted.props.diagnostics = {
      exerciseId: "intro1",
      sourceDigest: "current-digest",
      modelVersion: 1,
      markers: [marker],
    };
    await settle();
    expect(mock.markerCalls.at(-1)?.count).toBe(0);
  });

  it("keeps the editor keyboard reachable without trapping Escape", async () => {
    const mounted = await mountEditor({
      exerciseId: "intro1",
      source: "fn main() {}",
      sourceDigest: "digest",
    });
    const editorRegion = mounted.host.querySelector<HTMLElement>(
      "[aria-label='Rust source editor']",
    );
    expect(editorRegion).not.toBeNull();
    expect(ResizeObserverMock.observed).toBe(1);

    let escaped = false;
    mounted.host.addEventListener("keydown", (event) => {
      if (event.key === "Escape") escaped = true;
    });
    editorRegion?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(escaped).toBe(true);
  });
});
