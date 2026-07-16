<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import { computed } from "vue";
import { sanitizeDisplayText } from "../composables/useLearningSession";
import type {
  MonacoRange,
  NormalizedDiagnostic,
  RunResponse,
  StageResult,
  ValidationResult,
} from "../types/learning";

const props = defineProps<{
  result?: RunResponse;
  running: boolean;
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
  return result.finalRecheck[result.finalRecheck.length - 1] ?? result.validation;
});

const outputRecords = computed(() => {
  const result = props.result;
  if (!result) return [];
  return [result.validation, ...result.finalRecheck].flatMap((validation) =>
    validation.stages.map((stage) => ({ exerciseId: validation.exercise_id, stage })),
  );
});

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
  if (result.snapshot.sliceComplete) return "All exercises completed.";
  const validation = visibleValidation.value;
  if (!validation) return "Ready to run.";
  const prefix = result.finalRecheck.includes(validation) ? "Final recheck: " : "";
  return `${prefix}${validationOutcome(validation)}`;
});

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
  <section class="border-default bg-default" aria-labelledby="run-panel-title">
    <div class="flex flex-wrap items-center justify-between gap-3 border-b border-default p-3">
      <div>
        <h2 id="run-panel-title" class="font-semibold text-highlighted">Validation</h2>
        <p class="text-sm text-toned" role="status" aria-live="polite" aria-atomic="true">
          {{ outcomeText }}
        </p>
      </div>
      <div class="flex flex-wrap gap-2">
        <UButton
          type="button"
          label="Run"
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
          :disabled="!running || cancelling"
          :loading="cancelling"
          @click="emit('cancel')"
        />
      </div>
    </div>

    <div
      class="run-output scroll-panel grid gap-4 p-3 md:grid-cols-2"
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
              {{ record.exerciseId }} · {{ record.stage.stage }} —
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
              v-if="diagnostic.range && !result?.stale"
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
