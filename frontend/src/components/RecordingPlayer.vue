<template>
  <Teleport to="body">
    <div
      class="fixed inset-0 z-[100] flex items-center justify-center bg-black/80 backdrop-blur-sm p-4"
      @click.self="$emit('close')"
    >
      <div class="w-full max-w-5xl rounded-xl border border-border-default bg-bg-surface overflow-hidden shadow-2xl">
        <div class="flex items-center justify-between px-4 py-3 border-b border-border-default">
          <h3 class="text-sm font-semibold text-text-primary truncate">{{ recording.filename }}</h3>
          <button
            class="rounded-lg p-1.5 text-text-muted hover:text-text-primary hover:bg-bg-elevated transition"
            @click="$emit('close')"
          >
            <svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
        <div class="relative w-full bg-black" style="padding-bottom: 56.25%;">
          <video
            ref="videoRef"
            class="absolute inset-0 w-full h-full"
            :src="recording.url"
            controls
            autoplay
            playsinline
          ></video>
        </div>
        <div class="px-4 py-3 border-t border-border-default flex items-center justify-between text-xs text-text-secondary">
          <div class="flex items-center gap-3">
            <span>Stream: <span class="text-text-primary font-mono">{{ recording.stream_key }}</span></span>
            <span>Recorded: {{ formatDateTime(recording.created_at) }}</span>
            <span v-if="recording.duration_seconds">Duration: {{ formatDuration(recording.duration_seconds) }}</span>
          </div>
          <a
            :href="recording.url"
            download
            class="inline-flex items-center gap-1 rounded-lg bg-bg-elevated px-3 py-1.5 font-medium text-text-secondary border border-border-default hover:bg-bg-overlay hover:text-text-primary transition"
          >
            <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
            </svg>
            Download
          </a>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<script setup lang="ts">
import { onMounted, onUnmounted } from 'vue'
import type { Recording } from '@/types'
import { formatDateTime, formatDuration } from '@/utils/format'

interface Props {
  recording: Recording
}
defineProps<Props>()

defineEmits<{
  close: []
}>()

onMounted(() => {
  document.body.style.overflow = 'hidden'
})

onUnmounted(() => {
  document.body.style.overflow = ''
})
</script>
