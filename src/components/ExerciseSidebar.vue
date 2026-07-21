<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, watch } from "vue";
import type { ExerciseSnapshot } from "../types/learning";

const props = defineProps<{
  exercises: ExerciseSnapshot[];
  selected: string;
  selectedSolution?: string;
  disabled: boolean;
  dirty: boolean;
  saving: boolean;
  saveError?: string;
}>();

const emit = defineEmits<{
  select: [exerciseId: string];
  selectSolution: [exerciseId: string];
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

function folderPath(exerciseId: string, solution = false) {
  const exercise = props.exercises.find((item) => item.id === exerciseId);
  const path = solution ? exercise?.solutionPath : exercise?.sourcePath;
  return path?.split("/").slice(0, -1).join("/");
}

function buildFolders(solution: boolean) {
  const byPath = new Map<string, ExerciseFolder>();
  for (const exercise of props.exercises) {
    const sourcePath = solution ? exercise.solutionPath : exercise.sourcePath;
    const parts = sourcePath.split("/");
    const filename = parts.at(-1) ?? sourcePath;
    const path = parts.slice(0, -1).join("/");
    let folder = byPath.get(path);
    if (!folder) {
      folder = { name: parts.at(-2) ?? path, path, exercises: [] };
      byPath.set(path, folder);
    }
    folder.exercises.push({
      exercise,
      filename,
      displayedPath: `${solution ? "Solutions" : "Exercises"}/${parts.slice(1).join("/")}`,
    });
  }
  return [...byPath.values()];
}

const exerciseFolders = computed(() => buildFolders(false));
const solutionFolders = computed(() => buildFolders(true));

const initialFolder = folderPath(props.selected);
const expanded = ref(new Set(initialFolder ? ["exercises", initialFolder] : ["exercises"]));
const query = ref("");
let preSearchExpansion: Set<string> | undefined;
let preSearchSelection: string | undefined;
let preSearchSolution: string | undefined;

function revealSelection(root: string, exerciseId: string | undefined, solution = false) {
  if (!exerciseId || query.value.trim()) return;
  const selectedFolder = folderPath(exerciseId, solution);
  if (selectedFolder && (!expanded.value.has(root) || !expanded.value.has(selectedFolder))) {
    expanded.value = new Set([...expanded.value, root, selectedFolder]);
  }
}

watch(
  () => props.selected,
  (selected) => revealSelection("exercises", selected),
);

watch(
  () => props.selectedSolution,
  (selected) => revealSelection("solutions", selected, true),
);

watch(
  query,
  (value, previous) => {
    const searching = Boolean(value.trim());
    const wasSearching = Boolean(previous.trim());
    if (searching && !wasSearching) {
      preSearchExpansion = new Set(expanded.value);
      preSearchSelection = props.selected;
      preSearchSolution = props.selectedSolution;
    }
    if (!searching && wasSearching && preSearchExpansion) {
      const restored = new Set(preSearchExpansion);
      if (props.selected !== preSearchSelection) {
        const selectedFolder = folderPath(props.selected);
        if (selectedFolder) {
          restored.add("exercises");
          restored.add(selectedFolder);
        }
      }
      if (props.selectedSolution && props.selectedSolution !== preSearchSolution) {
        const selectedFolder = folderPath(props.selectedSolution, true);
        if (selectedFolder) {
          restored.add("solutions");
          restored.add(selectedFolder);
        }
      }
      expanded.value = restored;
      preSearchExpansion = undefined;
      preSearchSelection = undefined;
      preSearchSolution = undefined;
    }
  },
  { flush: "sync" },
);

const normalizedQuery = computed(() => query.value.trim().toLowerCase());
function filterFolders(folders: ExerciseFolder[]) {
  if (!normalizedQuery.value) return folders;
  return folders
    .map((folder) => ({
      ...folder,
      exercises: folder.exercises.filter((item) =>
        item.displayedPath.toLowerCase().includes(normalizedQuery.value),
      ),
    }))
    .filter((folder) => folder.exercises.length > 0);
}
const visibleExerciseFolders = computed(() => filterFolders(exerciseFolders.value));
const visibleSolutionFolders = computed(() => filterFolders(solutionFolders.value));

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
  return exercise.status === "completed" || exercise.solutionAvailable;
}

function completedCount(folder?: ExerciseFolder) {
  const items = folder ? folder.exercises : exerciseFolders.value.flatMap((item) => item.exercises);
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

function availableCount(folder?: ExerciseFolder) {
  const items = folder ? folder.exercises : solutionFolders.value.flatMap((item) => item.exercises);
  return items.filter((item) => item.exercise.solutionAvailable).length;
}

function statusIcon(exercise: ExerciseSnapshot) {
  if (isCompleted(exercise)) return "i-lucide-circle-check";
  switch (exercise.status) {
    case "locked":
      return "i-lucide-lock";
    case "current":
      return "i-lucide-circle-dot";
    default:
      return "i-lucide-circle";
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
      <label for="exercise-search" class="sr-only">Search Rustlings files</label>
      <input
        id="exercise-search"
        v-model="query"
        type="search"
        class="mt-2 min-h-8 w-full rounded-md border border-default bg-default px-2 text-sm text-default outline-none placeholder:text-muted focus-visible:ring-2 focus-visible:ring-primary"
        placeholder="Search exercises and solutions"
        autocomplete="off"
      />
    </header>

    <div class="exercise-tree-scroll p-2">
      <ul>
        <li v-if="!normalizedQuery || visibleExerciseFolders.length" data-folder-path="exercises">
          <UButton
            type="button"
            variant="ghost"
            color="neutral"
            block
            :leading-icon="
              isExpanded('exercises') ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'
            "
            class="min-h-8 justify-start text-start"
            :aria-label="`Exercises folder, ${completedCount()} of ${exercises.length} completed`"
            :aria-expanded="isExpanded('exercises')"
            @click="toggle('exercises')"
          >
            <span class="font-medium">Exercises</span>
            <span class="ms-auto text-xs text-muted">
              {{ completedCount() }}/{{ exercises.length }} completed
            </span>
          </UButton>

          <ul v-if="isExpanded('exercises')" class="ms-3 border-s border-default ps-2">
            <li
              v-for="folder in visibleExerciseFolders"
              :key="folder.path"
              :data-folder-path="folder.path"
            >
              <UButton
                type="button"
                variant="ghost"
                color="neutral"
                block
                :leading-icon="
                  isExpanded(folder.path) ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'
                "
                class="min-h-8 justify-start text-start"
                :aria-label="`${folder.name} folder, ${completedCount(folder)} of ${folder.exercises.length} completed`"
                :aria-expanded="isExpanded(folder.path)"
                @click="toggle(folder.path)"
              >
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
                    :leading-icon="statusIcon(item.exercise)"
                    :ui="isCompleted(item.exercise) ? { leadingIcon: 'text-success' } : undefined"
                    class="min-h-8 justify-start text-start"
                    :disabled="disabled || item.exercise.status === 'locked'"
                    :aria-current="item.exercise.id === selected ? 'step' : undefined"
                    :aria-label="`${item.filename}, ${statusLabel(item.exercise)}`"
                    @click="emit('select', item.exercise.id)"
                  >
                    <span class="min-w-0 truncate font-mono text-sm">{{ item.filename }}</span>
                    <span class="sr-only">{{ statusLabel(item.exercise) }}</span>
                  </UButton>
                </li>
              </ul>
            </li>
          </ul>
        </li>

        <li v-if="!normalizedQuery || visibleSolutionFolders.length" data-folder-path="solutions">
          <UButton
            type="button"
            variant="ghost"
            color="neutral"
            block
            :leading-icon="
              isExpanded('solutions') ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'
            "
            class="min-h-8 justify-start text-start"
            :aria-label="`Solutions folder, ${availableCount()} of ${exercises.length} available`"
            :aria-expanded="isExpanded('solutions')"
            @click="toggle('solutions')"
          >
            <span class="font-medium">Solutions</span>
            <span class="ms-auto text-xs text-muted">
              {{ availableCount() }}/{{ exercises.length }} available
            </span>
          </UButton>

          <ul v-if="isExpanded('solutions')" class="ms-3 border-s border-default ps-2">
            <li
              v-for="folder in visibleSolutionFolders"
              :key="folder.path"
              :data-folder-path="folder.path"
            >
              <UButton
                type="button"
                variant="ghost"
                color="neutral"
                block
                :leading-icon="
                  isExpanded(folder.path) ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'
                "
                class="min-h-8 justify-start text-start"
                :aria-label="`${folder.name} solutions folder, ${availableCount(folder)} of ${folder.exercises.length} available`"
                :aria-expanded="isExpanded(folder.path)"
                @click="toggle(folder.path)"
              >
                <span class="min-w-0 truncate font-mono text-sm">{{ folder.name }}</span>
                <span class="ms-auto shrink-0 text-xs text-muted">
                  {{ availableCount(folder) }}/{{ folder.exercises.length }} available
                </span>
              </UButton>

              <ul v-if="isExpanded(folder.path)" class="ms-3 border-s border-default ps-2">
                <li v-for="item in folder.exercises" :key="item.exercise.id">
                  <UButton
                    type="button"
                    variant="ghost"
                    color="neutral"
                    block
                    :leading-icon="
                      item.exercise.solutionAvailable ? 'i-lucide-file-code-2' : 'i-lucide-lock'
                    "
                    class="min-h-8 justify-start text-start"
                    :disabled="disabled || !item.exercise.solutionAvailable"
                    :aria-current="item.exercise.id === selectedSolution ? 'page' : undefined"
                    :aria-label="`${item.filename}, ${item.exercise.solutionAvailable ? 'Solution available' : 'Solution locked'}`"
                    @click="emit('selectSolution', item.exercise.id)"
                  >
                    <span class="min-w-0 truncate font-mono text-sm">{{ item.filename }}</span>
                  </UButton>
                </li>
              </ul>
            </li>
          </ul>
        </li>
      </ul>

      <p
        v-if="
          normalizedQuery &&
          visibleExerciseFolders.length === 0 &&
          visibleSolutionFolders.length === 0
        "
        class="p-3 text-sm text-muted"
        role="status"
      >
        No files found.
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
