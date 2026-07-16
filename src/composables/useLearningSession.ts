import { computed, ref, shallowRef } from "vue";
import { backend as tauriBackend, type LearningBackend } from "../lib/backend";
import type { RustMarker } from "../monaco/setup";
import type { RunResponse, RunTicket, SessionSnapshot } from "../types/learning";

const DEFAULT_SAVE_DEBOUNCE_MS = 500;
const DEFAULT_DISPLAY_LIMIT = 128 * 1024;

export interface DiagnosticBatch {
  exerciseId: string;
  sourceDigest: string;
  modelVersion: number;
  markers: RustMarker[];
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
  const runResult = shallowRef<RunResponse>();
  const diagnostics = shallowRef<DiagnosticBatch>();
  const activeTicket = shallowRef<RunTicket>();
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

  const selectedExercise = computed(() =>
    snapshot.value?.exercises.find((exercise) => exercise.id === snapshot.value?.selected),
  );
  const dirty = ref(false);
  const saving = ref(false);
  const running = computed(() => startingRun.value || activeTicket.value !== undefined);

  function replaceFromSnapshot(next: SessionSnapshot) {
    snapshot.value = next;
    source.value = next.source;
    modelVersion.value = 1;
    editIntent = 0;
    savedIntent = 0;
    queuedIntent = 0;
    dirty.value = false;
    hint.value = undefined;
    diagnostics.value = undefined;
  }

  function updateSnapshot(next: SessionSnapshot, savedThroughIntent: number) {
    const selectionChanged = snapshot.value?.selected !== next.selected;
    snapshot.value = next;
    if (selectionChanged) {
      source.value = next.source;
      modelVersion.value = 1;
      editIntent = 0;
      savedIntent = 0;
      queuedIntent = 0;
      dirty.value = false;
      hint.value = undefined;
      diagnostics.value = undefined;
    } else if (editIntent <= savedThroughIntent) {
      source.value = next.source;
    }
  }

  async function initialize(): Promise<boolean> {
    loading.value = true;
    error.value = undefined;
    try {
      replaceFromSnapshot(await backend.sessionSnapshot());
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
    diagnostics.value = undefined;
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
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = undefined;
    }
    const exerciseId = snapshot.value?.selected;
    if (!exerciseId) return false;
    if (editIntent > savedIntent && (editIntent > queuedIntent || saveError.value)) {
      lastSave = queueSave(editIntent, exerciseId, source.value, true);
    }
    try {
      await lastSave;
      return !saveError.value;
    } catch {
      return false;
    }
  }

  async function selectExercise(exerciseId: string): Promise<boolean> {
    error.value = undefined;
    const exercise = snapshot.value?.exercises.find((item) => item.id === exerciseId);
    if (!exercise || exercise.status === "locked" || running.value || navigating.value)
      return false;
    navigating.value = true;
    try {
      if (!(await flushSaves())) return false;
      replaceFromSnapshot(await backend.selectExercise({ exerciseId }));
      runResult.value = undefined;
      return true;
    } catch (caught) {
      error.value = errorMessage(caught);
      return false;
    } finally {
      navigating.value = false;
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

  function markerBatch(response: RunResponse, version: number): DiagnosticBatch | undefined {
    if (response.stale) return;
    const validation = [...response.finalRecheck, response.validation].find(
      (result) => result.exercise_id === snapshot.value?.selected,
    );
    if (!validation || validation.source_digest !== snapshot.value?.sourceDigest || dirty.value)
      return;
    const markers = validation.diagnostics.flatMap((diagnostic) => {
      if (!diagnostic.range || diagnostic.source_digest !== validation.source_digest) {
        return [];
      }
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
      exerciseId: validation.exercise_id,
      sourceDigest: validation.source_digest,
      modelVersion: version,
      markers,
    };
  }

  async function run(value = source.value, version = modelVersion.value): Promise<boolean> {
    if (running.value || navigating.value || !snapshot.value?.preflight.ready) return false;
    startingRun.value = true;
    if (value !== source.value) editSource(value, version);
    if (!(await flushSaves())) {
      startingRun.value = false;
      return false;
    }
    const exerciseId = snapshot.value.selected;
    error.value = undefined;
    runResult.value = undefined;
    diagnostics.value = undefined;
    try {
      const ticket = await backend.runExercise({ exerciseId });
      if (ticket.exerciseId !== exerciseId)
        throw new Error("backend returned a mismatched run ticket");
      activeTicket.value = ticket;
      const runEditIntent = editIntent;
      const runSource = source.value;
      const response = await backend.runResult({ runId: ticket.runId });
      if (response.runId !== ticket.runId) {
        error.value = "backend returned a mismatched run result";
        return false;
      }
      const locallyStale = source.value !== runSource;
      const visibleResponse = locallyStale ? { ...response, stale: true } : response;
      updateSnapshot(response.snapshot, runEditIntent);
      runResult.value = visibleResponse;
      diagnostics.value = markerBatch(visibleResponse, version);
      return true;
    } catch (caught) {
      error.value = errorMessage(caught);
      return false;
    } finally {
      startingRun.value = false;
      activeTicket.value = undefined;
      cancelling.value = false;
    }
  }

  async function cancel(): Promise<boolean> {
    const ticket = activeTicket.value;
    if (!ticket || cancelling.value) return false;
    cancelling.value = true;
    try {
      const result = await backend.cancelRun({ runId: ticket.runId });
      if (activeTicket.value?.runId !== ticket.runId) return false;
      return result === "requested";
    } catch (caught) {
      error.value = errorMessage(caught);
      return false;
    } finally {
      if (activeTicket.value?.runId === ticket.runId) cancelling.value = false;
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
    selectedExercise,
    hint,
    runResult,
    diagnostics,
    activeTicket,
    navigating,
    loading,
    retryingPreflight,
    saving,
    dirty,
    running,
    cancelling,
    saveError,
    error,
    initialize,
    editSource,
    flushSaves,
    selectExercise,
    revealHint,
    run,
    cancel,
    retryPreflight,
  };
}
