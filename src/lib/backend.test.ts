import { beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { backend } from "./backend";

const mock = vi.hoisted(() => ({ invoke: vi.fn(async () => undefined) }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mock.invoke }));

beforeEach(() => mock.invoke.mockClear());

describe("learning backend command contract", () => {
  it("uses the Rust command names and camelCase top-level arguments exactly", async () => {
    await backend.sessionSnapshot();
    await backend.retryPreflight();
    await backend.saveSource({ exerciseId: "intro1", expectedRevision: 4, source: "fn main() {}" });
    await backend.selectExercise({ exerciseId: "intro1" });
    await backend.revealHint({ exerciseId: "intro1" });
    await backend.runExercise({ exerciseId: "intro1" });
    await backend.runResult({ runId: "run-4" });
    await backend.cancelRun({ runId: "run-4" });

    expect(mock.invoke.mock.calls).toEqual([
      ["session_snapshot"],
      ["retry_preflight"],
      ["save_source", { exerciseId: "intro1", expectedRevision: 4, source: "fn main() {}" }],
      ["select_exercise", { exerciseId: "intro1" }],
      ["reveal_hint", { exerciseId: "intro1" }],
      ["run_exercise", { exerciseId: "intro1" }],
      ["run_result", { runId: "run-4" }],
      ["cancel_run", { runId: "run-4" }],
    ]);
  });
});
