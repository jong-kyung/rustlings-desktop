<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import type { ExerciseSnapshot } from "../types/learning";

defineProps<{
  exercises: ExerciseSnapshot[];
  selected: string;
  disabled: boolean;
  dirty: boolean;
  saving: boolean;
  saveError?: string;
}>();

const emit = defineEmits<{
  select: [exerciseId: string];
}>();

function statusLabel(exercise: ExerciseSnapshot) {
  switch (exercise.status) {
    case "locked":
      return "Locked";
    case "completed":
      return "Completed";
    case "current":
      return "Current";
    default:
      return "Available";
  }
}
</script>

<template>
  <aside
    class="exercise-panel scroll-panel border-b border-default bg-default"
    aria-labelledby="exercises-title"
    tabindex="0"
  >
    <header class="border-b border-default bg-muted/30 p-3">
      <h2 id="exercises-title" class="font-semibold text-highlighted">Exercises</h2>
      <p class="text-xs text-muted">Intro and Variables</p>
    </header>

    <div class="p-3">
      <ol class="space-y-2">
        <li v-for="exercise in exercises" :key="exercise.id">
          <UButton
            type="button"
            variant="ghost"
            color="neutral"
            block
            class="min-h-8 justify-between text-start"
            :disabled="disabled || exercise.status === 'locked'"
            :aria-current="exercise.id === selected ? 'step' : undefined"
            :aria-label="`${exercise.id}, ${statusLabel(exercise)}`"
            @click="emit('select', exercise.id)"
          >
            <span class="min-w-0 truncate font-mono text-sm">{{ exercise.id }}</span>
            <span class="ms-2 shrink-0 text-xs">{{ statusLabel(exercise) }}</span>
          </UButton>
        </li>
      </ol>

      <p class="mt-4 text-xs" :class="saveError ? 'text-error' : 'text-muted'" role="status">
        {{
          saveError ? `Save failed: ${saveError}` : saving ? "Saving…" : dirty ? "Unsaved" : "Saved"
        }}
      </p>
    </div>
  </aside>
</template>
