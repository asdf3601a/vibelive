<template>
  <div class="flex flex-col items-center justify-center rounded-xl border border-dashed border-accent-live/30 bg-accent-live/5 p-12 text-center">
    <div class="mx-auto mb-4 flex h-14 w-14 items-center justify-center rounded-full bg-accent-live/10">
      <svg class="h-6 w-6 text-accent-live" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
        <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
      </svg>
    </div>
    <h3 class="text-base font-semibold text-text-primary">{{ title }}</h3>
    <p class="mt-1 text-sm text-text-secondary">{{ description }}</p>
    <BaseButton v-if="onRetry" variant="secondary" class="mt-4" @click="onRetry">
      <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
        <path stroke-linecap="round" stroke-linejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
      </svg>
      Retry
    </BaseButton>
  </div>
</template>

<script setup lang="ts">
import BaseButton from './BaseButton.vue'

interface Props {
  title: string
  description: string
  onRetry?: () => void
}
defineProps<Props>()
</script>
