<script setup lang="ts">
import UButton from "@nuxt/ui/components/Button.vue";

defineProps<{
  message?: string | null;
  retrying: boolean;
}>();

const emit = defineEmits<{
  retry: [];
}>();
</script>

<template>
  <section
    class="flex min-h-64 flex-col items-start justify-center gap-4 overflow-auto bg-default p-6"
    aria-labelledby="toolchain-title"
  >
    <div class="space-y-2">
      <h3 id="toolchain-title" class="text-xl font-semibold text-highlighted">
        Rust toolchain required
      </h3>
      <p class="max-w-prose text-sm text-toned">
        Install Cargo, rustc 1.88 or newer, and Clippy, then retry. Your saved answers are
        unchanged.
      </p>
      <pre v-if="message" class="plain-text text-error">{{ message }}</pre>
    </div>
    <UButton
      type="button"
      label="Retry toolchain check"
      class="min-h-8"
      :loading="retrying"
      @click="emit('retry')"
    />
  </section>
</template>
