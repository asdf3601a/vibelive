<template>
  <span
    class="inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium border"
    :class="[
      status === 'live'
        ? 'bg-accent-live/10 text-accent-live border-accent-live/20'
        : 'bg-bg-elevated text-text-muted border-border-default',
    ]"
  >
    <span
      v-if="status === 'live'"
      class="h-1.5 w-1.5 rounded-full bg-accent-live animate-pulse"
    ></span>
    {{ status === 'live' ? 'LIVE' : 'Ended' }}
  </span>
</template>

<script setup lang="ts">
interface Props {
  status: 'live' | 'ended'
}
defineProps<Props>()
</script>
