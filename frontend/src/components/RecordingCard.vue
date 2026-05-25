<template>
  <div class="flex items-center justify-between gap-3 rounded-lg border border-border-default bg-bg-surface/60 px-4 py-3 hover:bg-bg-surface transition">
    <div class="min-w-0">
      <p class="text-sm font-medium text-text-primary truncate">{{ recording.filename }}</p>
      <p class="text-xs text-text-muted mt-0.5">
        {{ formatDate(recording.created_at) }} · {{ formatSize(recording.size_bytes) }}
      </p>
    </div>
    <a
      :href="recording.url"
      download
      class="inline-flex shrink-0 items-center gap-1 rounded-lg bg-bg-elevated px-3 py-1.5 text-xs font-medium text-text-secondary border border-border-default hover:bg-bg-overlay hover:text-text-primary transition"
    >
      <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
        <path stroke-linecap="round" stroke-linejoin="round" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
      </svg>
      Download
    </a>
  </div>
</template>

<script setup lang="ts">
import type { Recording } from '@/types'

interface Props {
  recording: Recording
}
defineProps<Props>()

function formatDate(d: string): string {
  return new Date(d).toLocaleString()
}

function formatSize(bytes: number): string {
  if (bytes >= 1_073_741_824) return `${(bytes / 1_073_741_824).toFixed(1)} GB`
  if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(1)} MB`
  if (bytes >= 1_024) return `${(bytes / 1_024).toFixed(1)} KB`
  return `${bytes} B`
}
</script>
