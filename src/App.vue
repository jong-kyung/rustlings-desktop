<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import UApp from "@nuxt/ui/components/App.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import UModal from "@nuxt/ui/components/Modal.vue";
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import ExerciseSidebar from "./components/ExerciseSidebar.vue";
import LessonPanel from "./components/LessonPanel.vue";
import RunPanel from "./components/RunPanel.vue";
import RustEditor from "./components/RustEditor.vue";
import ToolchainGate from "./components/ToolchainGate.vue";
import { sanitizeDisplayText, useLearningSession } from "./composables/useLearningSession";
import type { RustMarker } from "./monaco/setup";
import type { MonacoRange } from "./types/learning";

const session = useLearningSession();
const editor = ref<{ focusRange(range: RustMarker["range"]): void }>();
const keyboardHelpOpen = ref(false);
const solutionReviewOpen = ref(false);
let unlistenClose: (() => void) | undefined;
const readme = computed(() => sanitizeDisplayText(session.snapshot.value?.readme ?? ""));
const hint = computed(() =>
  session.hint.value === undefined ? undefined : sanitizeDisplayText(session.hint.value),
);
const reviewSource = computed(() => sanitizeCode(session.source.value));
const referenceSolution = computed(() => sanitizeCode(session.solution.value ?? ""));

function sanitizeCode(value: string) {
  return sanitizeDisplayText(value, Math.max(1, value.length));
}

function closeKeyboardHelp() {
  keyboardHelpOpen.value = false;
}

async function openSolutionReview() {
  if (await session.revealSolution()) solutionReviewOpen.value = true;
}

function closeSolutionReview() {
  solutionReviewOpen.value = false;
}

function focusDiagnostic(range: MonacoRange) {
  editor.value?.focusRange({
    startLineNumber: range.start_line_number,
    startColumn: range.start_column,
    endLineNumber: range.end_line_number,
    endColumn: range.end_column,
  });
}

onMounted(async () => {
  await session.initialize();
  try {
    const appWindow = getCurrentWindow();
    unlistenClose = await appWindow.onCloseRequested(async (event) => {
      if (!session.dirty.value) return;
      event.preventDefault();
      if (await session.flushSaves()) await appWindow.destroy();
    });
  } catch {
    // Browser previews do not expose Tauri window events.
  }
});

onBeforeUnmount(() => unlistenClose?.());
</script>

<template>
  <UApp :toaster="null">
    <main class="app-surface bg-default p-3 text-default sm:p-4">
      <section
        v-if="session.loading.value"
        class="grid min-h-[50svh] place-items-center"
        role="status"
      >
        Loading learning workspace…
      </section>

      <section
        v-else-if="!session.snapshot.value"
        class="mx-auto flex min-h-[50svh] max-w-xl flex-col items-start justify-center gap-4"
        role="alert"
      >
        <div>
          <h1 class="text-2xl font-semibold text-highlighted">Rustlings Desktop</h1>
          <p class="mt-2 plain-text text-error">
            {{
              sanitizeDisplayText(session.error.value ?? "Unable to load the learning workspace.")
            }}
          </p>
        </div>
        <UButton
          type="button"
          label="Retry"
          class="min-h-8"
          @click="() => void session.initialize()"
        />
      </section>

      <div v-else class="learning-shell mx-auto max-w-[120rem] border border-default bg-default">
        <ExerciseSidebar
          :exercises="session.snapshot.value.exercises"
          :selected="session.snapshot.value.selected"
          :disabled="
            session.running.value || session.navigating.value || session.revealingSolution.value
          "
          :dirty="session.dirty.value"
          :saving="session.saving.value"
          :save-error="
            session.saveError.value
              ? sanitizeDisplayText(session.saveError.value, 8_192)
              : undefined
          "
          @select="session.selectExercise"
        />

        <section class="workspace min-w-0">
          <header
            class="flex flex-wrap items-center justify-between gap-3 border-b border-default p-3"
          >
            <div class="min-w-0">
              <p class="text-xs text-muted">Current exercise</p>
              <h1 class="truncate font-mono text-xl font-semibold text-highlighted">
                {{ session.snapshot.value.selected }}
              </h1>
              <p
                v-if="session.snapshot.value.curriculumComplete"
                class="text-sm font-medium text-success"
              >
                Curriculum complete
              </p>
            </div>

            <UModal
              v-model:open="keyboardHelpOpen"
              title="Keyboard help"
              :close="false"
              :transition="false"
            >
              <UButton
                type="button"
                label="Keyboard help"
                color="neutral"
                variant="ghost"
                class="min-h-8"
              />
              <template #body>
                <div class="grid gap-4">
                  <dl class="grid gap-3 text-sm">
                    <div>
                      <dt class="font-medium text-highlighted">Run</dt>
                      <dd class="text-toned">
                        Command-Enter while editing, or use the Run button.
                      </dd>
                    </div>
                    <div>
                      <dt class="font-medium text-highlighted">Leave the editor</dt>
                      <dd class="text-toned">
                        Press Control-M to toggle Tab moves focus, then press Tab.
                      </dd>
                    </div>
                  </dl>
                  <UButton
                    type="button"
                    label="Close keyboard help"
                    color="neutral"
                    variant="outline"
                    class="min-h-8 justify-self-start"
                    @click="closeKeyboardHelp"
                  />
                </div>
              </template>
            </UModal>

            <UModal
              v-model:open="solutionReviewOpen"
              title="Solution review"
              description="Compare your completed answer with one reference solution."
              :close="false"
              :transition="false"
              scrollable
              :ui="{ content: 'sm:max-w-6xl' }"
            >
              <template #body>
                <div class="grid min-w-0 gap-4 sm:grid-cols-2">
                  <section class="min-w-0" aria-labelledby="review-source-title">
                    <h3 id="review-source-title" class="mb-2 font-medium text-highlighted">
                      Your solution
                    </h3>
                    <pre
                      class="plain-text max-h-[60svh] overflow-auto rounded-md border border-default bg-muted/30 p-3"
                      tabindex="0"
                    ><code>{{ reviewSource }}</code></pre>
                  </section>
                  <section class="min-w-0" aria-labelledby="reference-solution-title">
                    <h3 id="reference-solution-title" class="mb-2 font-medium text-highlighted">
                      Reference solution
                    </h3>
                    <pre
                      class="plain-text max-h-[60svh] overflow-auto rounded-md border border-default bg-muted/30 p-3"
                      tabindex="0"
                    ><code>{{ referenceSolution }}</code></pre>
                  </section>
                </div>
                <UButton
                  type="button"
                  label="Close solution review"
                  color="neutral"
                  variant="outline"
                  class="mt-4 min-h-8"
                  @click="closeSolutionReview"
                />
              </template>
            </UModal>
          </header>

          <div class="learning-content min-h-0 min-w-0">
            <section
              class="code-panel min-h-0 min-w-0 border-b border-default bg-default"
              aria-labelledby="code-title"
            >
              <header class="border-b border-default bg-muted/30 p-3">
                <h2 id="code-title" class="font-semibold text-highlighted">Code</h2>
              </header>
              <div class="min-h-0 min-w-0 bg-default">
                <ToolchainGate
                  v-if="!session.snapshot.value.preflight.ready"
                  :message="sanitizeDisplayText(session.snapshot.value.preflight.message ?? '')"
                  :retrying="session.retryingPreflight.value"
                  @retry="session.retryPreflight"
                />
                <RustEditor
                  v-else
                  ref="editor"
                  :exercise-id="session.snapshot.value.selected"
                  :source="session.source.value"
                  :source-digest="session.snapshot.value.sourceDigest"
                  :diagnostics="session.diagnostics.value"
                  @change="session.editSource"
                  @run="session.run"
                />
              </div>
            </section>

            <LessonPanel
              :readme="readme"
              :hint="hint"
              :solution-available="session.snapshot.value.solutionAvailable"
              :revealing-solution="session.revealingSolution.value"
              :disabled="
                session.running.value || session.navigating.value || session.revealingSolution.value
              "
              @reveal-hint="session.revealHint"
              @reveal-solution="openSolutionReview"
            />
          </div>

          <RunPanel
            :result="session.runResult.value"
            :running="session.running.value"
            :can-cancel="session.canCancel.value"
            :cancelling="session.cancelling.value"
            :run-disabled="!session.snapshot.value.preflight.ready"
            :error="session.error.value"
            @run="session.run()"
            @cancel="session.cancel"
            @diagnostic="focusDiagnostic"
          />
        </section>
      </div>
    </main>
  </UApp>
</template>
