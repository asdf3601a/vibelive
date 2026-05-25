<template>
  <div class="flex flex-col items-center justify-center rounded-xl border border-dashed border-border-default bg-bg-surface/50 p-12 text-center">
    <div class="mx-auto mb-4 flex h-14 w-14 items-center justify-center rounded-full bg-bg-elevated">
      <slot name="icon">
        <svg class="h-6 w-6 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
          <path stroke-linecap="round" stroke-linejoin="round" d="M15 10l4.553-4.553A1 1 0 0121 6.12V17.88a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
        </svg>
      </slot>
    </div>
    <h3 class="text-base font-semibold text-text-primary">{{ title }}</h3>
    <p class="mt-1 text-sm text-text-secondary">{{ description }}</p>
    <div v-if="$slots.action" class="mt-4">
      <slot name="action" />
    </div>
  </div>
</template>

<script setup lang="ts">
interface Props {
  title: string
  description: string
}
defineProps<Props>()
</script>
