<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, watch } from "vue";
import type { ExerciseSnapshot } from "../types/learning";

const props = defineProps<{
  exercises: ExerciseSnapshot[];
  selected: string;
  selectedSolutionAvailable: boolean;
  disabled: boolean;
  dirty: boolean;
  saving: boolean;
  saveError?: string;
}>();

const emit = defineEmits<{
  select: [exerciseId: string];
}>();

interface DisplayExercise {
  exercise: ExerciseSnapshot;
  filename: string;
  displayedPath: string;
}

interface ExerciseFolder {
  name: string;
  path: string;
  exercises: DisplayExercise[];
}

function pathParts(exercise: ExerciseSnapshot) {
  return exercise.sourcePath.split("/");
}

function folderPath(exerciseId: string) {
  const exercise = props.exercises.find((item) => item.id === exerciseId);
  return exercise ? pathParts(exercise).slice(0, -1).join("/") : undefined;
}

const folders = computed(() => {
  const byPath = new Map<string, ExerciseFolder>();
  for (const exercise of props.exercises) {
    const parts = pathParts(exercise);
    const filename = parts.at(-1) ?? exercise.sourcePath;
    const path = parts.slice(0, -1).join("/");
    let folder = byPath.get(path);
    if (!folder) {
      folder = { name: parts.at(-2) ?? path, path, exercises: [] };
      byPath.set(path, folder);
    }
    folder.exercises.push({
      exercise,
      filename,
      displayedPath: `Exercises/${parts.slice(1).join("/")}`,
    });
  }
  return [...byPath.values()];
});

const initialFolder = folderPath(props.selected);
const expanded = ref(new Set(initialFolder ? ["exercises", initialFolder] : ["exercises"]));
const query = ref("");
let preSearchExpansion: Set<string> | undefined;

watch(
  () => props.selected,
  (selected) => {
    if (query.value.trim()) return;
    const selectedFolder = folderPath(selected);
    if (selectedFolder) expanded.value = new Set([...expanded.value, "exercises", selectedFolder]);
  },
);

watch(query, (value, previous) => {
  const searching = Boolean(value.trim());
  const wasSearching = Boolean(previous.trim());
  if (searching && !wasSearching) preSearchExpansion = new Set(expanded.value);
  if (!searching && wasSearching && preSearchExpansion) {
    const selectedFolder = folderPath(props.selected);
    expanded.value = new Set([
      ...preSearchExpansion,
      "exercises",
      ...(selectedFolder ? [selectedFolder] : []),
    ]);
    preSearchExpansion = undefined;
  }
});

const normalizedQuery = computed(() => query.value.trim().toLowerCase());
const visibleFolders = computed(() => {
  if (!normalizedQuery.value) return folders.value;
  return folders.value
    .map((folder) => ({
      ...folder,
      exercises: folder.exercises.filter((item) =>
        item.displayedPath.toLowerCase().includes(normalizedQuery.value),
      ),
    }))
    .filter((folder) => folder.exercises.length > 0);
});

function isExpanded(path: string) {
  return normalizedQuery.value ? true : expanded.value.has(path);
}

function toggle(path: string) {
  if (normalizedQuery.value) return;
  const next = new Set(expanded.value);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  expanded.value = next;
}

function isCompleted(exercise: ExerciseSnapshot) {
  return (
    exercise.status === "completed" ||
    (exercise.id === props.selected && props.selectedSolutionAvailable)
  );
}

function completedCount(folder?: ExerciseFolder) {
  const items = folder ? folder.exercises : folders.value.flatMap((item) => item.exercises);
  return items.filter((item) => isCompleted(item.exercise)).length;
}

function statusLabel(exercise: ExerciseSnapshot) {
  if (isCompleted(exercise)) return "Completed";
  switch (exercise.status) {
    case "locked":
      return "Locked";
    case "current":
      return "Current";
    default:
      return "Available";
  }
}

function statusIcon(exercise: ExerciseSnapshot) {
  switch (statusLabel(exercise)) {
    case "Locked":
      return "🔒";
    case "Completed":
      return "✓";
    case "Current":
      return "●";
    default:
      return "○";
  }
}
</script>

<template>
  <aside
    class="exercise-panel border-b border-default bg-default"
    aria-labelledby="exercises-title"
  >
    <header class="border-b border-default bg-muted/30 p-3">
      <h2 id="exercises-title" class="font-semibold text-highlighted">Rustlings</h2>
      <label for="exercise-search" class="sr-only">Search exercises</label>
      <input
        id="exercise-search"
        v-model="query"
        type="search"
        class="mt-2 min-h-8 w-full rounded-md border border-default bg-default px-2 text-sm text-default outline-none placeholder:text-muted focus-visible:ring-2 focus-visible:ring-primary"
        placeholder="Search exercises"
        autocomplete="off"
      />
    </header>

    <div class="exercise-tree-scroll p-2">
      <ul>
        <li data-folder-path="exercises">
          <UButton
            type="button"
            variant="ghost"
            color="neutral"
            block
            class="min-h-8 justify-start text-start"
            aria-label="Exercises folder"
            :aria-expanded="isExpanded('exercises')"
            @click="toggle('exercises')"
          >
            <span aria-hidden="true" class="me-2">{{ isExpanded("exercises") ? "▾" : "▸" }}</span>
            <span class="font-medium">Exercises</span>
            <span class="ms-auto text-xs text-muted">
              {{ completedCount() }}/{{ exercises.length }} completed
            </span>
          </UButton>

          <ul v-if="isExpanded('exercises')" class="ms-3 border-s border-default ps-2">
            <li v-for="folder in visibleFolders" :key="folder.path" :data-folder-path="folder.path">
              <UButton
                type="button"
                variant="ghost"
                color="neutral"
                block
                class="min-h-8 justify-start text-start"
                :aria-label="`${folder.name} folder`"
                :aria-expanded="isExpanded(folder.path)"
                @click="toggle(folder.path)"
              >
                <span aria-hidden="true" class="me-2">
                  {{ isExpanded(folder.path) ? "▾" : "▸" }}
                </span>
                <span class="min-w-0 truncate font-mono text-sm">{{ folder.name }}</span>
                <span class="ms-auto shrink-0 text-xs text-muted">
                  {{ completedCount(folder) }}/{{ folder.exercises.length }} completed
                </span>
              </UButton>

              <ul v-if="isExpanded(folder.path)" class="ms-3 border-s border-default ps-2">
                <li v-for="item in folder.exercises" :key="item.exercise.id">
                  <UButton
                    type="button"
                    variant="ghost"
                    color="neutral"
                    block
                    class="min-h-8 justify-start text-start"
                    :disabled="disabled || item.exercise.status === 'locked'"
                    :aria-current="item.exercise.id === selected ? 'step' : undefined"
                    :aria-label="`${item.filename}, ${statusLabel(item.exercise)}`"
                    @click="emit('select', item.exercise.id)"
                  >
                    <span aria-hidden="true" class="me-2 shrink-0">
                      {{ statusIcon(item.exercise) }}
                    </span>
                    <span class="min-w-0 truncate font-mono text-sm">{{ item.filename }}</span>
                    <span class="sr-only">{{ statusLabel(item.exercise) }}</span>
                  </UButton>
                </li>
              </ul>
            </li>
          </ul>
        </li>
      </ul>

      <p
        v-if="normalizedQuery && visibleFolders.length === 0"
        class="p-3 text-sm text-muted"
        role="status"
      >
        No exercises found.
      </p>
    </div>

    <footer class="border-t border-default bg-muted/30 p-3">
      <p class="text-xs" :class="saveError ? 'text-error' : 'text-muted'" role="status">
        {{
          saveError ? `Save failed: ${saveError}` : saving ? "Saving…" : dirty ? "Unsaved" : "Saved"
        }}
      </p>
    </footer>
  </aside>
</template>
