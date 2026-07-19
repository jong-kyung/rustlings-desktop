// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import type { LearningBackend } from "../lib/backend";
import type {
  CancelRunResult,
  RunResponse,
  RunTicket,
  SaveSourceResponse,
  SessionSnapshot,
  SolutionResponse,
  ValidationResult,
} from "../types/learning";
import { sanitizeDisplayText, useLearningSession } from "./useLearningSession";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function snapshot(overrides: Partial<SessionSnapshot> = {}): SessionSnapshot {
  return {
    selected: "intro1",
    source: "original",
    sourceDigest: "digest-0",
    readme: "Read <b>carefully</b>: javascript:alert(1)",
    exercises: [
      { id: "intro1", status: "current", revision: 0 },
      { id: "intro2", status: "locked", revision: 0 },
    ],
    activeRunId: null,
    curriculumComplete: false,
    solutionAvailable: false,
    preflight: { ready: true, message: null, rustcVersion: "1.88.0" },
    ...overrides,
  };
}

function validation(sourceDigest = "digest-1"): ValidationResult {
  return {
    exercise_id: "intro1",
    source_digest: sourceDigest,
    outcome: { status: "learner_failure", stage: "build" },
    stages: [
      {
        stage: "build",
        success: false,
        stdout: "compiler output",
        stderr: "",
        output_truncated: false,
      },
    ],
    diagnostics: [
      {
        stage: "build",
        severity: "error",
        message: "expected semicolon",
        code: null,
        range: {
          start_line_number: 1,
          start_column: 1,
          end_line_number: 1,
          end_column: 2,
        },
        source_digest: sourceDigest,
      },
    ],
  };
}

class FakeBackend implements LearningBackend {
  current = snapshot();
  saveCalls: Array<{ exerciseId: string; expectedRevision: number; source: string }> = [];
  selectCalls: string[] = [];
  runCalls: string[] = [];
  resultCalls: string[] = [];
  cancelCalls: string[] = [];
  solutionCalls: string[] = [];
  saves: Array<ReturnType<typeof deferred<SaveSourceResponse>>> = [];
  runTicket: RunTicket = {
    runId: "run-1",
    exerciseId: "intro1",
    revision: 0,
    sourceDigest: "digest-0",
  };
  runStart?: ReturnType<typeof deferred<RunTicket>>;
  selectResult?: ReturnType<typeof deferred<SessionSnapshot>>;
  result = deferred<RunResponse>();
  cancelResult?: ReturnType<typeof deferred<CancelRunResult>>;
  solutionResult?: ReturnType<typeof deferred<SolutionResponse>>;

  async sessionSnapshot() {
    return this.current;
  }

  async retryPreflight() {
    return this.current.preflight;
  }

  saveSource(input: { exerciseId: string; expectedRevision: number; source: string }) {
    this.saveCalls.push(input);
    const pending = deferred<SaveSourceResponse>();
    this.saves.push(pending);
    return pending.promise;
  }

  async selectExercise(input: { exerciseId: string }) {
    this.selectCalls.push(input.exerciseId);
    this.current = snapshot({
      selected: input.exerciseId,
      source: `${input.exerciseId} source`,
      sourceDigest: `${input.exerciseId} digest`,
      exercises: [
        { id: "intro1", status: "completed", revision: 1 },
        { id: "intro2", status: "current", revision: 0 },
      ],
    });
    return this.selectResult ? await this.selectResult.promise : this.current;
  }

  async revealHint(input: { exerciseId: string }) {
    return { exerciseId: input.exerciseId, hint: "official hint" };
  }

  async revealSolution(input: { exerciseId: string }) {
    this.solutionCalls.push(input.exerciseId);
    return this.solutionResult
      ? await this.solutionResult.promise
      : { exerciseId: input.exerciseId, solution: "official solution" };
  }

  async runExercise(input: { exerciseId: string }) {
    this.runCalls.push(input.exerciseId);
    this.runTicket = {
      ...this.runTicket,
      exerciseId: input.exerciseId,
      revision: this.current.exercises.find((exercise) => exercise.id === input.exerciseId)!
        .revision,
      sourceDigest: this.current.sourceDigest,
    };
    return this.runStart ? await this.runStart.promise : this.runTicket;
  }

  runResult(input: { runId: string }) {
    this.resultCalls.push(input.runId);
    return this.result.promise;
  }

  async cancelRun(input: { runId: string }) {
    this.cancelCalls.push(input.runId);
    return this.cancelResult ? await this.cancelResult.promise : ("requested" as const);
  }
}

async function tick() {
  for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
}

function saved(backend: FakeBackend, revision: number, source: string): SaveSourceResponse {
  const next = snapshot({
    source,
    sourceDigest: `digest-${revision}`,
    exercises: [
      { id: "intro1", status: "current", revision },
      { id: "intro2", status: "locked", revision: 0 },
    ],
  });
  backend.current = next;
  return { revision, sourceDigest: `digest-${revision}`, snapshot: next };
}

afterEach(() => {
  vi.useRealTimers();
});

describe("useLearningSession", () => {
  it("serializes autosaves and flushes the newest edit before Run", async () => {
    vi.useFakeTimers();
    const backend = new FakeBackend();
    const session = useLearningSession(backend, { saveDebounceMs: 20 });
    await session.initialize();

    session.editSource("edit A", 2);
    expect(session.dirty.value).toBe(true);
    await vi.advanceTimersByTimeAsync(20);
    expect(backend.saveCalls).toEqual([
      { exerciseId: "intro1", expectedRevision: 0, source: "edit A" },
    ]);

    session.editSource("edit B", 3);
    const running = session.run();
    await tick();
    expect(backend.saveCalls).toHaveLength(1);
    expect(backend.runCalls).toHaveLength(0);

    backend.saves[0]!.resolve(saved(backend, 1, "edit A"));
    await tick();
    expect(backend.saveCalls[1]).toEqual({
      exerciseId: "intro1",
      expectedRevision: 1,
      source: "edit B",
    });
    expect(session.source.value).toBe("edit B");

    backend.saves[1]!.resolve(saved(backend, 2, "edit B"));
    await tick();
    expect(session.dirty.value).toBe(false);
    expect(backend.runCalls).toEqual(["intro1"]);
    expect(backend.runTicket.revision).toBe(2);

    backend.result.resolve({
      runId: "run-1",
      revision: 2,
      stale: false,
      validation: validation("digest-2"),
      finalRecheck: [],
      snapshot: backend.current,
    });
    await running;
  });

  it("drains edits made during the Run flush and marks edits made while starting stale", async () => {
    const backend = new FakeBackend();
    backend.runStart = deferred<RunTicket>();
    const session = useLearningSession(backend, { saveDebounceMs: 60_000 });
    await session.initialize();

    session.editSource("edit A", 2);
    const running = session.run();
    await tick();
    expect(backend.saveCalls.map((call) => call.source)).toEqual(["edit A"]);

    session.editSource("edit B", 3);
    backend.saves[0]!.resolve(saved(backend, 1, "edit A"));
    await tick();
    expect(backend.saveCalls.map((call) => call.source)).toEqual(["edit A", "edit B"]);
    expect(backend.runCalls).toHaveLength(0);

    backend.saves[1]!.resolve(saved(backend, 2, "edit B"));
    await tick();
    expect(backend.runCalls).toEqual(["intro1"]);
    expect(backend.runTicket.revision).toBe(2);

    session.editSource("edit C", 4);
    backend.runStart.resolve(backend.runTicket);
    await tick();
    backend.result.resolve({
      runId: "run-1",
      revision: 2,
      stale: false,
      validation: validation("digest-2"),
      finalRecheck: [],
      snapshot: backend.current,
    });
    await expect(running).resolves.toBe(true);

    expect(session.runResult.value?.stale).toBe(true);
    expect(session.source.value).toBe("edit C");
  });

  it("preserves and saves a dirty edit when a passing run advances the backend selection", async () => {
    vi.useFakeTimers();
    const backend = new FakeBackend();
    const session = useLearningSession(backend, { saveDebounceMs: 20 });
    await session.initialize();

    const running = session.run();
    await tick();
    session.editSource("late edit", 2);
    const advanced = snapshot({
      selected: "intro2",
      source: "next exercise",
      sourceDigest: "digest-next",
      exercises: [
        { id: "intro1", status: "completed", revision: 0 },
        { id: "intro2", status: "current", revision: 0 },
      ],
    });
    backend.current = advanced;
    backend.result.resolve({
      runId: "run-1",
      revision: 0,
      stale: false,
      validation: validation("digest-0"),
      finalRecheck: [],
      snapshot: advanced,
    });
    await expect(running).resolves.toBe(true);

    expect(session.snapshot.value?.selected).toBe("intro1");
    expect(session.source.value).toBe("late edit");
    expect(session.dirty.value).toBe(true);

    await vi.advanceTimersByTimeAsync(20);
    expect(backend.saveCalls).toEqual([
      { exerciseId: "intro1", expectedRevision: 0, source: "late edit" },
    ]);
    backend.saves[0]!.resolve(saved(backend, 1, "late edit"));
    await tick();
    expect(session.source.value).toBe("late edit");
    expect(session.dirty.value).toBe(false);
  });

  it("blocks navigation and Run when the required save fails", async () => {
    const backend = new FakeBackend();
    backend.current = snapshot({
      exercises: [
        { id: "intro1", status: "current", revision: 0 },
        { id: "intro2", status: "unlocked", revision: 0 },
      ],
    });
    const session = useLearningSession(backend, { saveDebounceMs: 60_000 });
    await session.initialize();
    session.editSource("cannot save", 2);

    const navigating = session.selectExercise("intro2");
    await tick();
    await expect(session.run()).resolves.toBe(false);
    expect(backend.runCalls).toHaveLength(0);
    backend.saves[0]!.reject(new Error("disk full"));
    await expect(navigating).resolves.toBe(false);
    expect(backend.selectCalls).toHaveLength(0);
    expect(session.saveError.value).toContain("disk full");

    const running = session.run();
    await tick();
    backend.saves[1]!.reject(new Error("still full"));
    await expect(running).resolves.toBe(false);
    expect(backend.runCalls).toHaveLength(0);
  });

  it("flushes edits made while a navigation request is pending", async () => {
    const backend = new FakeBackend();
    backend.current = snapshot({
      exercises: [
        { id: "intro1", status: "current", revision: 1 },
        { id: "intro2", status: "completed", revision: 0 },
      ],
    });
    backend.selectResult = deferred<SessionSnapshot>();
    const session = useLearningSession(backend, { saveDebounceMs: 60_000 });
    await session.initialize();

    const navigating = session.selectExercise("intro2");
    await tick();
    session.editSource("late navigation edit", 2);
    backend.selectResult.resolve(backend.current);
    await tick();

    expect(backend.saveCalls).toEqual([
      { exerciseId: "intro1", expectedRevision: 1, source: "late navigation edit" },
    ]);
    backend.saves[0]!.resolve(saved(backend, 2, "late navigation edit"));
    await expect(navigating).resolves.toBe(false);
    expect(session.snapshot.value?.selected).toBe("intro1");
    expect(session.source.value).toBe("late navigation edit");
    expect(session.dirty.value).toBe(false);
  });

  it("enables cancellation only after Run returns an active run ID", async () => {
    const backend = new FakeBackend();
    backend.runStart = deferred<RunTicket>();
    const session = useLearningSession(backend);
    await session.initialize();

    const running = session.run();
    await tick();
    expect(session.running.value).toBe(true);
    expect(session.canCancel.value).toBe(false);
    await expect(session.cancel()).resolves.toBe(false);
    expect(backend.cancelCalls).toEqual([]);

    backend.runStart.resolve(backend.runTicket);
    await tick();
    expect(session.canCancel.value).toBe(true);
    await expect(session.cancel()).resolves.toBe(true);
    expect(backend.cancelCalls).toEqual(["run-1"]);

    backend.result.resolve({
      runId: "run-1",
      revision: 0,
      stale: false,
      validation: validation("digest-0"),
      finalRecheck: [],
      snapshot: backend.current,
    });
    await running;
  });

  it("clears cancelling when the run completes before a pending cancel request", async () => {
    const backend = new FakeBackend();
    backend.cancelResult = deferred<CancelRunResult>();
    const session = useLearningSession(backend);
    await session.initialize();

    const running = session.run();
    await tick();
    const cancelling = session.cancel();
    await tick();
    expect(session.cancelling.value).toBe(true);
    expect(backend.cancelCalls).toEqual(["run-1"]);

    backend.result.resolve({
      runId: "run-1",
      revision: 0,
      stale: false,
      validation: validation("digest-0"),
      finalRecheck: [],
      snapshot: snapshot({ activeRunId: null }),
    });
    await expect(running).resolves.toBe(true);
    expect(session.cancelling.value).toBe(false);

    backend.cancelResult.resolve("requested");
    await expect(cancelling).resolves.toBe(false);
    expect(session.cancelling.value).toBe(false);
  });

  it("restores and reattaches the active run reported by the session snapshot", async () => {
    const backend = new FakeBackend();
    backend.current = snapshot({ activeRunId: "run-restored" });
    const session = useLearningSession(backend);

    await expect(session.initialize()).resolves.toBe(true);
    expect(backend.resultCalls).toEqual(["run-restored"]);
    expect(session.running.value).toBe(true);
    expect(session.canCancel.value).toBe(true);
    await expect(session.cancel()).resolves.toBe(true);
    expect(backend.cancelCalls).toEqual(["run-restored"]);

    const completed = snapshot({ activeRunId: null });
    backend.result.resolve({
      runId: "run-restored",
      revision: 0,
      stale: false,
      validation: validation("digest-0"),
      finalRecheck: [],
      snapshot: completed,
    });
    await tick();

    expect(session.runResult.value?.runId).toBe("run-restored");
    expect(session.running.value).toBe(false);
    expect(session.canCancel.value).toBe(false);
  });

  it("rejects locked selection, blocks duplicate runs, and cancels only its active run ID", async () => {
    const backend = new FakeBackend();
    const session = useLearningSession(backend);
    await session.initialize();

    await expect(session.selectExercise("intro2")).resolves.toBe(false);
    expect(backend.selectCalls).toHaveLength(0);

    const first = session.run();
    await tick();
    await expect(session.run()).resolves.toBe(false);
    expect(backend.runCalls).toEqual(["intro1"]);

    await expect(session.cancel()).resolves.toBe(true);
    expect(backend.cancelCalls).toEqual(["run-1"]);

    backend.result.resolve({
      runId: "other-run",
      revision: 0,
      stale: false,
      validation: validation("digest-0"),
      finalRecheck: [],
      snapshot: backend.current,
    });
    await first;
    expect(session.runResult.value).toBeUndefined();
  });

  it("marks a result stale after an active-run edit and filters its diagnostics", async () => {
    const backend = new FakeBackend();
    const session = useLearningSession(backend, { saveDebounceMs: 60_000 });
    await session.initialize();

    const running = session.run();
    await tick();
    session.editSource("changed during run", 2);
    backend.result.resolve({
      runId: "run-1",
      revision: 0,
      stale: false,
      validation: validation("digest-0"),
      finalRecheck: [],
      snapshot: backend.current,
    });
    await running;

    expect(session.runResult.value?.stale).toBe(true);
    expect(session.diagnostics.value).toBeUndefined();
    expect(session.source.value).toBe("changed during run");
  });

  it("keeps hint disclosure local to the selected exercise", async () => {
    const backend = new FakeBackend();
    backend.current = snapshot({
      exercises: [
        { id: "intro1", status: "current", revision: 0 },
        { id: "intro2", status: "unlocked", revision: 0 },
      ],
    });
    const session = useLearningSession(backend);
    await session.initialize();

    await session.revealHint();
    expect(session.hint.value).toBe("official hint");
    await session.selectExercise("intro2");
    expect(session.hint.value).toBeUndefined();
  });

  it("reveals a completed solution and clears it after navigation", async () => {
    const backend = new FakeBackend();
    backend.current = snapshot({
      solutionAvailable: true,
      exercises: [
        { id: "intro1", status: "current", revision: 0 },
        { id: "intro2", status: "unlocked", revision: 0 },
      ],
    });
    const session = useLearningSession(backend);
    await session.initialize();

    await expect(session.revealSolution()).resolves.toBe(true);
    await expect(session.revealSolution()).resolves.toBe(true);
    expect(backend.solutionCalls).toEqual(["intro1"]);
    expect(session.solution.value).toBe("official solution");

    await session.selectExercise("intro2");
    expect(session.solution.value).toBeUndefined();
  });

  it("flushes edits before checking solution availability", async () => {
    const backend = new FakeBackend();
    backend.current = snapshot({ solutionAvailable: true });
    const session = useLearningSession(backend, { saveDebounceMs: 60_000 });
    await session.initialize();
    session.editSource("changed after completion", 2);

    const revealing = session.revealSolution();
    await tick();
    expect(session.revealingSolution.value).toBe(true);
    await expect(session.revealSolution()).resolves.toBe(false);
    expect(backend.solutionCalls).toEqual([]);
    backend.saves[0]!.resolve(saved(backend, 1, "changed after completion"));

    await expect(revealing).resolves.toBe(false);
    expect(backend.solutionCalls).toEqual([]);
  });

  it("rejects navigation and discards solution responses after new edits", async () => {
    const backend = new FakeBackend();
    backend.current = snapshot({
      solutionAvailable: true,
      exercises: [
        { id: "intro1", status: "current", revision: 0 },
        { id: "intro2", status: "unlocked", revision: 0 },
      ],
    });
    backend.solutionResult = deferred<SolutionResponse>();
    const session = useLearningSession(backend, { saveDebounceMs: 60_000 });
    await session.initialize();

    const revealing = session.revealSolution();
    await tick();
    await expect(session.selectExercise("intro2")).resolves.toBe(false);
    expect(backend.selectCalls).toEqual([]);
    session.editSource("changed during disclosure", 2);
    backend.solutionResult.resolve({ exerciseId: "intro1", solution: "official solution" });

    await expect(revealing).resolves.toBe(false);
    expect(session.solution.value).toBeUndefined();
  });
});

describe("sanitizeDisplayText", () => {
  it("strips ANSI and controls, bounds text, and leaves hostile markup inert as text", () => {
    const hostile =
      "\u001b[31m<script>javascript:alert(1)</script>\u001b[0m\u0000" + "x".repeat(50);
    expect(sanitizeDisplayText(hostile, 40)).toBe("<script>javascript:alert(1)</script>xxx…");
  });
});
