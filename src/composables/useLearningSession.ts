import { computed, ref, shallowRef } from "vue";
import { backend as tauriBackend, type LearningBackend } from "../lib/backend";
import type { RustMarker } from "../monaco/setup";
import type {
  ActiveRunSnapshot,
  RunResponse,
  RunTarget,
  SessionSnapshot,
  SolutionResponse,
} from "../types/learning";

const DEFAULT_SAVE_DEBOUNCE_MS = 500;
const DEFAULT_DISPLAY_LIMIT = 128 * 1024;

export interface DiagnosticBatch {
  modelId: string;
  sourceDigest: string;
  modelVersion: number;
  markers: RustMarker[];
}

interface RunContext {
  target: RunTarget;
  source: string;
  sourceDigest: string;
  editIntent: number;
  modelVersion: number;
  modelId: string;
}

interface LearningSessionOptions {
  saveDebounceMs?: number;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function sanitizeDisplayText(value: string, maximumLength = DEFAULT_DISPLAY_LIMIT): string {
  const limit = Math.max(1, maximumLength);
  let plain = "";
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code === 0x1b) {
      const kind = value.charCodeAt(index + 1);
      if (kind === 0x5b) {
        index += 2;
        while (index < value.length && value.charCodeAt(index) < 0x40) index += 1;
      } else if (kind === 0x5d) {
        index += 2;
        while (index < value.length) {
          const current = value.charCodeAt(index);
          if (current === 0x07) break;
          if (current === 0x1b && value.charCodeAt(index + 1) === 0x5c) {
            index += 1;
            break;
          }
          index += 1;
        }
      } else {
        index += 1;
      }
      continue;
    }
    if ((code < 0x20 && code !== 0x09 && code !== 0x0a) || code === 0x7f) continue;
    const character = String.fromCodePoint(value.codePointAt(index) ?? code);
    plain += character;
    index += character.length - 1;
    if (plain.length >= limit) {
      if (index + 1 >= value.length && plain.length <= limit) return plain;
      let end = limit - 1;
      if (end > 0 && /[\uD800-\uDBFF]/.test(plain[end - 1] ?? "")) end -= 1;
      return `${plain.slice(0, end)}…`;
    }
  }
  return plain;
}

export function useLearningSession(
  backend: LearningBackend = tauriBackend,
  options: LearningSessionOptions = {},
) {
  const saveDebounceMs = options.saveDebounceMs ?? DEFAULT_SAVE_DEBOUNCE_MS;
  const snapshot = shallowRef<SessionSnapshot>();
  const source = ref("");
  const modelVersion = ref(1);
  const hint = ref<string>();
  const solution = shallowRef<SolutionResponse>();
  const learnerRunResult = shallowRef<RunResponse>();
  const solutionRunResult = shallowRef<RunResponse>();
  const learnerDiagnostics = shallowRef<DiagnosticBatch>();
  const solutionDiagnostics = shallowRef<DiagnosticBatch>();
  const activeRun = shallowRef<ActiveRunSnapshot>();
  const startingRun = ref(false);
  const navigating = ref(false);
  const loading = ref(true);
  const retryingPreflight = ref(false);
  const saveError = ref<string>();
  const error = ref<string>();
  const cancelling = ref(false);

  let editIntent = 0;
  let savedIntent = 0;
  let queuedIntent = 0;
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  let saveTail: Promise<void> = Promise.resolve();
  let lastSave: Promise<void> = Promise.resolve();
  let selectionTail: Promise<void> = Promise.resolve();
  let navigationGeneration = 0;

  const dirty = ref(false);
  const saving = ref(false);
  const running = computed(() => startingRun.value || activeRun.value !== undefined);
  const canCancel = computed(() => activeRun.value !== undefined);
  const viewingSolution = computed(() => solution.value !== undefined);
  const viewPath = computed(
    () =>
      solution.value?.path ??
      snapshot.value?.exercises.find((item) => item.id === snapshot.value?.selected)?.sourcePath ??
      "",
  );
  const viewSource = computed(() => solution.value?.source ?? source.value);
  const viewSourceDigest = computed(
    () => solution.value?.sourceDigest ?? snapshot.value?.sourceDigest ?? "",
  );
  const viewReadme = computed(() => solution.value?.readme ?? snapshot.value?.readme ?? "");
  const viewModelId = computed(() =>
    solution.value
      ? `solution:${solution.value.path}:${solution.value.sourceDigest}`
      : `learner:${snapshot.value?.selected ?? ""}`,
  );
  const runResult = computed(() => {
    if (!solution.value) return learnerRunResult.value;
    if (startingRun.value || activeRun.value?.target.kind === "solution") {
      return solutionRunResult.value;
    }
    return solutionRunResult.value ?? learnerRunResult.value;
  });
  const diagnostics = computed(() =>
    solution.value ? solutionDiagnostics.value : learnerDiagnostics.value,
  );

  function replaceFromSnapshot(next: SessionSnapshot) {
    snapshot.value = next;
    activeRun.value = next.activeRun ?? undefined;
    source.value = next.source;
    modelVersion.value = 1;
    editIntent = 0;
    savedIntent = 0;
    queuedIntent = 0;
    dirty.value = false;
    hint.value = undefined;
    solution.value = undefined;
    learnerDiagnostics.value = undefined;
    solutionDiagnostics.value = undefined;
    solutionRunResult.value = undefined;
  }

  function updateSnapshot(next: SessionSnapshot, savedThroughIntent: number) {
    const selectionChanged = snapshot.value?.selected !== next.selected;
    if (selectionChanged && editIntent > savedThroughIntent) {
      if (next.activeRun) {
        snapshot.value = { ...snapshot.value!, activeRun: next.activeRun };
        activeRun.value = next.activeRun;
      }
      return;
    }
    snapshot.value = next;
    if (selectionChanged) {
      source.value = next.source;
      modelVersion.value = 1;
      editIntent = 0;
      savedIntent = 0;
      queuedIntent = 0;
      dirty.value = false;
      hint.value = undefined;
      solution.value = undefined;
      learnerDiagnostics.value = undefined;
      solutionDiagnostics.value = undefined;
      solutionRunResult.value = undefined;
    } else if (editIntent <= savedThroughIntent) {
      source.value = next.source;
    }
    if (next.activeRun) activeRun.value = next.activeRun;
    if (
      solution.value &&
      !next.exercises.find((item) => item.id === solution.value?.exerciseId)?.solutionAvailable
    ) {
      solution.value = undefined;
      solutionRunResult.value = undefined;
      solutionDiagnostics.value = undefined;
    }
  }

  async function initialize(): Promise<boolean> {
    loading.value = true;
    error.value = undefined;
    try {
      replaceFromSnapshot(await backend.sessionSnapshot());
      const restored = activeRun.value;
      if (restored) {
        void awaitRunResult(restored.runId, {
          target: restored.target,
          source: restored.target.kind === "learner" ? source.value : "",
          sourceDigest: restored.sourceDigest,
          editIntent,
          modelVersion: modelVersion.value,
          modelId: restored.target.kind === "learner" ? viewModelId.value : "",
        });
      }
      return true;
    } catch (caught) {
      error.value = errorMessage(caught);
      return false;
    } finally {
      loading.value = false;
    }
  }

  function expectedRevision(exerciseId: string): number {
    const exercise = snapshot.value?.exercises.find((item) => item.id === exerciseId);
    if (!exercise) throw new Error(`exercise is absent from session: ${exerciseId}`);
    return exercise.revision;
  }

  function queueSave(intent: number, exerciseId: string, value: string, retry: boolean) {
    queuedIntent = Math.max(queuedIntent, intent);
    const task = saveTail.then(async () => {
      if (saveError.value && !retry) throw new Error(saveError.value);
      if (retry) saveError.value = undefined;
      saving.value = true;
      try {
        const response = await backend.saveSource({
          exerciseId,
          expectedRevision: expectedRevision(exerciseId),
          source: value,
        });
        savedIntent = Math.max(savedIntent, intent);
        dirty.value = editIntent > savedIntent;
        updateSnapshot(response.snapshot, intent);
      } catch (caught) {
        saveError.value = errorMessage(caught);
        throw caught;
      } finally {
        saving.value = false;
      }
    });
    saveTail = task.catch(() => undefined);
    lastSave = task;
    return task;
  }

  function editSource(value: string, version: number) {
    source.value = value;
    modelVersion.value = version;
    editIntent += 1;
    dirty.value = true;
    learnerDiagnostics.value = undefined;
    if (saveTimer) clearTimeout(saveTimer);
    const intent = editIntent;
    const exerciseId = snapshot.value?.selected;
    saveTimer = setTimeout(() => {
      saveTimer = undefined;
      if (exerciseId && intent > savedIntent) {
        void queueSave(intent, exerciseId, value, false).catch(() => undefined);
      }
    }, saveDebounceMs);
  }

  async function flushSaves(): Promise<boolean> {
    const exerciseId = snapshot.value?.selected;
    if (!exerciseId) return false;
    while (true) {
      if (saveTimer) {
        clearTimeout(saveTimer);
        saveTimer = undefined;
      }
      if (editIntent > savedIntent && (editIntent > queuedIntent || saveError.value)) {
        lastSave = queueSave(editIntent, exerciseId, source.value, true);
      }
      try {
        await lastSave;
      } catch {
        return false;
      }
      if (editIntent <= savedIntent) return !saveError.value;
    }
  }

  function learnerResultMatches(next: SessionSnapshot | undefined) {
    const result = learnerRunResult.value;
    if (!next || !result || result.stale || result.target.kind !== "learner") return false;
    const validation = [...result.finalRecheck, result.validation].find(
      (item) => item.exercise_id === next.selected,
    );
    return (
      result.target.exerciseId === next.selected && validation?.source_digest === next.sourceDigest
    );
  }

  function queueSelection(generation: number, exerciseId: string) {
    const task = selectionTail.then(() =>
      generation === navigationGeneration ? backend.selectExercise({ exerciseId }) : undefined,
    );
    selectionTail = task.then(
      () => undefined,
      () => undefined,
    );
    return task;
  }

  async function selectExercise(exerciseId: string): Promise<boolean> {
    error.value = undefined;
    const exercise = snapshot.value?.exercises.find((item) => item.id === exerciseId);
    if (!exercise || exercise.status === "locked" || running.value) return false;
    const generation = ++navigationGeneration;
    navigating.value = true;
    try {
      if (!(await flushSaves()) || generation !== navigationGeneration) return false;
      const requestedAtIntent = editIntent;
      const next = await queueSelection(generation, exerciseId);
      if (!next || generation !== navigationGeneration) return false;
      if (editIntent > requestedAtIntent) {
        if (!(await flushSaves()) || generation !== navigationGeneration) return false;
        const selected = snapshot.value?.selected === exerciseId;
        if (selected && !learnerResultMatches(snapshot.value)) learnerRunResult.value = undefined;
        return selected;
      }
      const retainLearnerResult = learnerResultMatches(next);
      replaceFromSnapshot(next);
      if (!retainLearnerResult) learnerRunResult.value = undefined;
      return true;
    } catch (caught) {
      if (generation === navigationGeneration) error.value = errorMessage(caught);
      return false;
    } finally {
      if (generation === navigationGeneration) navigating.value = false;
    }
  }

  async function revealHint(): Promise<boolean> {
    const exerciseId = snapshot.value?.selected;
    if (!exerciseId) return false;
    try {
      const response = await backend.revealHint({ exerciseId });
      if (snapshot.value?.selected === response.exerciseId) hint.value = response.hint;
      return true;
    } catch (caught) {
      error.value = errorMessage(caught);
      return false;
    }
  }

  async function revealSolution(exerciseId = snapshot.value?.selected): Promise<boolean> {
    const exercise = snapshot.value?.exercises.find((item) => item.id === exerciseId);
    if (!exerciseId || !exercise?.solutionAvailable || running.value) return false;
    if (solution.value?.exerciseId === exerciseId && solution.value.path === exercise.solutionPath)
      return true;
    const observed = navigationGeneration;
    await selectionTail;
    if (observed !== navigationGeneration) return false;
    const generation = ++navigationGeneration;
    navigating.value = true;
    error.value = undefined;
    try {
      if (!(await flushSaves()) || generation !== navigationGeneration) return false;
      const available = snapshot.value?.exercises.find((item) => item.id === exerciseId);
      if (!available?.solutionAvailable) return false;
      const requestedAtIntent = editIntent;
      const response = await backend.revealSolution({ exerciseId });
      if (
        generation !== navigationGeneration ||
        response.exerciseId !== exerciseId ||
        response.path !== available.solutionPath ||
        !snapshot.value?.exercises.find((item) => item.id === exerciseId)?.solutionAvailable ||
        editIntent !== requestedAtIntent ||
        dirty.value
      )
        return false;
      solution.value = response;
      solutionRunResult.value = undefined;
      solutionDiagnostics.value = undefined;
      return true;
    } catch (caught) {
      if (generation === navigationGeneration) error.value = errorMessage(caught);
      return false;
    } finally {
      if (generation === navigationGeneration) navigating.value = false;
    }
  }

  function sameTarget(left: RunTarget, right: RunTarget) {
    return (
      left.kind === right.kind &&
      left.exerciseId === right.exerciseId &&
      (left.kind === "learner" || (right.kind === "solution" && left.path === right.path))
    );
  }

  function markerBatch(response: RunResponse, context: RunContext): DiagnosticBatch | undefined {
    if (response.stale || !sameTarget(response.target, context.target)) return;
    const validation =
      context.target.kind === "solution"
        ? response.validation
        : [...response.finalRecheck, response.validation].find(
            (result) => result.exercise_id === context.target.exerciseId,
          );
    if (
      !validation ||
      validation.exercise_id !== context.target.exerciseId ||
      validation.source_digest !== context.sourceDigest ||
      viewModelId.value !== context.modelId ||
      (context.target.kind === "learner" && dirty.value)
    )
      return;
    const markers = validation.diagnostics.flatMap((diagnostic) => {
      if (!diagnostic.range || diagnostic.source_digest !== validation.source_digest) return [];
      return [
        {
          severity: diagnostic.severity,
          message: sanitizeDisplayText(diagnostic.message, 8_192),
          code: diagnostic.code ? sanitizeDisplayText(diagnostic.code, 1_024) : undefined,
          range: {
            startLineNumber: diagnostic.range.start_line_number,
            startColumn: diagnostic.range.start_column,
            endLineNumber: diagnostic.range.end_line_number,
            endColumn: diagnostic.range.end_column,
          },
        },
      ];
    });
    return {
      modelId: context.modelId,
      sourceDigest: validation.source_digest,
      modelVersion: context.modelVersion,
      markers,
    };
  }

  async function awaitRunResult(runId: string, context: RunContext): Promise<boolean> {
    let completedActiveRun = false;
    try {
      const response = await backend.runResult({ runId });
      if (activeRun.value?.runId !== runId) return false;
      completedActiveRun = true;
      if (response.runId !== runId || !sameTarget(response.target, context.target)) {
        error.value = "backend returned a mismatched run result";
        return false;
      }
      const solutionMatches =
        context.target.kind === "solution" &&
        solution.value?.exerciseId === context.target.exerciseId &&
        solution.value.path === context.target.path &&
        solution.value.sourceDigest === context.sourceDigest &&
        solution.value.source === context.source;
      const locallyStale =
        context.target.kind === "learner"
          ? source.value !== context.source || snapshot.value?.sourceDigest !== context.sourceDigest
          : !solutionMatches;
      const visibleResponse = locallyStale ? { ...response, stale: true } : response;
      updateSnapshot(response.snapshot, context.editIntent);
      if (context.target.kind === "learner") {
        learnerRunResult.value = visibleResponse;
        learnerDiagnostics.value = markerBatch(visibleResponse, context);
      } else if (solutionMatches) {
        solutionRunResult.value = visibleResponse;
        solutionDiagnostics.value = markerBatch(visibleResponse, context);
      }
      return true;
    } catch (caught) {
      if (activeRun.value?.runId === runId) error.value = errorMessage(caught);
      return false;
    } finally {
      if (activeRun.value?.runId === runId) {
        activeRun.value = undefined;
        cancelling.value = false;
      } else if (completedActiveRun) {
        cancelling.value = false;
      }
    }
  }

  async function run(
    value = viewSource.value,
    version = solution.value ? 1 : modelVersion.value,
  ): Promise<boolean> {
    if (running.value || navigating.value || !snapshot.value?.preflight.ready) return false;
    startingRun.value = true;
    const currentSolution = solution.value;
    if (!currentSolution && value !== source.value) editSource(value, version);
    if (!currentSolution && !(await flushSaves())) {
      startingRun.value = false;
      return false;
    }
    const target: RunTarget = currentSolution
      ? {
          kind: "solution",
          exerciseId: currentSolution.exerciseId,
          path: currentSolution.path,
        }
      : { kind: "learner", exerciseId: snapshot.value.selected };
    const context: RunContext = {
      target,
      source: currentSolution?.source ?? source.value,
      sourceDigest: currentSolution?.sourceDigest ?? snapshot.value.sourceDigest,
      editIntent,
      modelVersion: currentSolution ? version : modelVersion.value,
      modelId: viewModelId.value,
    };
    error.value = undefined;
    if (target.kind === "solution") {
      solutionRunResult.value = undefined;
      solutionDiagnostics.value = undefined;
    } else {
      learnerRunResult.value = undefined;
      learnerDiagnostics.value = undefined;
    }
    try {
      const ticket =
        target.kind === "solution"
          ? await backend.runSolution({ exerciseId: target.exerciseId })
          : await backend.runExercise({ exerciseId: target.exerciseId });
      if (!sameTarget(ticket.target, target) || ticket.sourceDigest !== context.sourceDigest)
        throw new Error("backend returned a mismatched run ticket");
      activeRun.value = {
        runId: ticket.runId,
        target: ticket.target,
        sourceDigest: ticket.sourceDigest,
      };
      startingRun.value = false;
      return await awaitRunResult(ticket.runId, context);
    } catch (caught) {
      error.value = errorMessage(caught);
      return false;
    } finally {
      startingRun.value = false;
    }
  }

  async function cancel(): Promise<boolean> {
    const runId = activeRun.value?.runId;
    if (!runId || cancelling.value) return false;
    cancelling.value = true;
    try {
      const result = await backend.cancelRun({ runId });
      if (activeRun.value?.runId !== runId) return false;
      return result === "requested";
    } catch (caught) {
      error.value = errorMessage(caught);
      return false;
    } finally {
      if (activeRun.value?.runId === runId) cancelling.value = false;
    }
  }

  async function retryPreflight(): Promise<boolean> {
    if (!snapshot.value || retryingPreflight.value) return false;
    retryingPreflight.value = true;
    error.value = undefined;
    try {
      snapshot.value = { ...snapshot.value, preflight: await backend.retryPreflight() };
      return snapshot.value.preflight.ready;
    } catch (caught) {
      error.value = errorMessage(caught);
      return false;
    } finally {
      retryingPreflight.value = false;
    }
  }

  return {
    snapshot,
    source,
    modelVersion,
    hint,
    solution,
    viewingSolution,
    viewPath,
    viewSource,
    viewSourceDigest,
    viewReadme,
    viewModelId,
    runResult,
    diagnostics,
    activeRun,
    navigating,
    loading,
    retryingPreflight,
    saving,
    dirty,
    flushSaves,
    running,
    canCancel,
    cancelling,
    saveError,
    error,
    initialize,
    editSource,
    selectExercise,
    revealHint,
    revealSolution,
    run,
    cancel,
    retryPreflight,
  };
}
