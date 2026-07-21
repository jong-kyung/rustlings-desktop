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
import {
  clampSidebarWidth,
  MAX_SIDEBAR_WIDTH,
  MIN_SIDEBAR_WIDTH,
  persistSidebarWidth,
  readSidebarWidth,
} from "./lib/sidebarWidth";
import type { RustMarker } from "./monaco/setup";
import { toMonacoRange, type MonacoRange, type RunTarget } from "./types/learning";

const session = useLearningSession();
const editor = ref<{ focusRange(range: RustMarker["range"]): void }>();
const learningShell = ref<HTMLElement>();
const sidebarWidth = ref(readSidebarWidth());
const keyboardHelpOpen = ref(false);
let resizingPointerId: number | undefined;
let resizingShellLeft = 0;
let resizingStartWidth = 0;
let unlistenClose: (() => void) | undefined;
const readme = computed(() => sanitizeDisplayText(session.viewReadme.value));
const hint = computed(() =>
  session.viewingSolution.value || session.hint.value === undefined
    ? undefined
    : sanitizeDisplayText(session.hint.value),
);
const currentTarget = computed<RunTarget>(() =>
  session.solution.value
    ? {
        kind: "solution",
        exerciseId: session.solution.value.exerciseId,
        path: session.solution.value.path,
      }
    : { kind: "learner", exerciseId: session.snapshot.value?.selected ?? "" },
);

function closeKeyboardHelp() {
  keyboardHelpOpen.value = false;
}

function focusDiagnostic(range: MonacoRange) {
  editor.value?.focusRange(toMonacoRange(range));
}

function resizeSidebar(clientX: number) {
  sidebarWidth.value = clampSidebarWidth(clientX - resizingShellLeft);
}

function startSidebarResize(event: PointerEvent) {
  if (resizingPointerId !== undefined || event.button !== 0) return;
  resizingPointerId = event.pointerId;
  resizingShellLeft = learningShell.value?.getBoundingClientRect().left ?? 0;
  resizingStartWidth = sidebarWidth.value;
  if (event.currentTarget instanceof HTMLElement) {
    event.currentTarget.setPointerCapture(event.pointerId);
  }
}

function moveSidebarResize(event: PointerEvent) {
  if (event.pointerId === resizingPointerId) resizeSidebar(event.clientX);
}

function finishSidebarResize(event: PointerEvent) {
  if (event.pointerId !== resizingPointerId) return;
  if (event.type === "pointerup") resizeSidebar(event.clientX);
  resizingPointerId = undefined;
  if (sidebarWidth.value !== resizingStartWidth) persistSidebarWidth(sidebarWidth.value);
}

function resizeSidebarWithKeyboard(event: KeyboardEvent) {
  let width: number;
  switch (event.key) {
    case "ArrowLeft":
      width = sidebarWidth.value - 8;
      break;
    case "ArrowRight":
      width = sidebarWidth.value + 8;
      break;
    case "Home":
      width = MIN_SIDEBAR_WIDTH;
      break;
    case "End":
      width = MAX_SIDEBAR_WIDTH;
      break;
    default:
      return;
  }
  event.preventDefault();
  const next = clampSidebarWidth(width);
  if (next === sidebarWidth.value) return;
  sidebarWidth.value = next;
  persistSidebarWidth(next);
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

      <div
        v-else
        ref="learningShell"
        class="learning-shell mx-auto max-w-[120rem] border border-default bg-default"
        :style="{ '--sidebar-width': `${sidebarWidth}px` }"
      >
        <ExerciseSidebar
          :exercises="session.snapshot.value.exercises"
          :selected="session.snapshot.value.selected"
          :selected-solution="session.solution.value?.exerciseId"
          :disabled="session.running.value"
          :dirty="session.dirty.value"
          :saving="session.saving.value"
          :save-error="
            session.saveError.value
              ? sanitizeDisplayText(session.saveError.value, 8_192)
              : undefined
          "
          @select="session.selectExercise"
          @select-solution="session.revealSolution"
        />

        <div
          class="sidebar-resizer"
          role="separator"
          aria-label="Resize Rustlings sidebar"
          aria-orientation="vertical"
          :aria-valuemin="MIN_SIDEBAR_WIDTH"
          :aria-valuemax="MAX_SIDEBAR_WIDTH"
          :aria-valuenow="sidebarWidth"
          tabindex="0"
          @pointerdown="startSidebarResize"
          @pointermove="moveSidebarResize"
          @pointerup="finishSidebarResize"
          @pointercancel="finishSidebarResize"
          @keydown="resizeSidebarWithKeyboard"
        />

        <section class="workspace min-w-0">
          <header
            class="flex flex-wrap items-center justify-between gap-3 border-b border-default p-3"
          >
            <div class="min-w-0">
              <p class="text-xs text-muted">
                {{ session.viewingSolution.value ? "Current solution" : "Current exercise" }}
              </p>
              <h1 class="truncate font-mono text-xl font-semibold text-highlighted">
                {{
                  session.viewingSolution.value
                    ? session.viewPath.value
                    : session.snapshot.value.selected
                }}
              </h1>
              <p v-if="session.viewingSolution.value" class="text-sm font-medium text-warning">
                Read-only
              </p>
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
                  :model-id="session.viewModelId.value"
                  :path="session.viewPath.value"
                  :source="session.viewSource.value"
                  :source-digest="session.viewSourceDigest.value"
                  :read-only="session.viewingSolution.value"
                  :aria-label="
                    session.viewingSolution.value
                      ? 'Read-only Rust solution editor'
                      : 'Rust source editor'
                  "
                  :diagnostics="session.diagnostics.value"
                  @change="session.editSource"
                  @run="session.run"
                />
              </div>
            </section>

            <LessonPanel
              :readme="readme"
              :hint="hint"
              :show-hint="!session.viewingSolution.value"
              :disabled="session.running.value || session.navigating.value"
              @reveal-hint="session.revealHint"
            />
          </div>

          <RunPanel
            :result="session.runResult.value"
            :target="session.activeRun.value?.target ?? currentTarget"
            :running="session.running.value"
            :can-cancel="session.canCancel.value"
            :cancelling="session.cancelling.value"
            :run-disabled="!session.snapshot.value.preflight.ready || session.navigating.value"
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
