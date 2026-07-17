<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";

defineProps<{
  readme: string;
  hint?: string;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  revealHint: [];
}>();
</script>

<template>
  <aside
    class="scroll-panel border-default bg-default p-4"
    aria-labelledby="lesson-title"
    tabindex="0"
  >
    <h2 id="lesson-title" class="mb-3 font-semibold text-highlighted">Lesson</h2>
    <pre class="plain-text">{{ readme }}</pre>

    <section class="mt-5 border-t border-default pt-4" aria-labelledby="hint-title">
      <div class="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h3 id="hint-title" class="font-medium text-highlighted">Hint</h3>
        <UButton
          v-if="hint === undefined"
          type="button"
          size="sm"
          variant="soft"
          label="Reveal hint"
          :disabled="disabled"
          class="min-h-8"
          @click="emit('revealHint')"
        />
      </div>
      <pre v-if="hint !== undefined" class="plain-text">{{ hint }}</pre>
      <p v-else class="text-sm text-muted">Hidden until you choose to reveal it.</p>
    </section>
  </aside>
</template>
