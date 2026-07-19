export type ExerciseStatus = "locked" | "current" | "completed" | "unlocked";
export type ValidationStage = "build" | "test" | "clippy" | "program";
export type DiagnosticSeverity = "error" | "warning" | "info" | "hint";
export type OperationalKind =
  | "infrastructure"
  | "storage"
  | "spawn"
  | "busy"
  | "process"
  | "cargo_startup"
  | "artifact";

export interface ExerciseSnapshot {
  id: string;
  sourcePath: string;
  status: ExerciseStatus;
  revision: number;
}

export interface PreflightSnapshot {
  ready: boolean;
  message: string | null;
  rustcVersion: string | null;
}

export interface SessionSnapshot {
  selected: string;
  source: string;
  sourceDigest: string;
  readme: string;
  exercises: ExerciseSnapshot[];
  activeRunId: string | null;
  curriculumComplete: boolean;
  solutionAvailable: boolean;
  preflight: PreflightSnapshot;
}

export interface SaveSourceResponse {
  revision: number;
  sourceDigest: string;
  snapshot: SessionSnapshot;
}

export interface HintResponse {
  exerciseId: string;
  hint: string;
}

export interface SolutionResponse {
  exerciseId: string;
  solution: string;
}

export interface RunTicket {
  runId: string;
  exerciseId: string;
  revision: number;
  sourceDigest: string;
}

export type ValidationOutcome =
  | { status: "passed" }
  | { status: "learner_failure"; stage: ValidationStage }
  | { status: "cancelled" }
  | { status: "timed_out" }
  | { status: "output_limit" }
  | { status: "operational_failure"; kind: OperationalKind; message: string };

export interface MonacoRange {
  start_line_number: number;
  start_column: number;
  end_line_number: number;
  end_column: number;
}

export interface NormalizedDiagnostic {
  stage: ValidationStage;
  severity: DiagnosticSeverity;
  message: string;
  code: string | null;
  range: MonacoRange | null;
  source_digest: string;
}

export interface StageResult {
  stage: ValidationStage;
  success: boolean;
  stdout: string;
  stderr: string;
  output_truncated: boolean;
}

export interface ValidationResult {
  exercise_id: string;
  source_digest: string;
  outcome: ValidationOutcome;
  stages: StageResult[];
  diagnostics: NormalizedDiagnostic[];
}

export interface RunResponse {
  runId: string;
  revision: number;
  stale: boolean;
  validation: ValidationResult;
  finalRecheck: ValidationResult[];
  snapshot: SessionSnapshot;
}

export type CancelRunResult = "requested" | "not_active" | "id_mismatch";
