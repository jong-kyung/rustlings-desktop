<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { Disposable, RustEditorInstance, RustModel, RustMarker } from "../monaco/setup";

interface DiagnosticBatch {
  exerciseId: string;
  sourceDigest: string;
  modelVersion: number;
  markers: readonly RustMarker[];
}

const props = defineProps<{
  exerciseId: string;
  source: string;
  sourceDigest: string;
  diagnostics?: DiagnosticBatch;
}>();

const emit = defineEmits<{
  change: [source: string, modelVersion: number];
  run: [source: string, modelVersion: number];
}>();

const host = ref<HTMLElement>();
let active = false;
let loadGeneration = 0;
let composing = false;
let setup: typeof import("../monaco/setup") | undefined;
let model: RustModel | undefined;
let editor: RustEditorInstance | undefined;
let observer: ResizeObserver | undefined;
let disposables: Disposable[] = [];
let markerOwner = "";

function clearMarkers() {
  if (setup && model && markerOwner) setup.setMarkers(model, markerOwner, []);
}

function focusRange(range: RustMarker["range"]) {
  if (setup && editor) setup.focusRange(editor, range);
}

defineExpose({ focusRange });

function applyDiagnostics() {
  if (!setup || !model) return;
  clearMarkers();
  const diagnostics = props.diagnostics;
  if (
    diagnostics?.exerciseId === props.exerciseId &&
    diagnostics.sourceDigest === props.sourceDigest &&
    diagnostics.modelVersion === model.getVersionId()
  ) {
    setup.setMarkers(model, markerOwner, diagnostics.markers);
  }
}

function disposeEditor() {
  observer?.disconnect();
  observer = undefined;
  for (const disposable of disposables.splice(0).reverse()) disposable.dispose();
  clearMarkers();
  editor?.dispose();
  model?.dispose();
  editor = undefined;
  model = undefined;
  markerOwner = "";
  composing = false;
}

async function createExerciseEditor(generation: number) {
  const loadedSetup = await import("../monaco/setup");
  if (!active || generation !== loadGeneration || !host.value) return;

  setup = loadedSetup;
  model = setup.createModel(props.source, props.exerciseId);
  editor = setup.createEditor(host.value, model, `${props.exerciseId} Rust source editor`);
  markerOwner = `rustlings-diagnostics:${props.exerciseId}`;

  disposables = [
    model.onDidChangeContent(() => {
      clearMarkers();
      if (model) emit("change", model.getValue(), model.getVersionId());
    }),
    editor.onDidCompositionStart(() => {
      composing = true;
    }),
    editor.onDidCompositionEnd(() => {
      composing = false;
    }),
    setup.addRunCommand(editor, () => {
      if (!composing && model) emit("run", model.getValue(), model.getVersionId());
    }),
  ];

  observer = new ResizeObserver(() => editor?.layout());
  observer.observe(host.value);
  applyDiagnostics();
}

function startExerciseEditor() {
  const generation = ++loadGeneration;
  void createExerciseEditor(generation);
}

watch(
  () => props.exerciseId,
  () => {
    loadGeneration += 1;
    disposeEditor();
    if (active) startExerciseEditor();
  },
);

watch(
  () => props.source,
  (source) => {
    if (model && model.getValue() !== source) model.setValue(source);
  },
);

watch([() => props.sourceDigest, () => props.diagnostics], applyDiagnostics);

onMounted(() => {
  active = true;
  startExerciseEditor();
});

onBeforeUnmount(() => {
  active = false;
  loadGeneration += 1;
  disposeEditor();
});
</script>

<template>
  <section class="rust-editor" aria-label="Rust source editor">
    <div ref="host" class="rust-editor__host" />
  </section>
</template>

<style scoped>
.rust-editor {
  inline-size: 100%;
  max-inline-size: 100%;
  min-inline-size: 0;
  min-block-size: 16rem;
  block-size: 100%;
  overflow: hidden;
}

.rust-editor:focus-within {
  outline: 2px solid var(--ui-primary);
  outline-offset: 2px;
}

.rust-editor__host {
  inline-size: 100%;
  block-size: 100%;
  min-block-size: inherit;
}

@media (forced-colors: active) {
  .rust-editor:focus-within {
    outline-color: CanvasText;
  }
}
</style>
