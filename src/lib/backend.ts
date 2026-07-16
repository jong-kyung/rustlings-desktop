import { invoke } from "@tauri-apps/api/core";
import type {
  CancelRunResult,
  HintResponse,
  PreflightSnapshot,
  RunResponse,
  RunTicket,
  SaveSourceResponse,
  SessionSnapshot,
} from "../types/learning";

export interface LearningBackend {
  sessionSnapshot(): Promise<SessionSnapshot>;
  retryPreflight(): Promise<PreflightSnapshot>;
  saveSource(input: {
    exerciseId: string;
    expectedRevision: number;
    source: string;
  }): Promise<SaveSourceResponse>;
  selectExercise(input: { exerciseId: string }): Promise<SessionSnapshot>;
  revealHint(input: { exerciseId: string }): Promise<HintResponse>;
  runExercise(input: { exerciseId: string }): Promise<RunTicket>;
  runResult(input: { runId: string }): Promise<RunResponse>;
  cancelRun(input: { runId: string }): Promise<CancelRunResult>;
}

export const backend: LearningBackend = {
  sessionSnapshot: () => invoke("session_snapshot"),
  retryPreflight: () => invoke("retry_preflight"),
  saveSource: (input) => invoke("save_source", input),
  selectExercise: (input) => invoke("select_exercise", input),
  revealHint: (input) => invoke("reveal_hint", input),
  runExercise: (input) => invoke("run_exercise", input),
  runResult: (input) => invoke("run_result", input),
  cancelRun: (input) => invoke("cancel_run", input),
};
