<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import UModal from "@nuxt/ui/components/Modal.vue";
import { computed, ref, watch } from "vue";
import { sanitizeDisplayText } from "../composables/useLearningSession";
import type {
  MonacoRange,
  NormalizedDiagnostic,
  RunResponse,
  RunTarget,
  StageResult,
  ValidationResult,
} from "../types/learning";

const CURRICULUM_COMPLETE_TEXT = "All exercises completed.";

const props = defineProps<{
  result?: RunResponse;
  target?: RunTarget;
  running: boolean;
  canCancel: boolean;
  cancelling: boolean;
  runDisabled: boolean;
  error?: string;
}>();

const emit = defineEmits<{
  run: [];
  cancel: [];
  diagnostic: [range: MonacoRange];
}>();

const visibleValidation = computed(() => {
  const result = props.result;
  if (!result) return;
  if (result.validation.outcome.status === "operational_failure") return result.validation;
  return result.finalRecheck[result.finalRecheck.length - 1] ?? result.validation;
});

const outputRecords = computed(() => {
  const result = props.result;
  if (!result) return [];
  return [result.validation, ...result.finalRecheck].flatMap((validation, index) =>
    validation.stages.map((stage) => ({
      exerciseId: validation.exercise_id,
      provenance:
        index > 0
          ? "Final recheck"
          : result.target.kind === "solution"
            ? "Solution code"
            : "Learner code",
      stage,
    })),
  );
});

function sameTarget(left: RunTarget | undefined, right: RunTarget | undefined) {
  return (
    left?.kind === right?.kind &&
    left?.exerciseId === right?.exerciseId &&
    (left?.kind !== "solution" || (right?.kind === "solution" && left.path === right.path))
  );
}

const provenance = computed(() => {
  const target = props.result?.target ?? props.target;
  return target?.kind === "solution" ? "Solution code" : "Learner code";
});
const diagnosticsActionable = computed(() => sameTarget(props.result?.target, props.target));

function validationOutcome(validation: ValidationResult) {
  switch (validation.outcome.status) {
    case "passed":
      return "Passed.";
    case "learner_failure":
      return `Needs another try (${validation.outcome.stage}).`;
    case "cancelled":
      return "Cancelled.";
    case "timed_out":
      return "Timed out.";
    case "output_limit":
      return "Output limit reached.";
    case "operational_failure":
      return `Validation unavailable: ${sanitizeDisplayText(validation.outcome.message, 8_192)}`;
  }
}

const outcomeText = computed(() => {
  if (props.running) return props.cancelling ? "Cancelling validation…" : "Validation running…";
  if (props.error) return `Error: ${sanitizeDisplayText(props.error, 8_192)}`;
  const result = props.result;
  if (!result) return "Ready to run.";
  if (result.stale) return "Stale result — source changed; progress and markers were not updated.";
  const validation = visibleValidation.value;
  if (!validation) return "Ready to run.";
  if (result.snapshot.curriculumComplete && validation.outcome.status === "passed")
    return CURRICULUM_COMPLETE_TEXT;
  const prefix = result.finalRecheck.includes(validation) ? "Final recheck" : provenance.value;
  return `${prefix}: ${validationOutcome(validation)}`;
});

const resultDialogOpen = ref(false);
const resultDialog = ref<{ success: boolean; title: string; message: string }>();
let dialogRunId: string | undefined;

watch(
  () => props.result,
  (result) => {
    // Same runId means a retained result resurfacing (e.g. returning from the
    // solution view), not a completed run — only a fresh run may open the dialog.
    if (!result || result.stale || result.target.kind !== "learner") return;
    if (result.runId === dialogRunId) return;
    const validation = visibleValidation.value;
    if (!validation) return;
    const status = validation.outcome.status;
    if (status !== "passed" && status !== "learner_failure") return;
    dialogRunId = result.runId;
    resultDialog.value =
      status === "passed"
        ? {
            success: true,
            title: "Exercise passed",
            message: result.snapshot.curriculumComplete
              ? CURRICULUM_COMPLETE_TEXT
              : `${result.target.exerciseId} passed.`,
          }
        : {
            success: false,
            title: "Not yet",
            message: result.finalRecheck.includes(validation)
              ? `Final recheck of ${validation.exercise_id}: ${validationOutcome(validation)}`
              : validationOutcome(validation),
          };
    resultDialogOpen.value = true;
  },
);

function stageText(stage: StageResult) {
  const sections = [
    stage.stdout && `stdout\n${stage.stdout}`,
    stage.stderr && `stderr\n${stage.stderr}`,
  ]
    .filter(Boolean)
    .join("\n");
  return sanitizeDisplayText(sections || "No output.", 65_536);
}

function diagnosticText(diagnostic: NormalizedDiagnostic) {
  const location = diagnostic.range
    ? `Line ${diagnostic.range.start_line_number}, column ${diagnostic.range.start_column}: `
    : "";
  return `${location}${sanitizeDisplayText(diagnostic.message, 8_192)}`;
}
</script>

<template>
  <section class="border-t border-default bg-default" aria-labelledby="run-panel-title">
    <div
      class="flex flex-wrap items-center justify-between gap-3 border-b border-default bg-muted/30 p-3"
    >
      <div>
        <h2 id="run-panel-title" class="font-semibold text-highlighted">
          Validation · {{ provenance }}
        </h2>
        <p class="text-sm text-toned" role="status" aria-live="polite" aria-atomic="true">
          {{ outcomeText }}
        </p>
      </div>
      <div class="flex flex-wrap gap-2">
        <UButton
          type="button"
          :label="target?.kind === 'solution' ? 'Run solution' : 'Run'"
          leading-icon="i-lucide-play"
          class="min-h-8 min-w-16"
          :disabled="runDisabled || running"
          @click="emit('run')"
        />
        <UButton
          type="button"
          label="Cancel"
          color="neutral"
          variant="outline"
          class="min-h-8 min-w-16"
          :disabled="!canCancel || cancelling"
          :loading="cancelling"
          @click="emit('cancel')"
        />
      </div>
    </div>

    <UModal
      v-model:open="resultDialogOpen"
      :title="resultDialog?.title"
      :close="false"
      :transition="false"
    >
      <template #body>
        <div class="grid gap-4">
          <p class="font-medium" :class="resultDialog?.success ? 'text-success' : 'text-error'">
            {{ resultDialog?.message }}
          </p>
          <UButton
            type="button"
            :label="resultDialog?.success ? 'Continue' : 'Try again'"
            color="neutral"
            variant="outline"
            class="min-h-8 justify-self-start"
            @click="resultDialogOpen = false"
          />
        </div>
      </template>
    </UModal>

    <div
      class="run-output scroll-panel grid gap-4 bg-default p-3 md:grid-cols-2"
      aria-label="Validation output and diagnostics"
      tabindex="0"
    >
      <div>
        <h3 class="mb-2 text-sm font-medium text-highlighted">Output</h3>
        <div v-if="outputRecords.length" class="space-y-3">
          <section
            v-for="(record, index) in outputRecords"
            :key="`${record.exerciseId}-${record.stage.stage}-${index}`"
          >
            <h4 class="text-xs font-semibold uppercase text-muted">
              {{ record.provenance }} · {{ record.exerciseId }} · {{ record.stage.stage }} —
              {{ record.stage.success ? "Succeeded" : "Failed"
              }}<span v-if="record.stage.output_truncated"> — Truncated</span>
            </h4>
            <pre class="plain-text mt-1">{{ stageText(record.stage) }}</pre>
          </section>
        </div>
        <p v-else class="text-sm text-muted">No validation output yet.</p>
      </div>

      <div>
        <h3 class="mb-2 text-sm font-medium text-highlighted">Diagnostics</h3>
        <ul v-if="visibleValidation?.diagnostics.length" class="space-y-2">
          <li v-for="(diagnostic, index) in visibleValidation.diagnostics" :key="index">
            <UButton
              v-if="diagnostic.range && !result?.stale && diagnosticsActionable"
              type="button"
              variant="soft"
              color="neutral"
              block
              class="min-h-8 justify-start whitespace-normal text-start"
              :label="diagnosticText(diagnostic)"
              @click="emit('diagnostic', diagnostic.range)"
            />
            <p v-else class="plain-text rounded bg-muted/40 p-2 text-sm">
              {{ diagnosticText(diagnostic) }}
            </p>
          </li>
        </ul>
        <p v-else class="text-sm text-muted">No diagnostics.</p>
      </div>
    </div>
  </section>
</template>
