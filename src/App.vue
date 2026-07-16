<script setup lang="ts">
import UApp from "@nuxt/ui/components/App.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import UModal from "@nuxt/ui/components/Modal.vue";

const plainTextFixture = '<script>alert("escaped")<' + "/script>";
</script>

<template>
  <UApp :toaster="null">
    <main class="min-h-svh overflow-auto bg-default p-4 text-default sm:p-8">
      <section class="mx-auto flex max-w-3xl flex-col items-start gap-4">
        <div class="space-y-1">
          <h1 class="text-2xl font-semibold text-highlighted">Rustlings Desktop</h1>
          <p class="text-sm text-muted">Frontend foundation ready.</p>
        </div>

        <UModal title="Local text preview" :close="false" :transition="false">
          <UButton type="button" label="Test overlay" class="min-h-6 min-w-6" />

          <template #body>
            <p class="overflow-auto whitespace-pre-wrap break-words" v-text="plainTextFixture" />
          </template>
        </UModal>

        <p role="status" aria-live="polite" class="text-sm text-toned">
          Tailwind CSS and Nuxt UI are available.
        </p>
      </section>
    </main>
  </UApp>
</template>
