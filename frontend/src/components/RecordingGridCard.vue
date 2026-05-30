<template>
  <BaseCard hoverable class="group">
    <!-- Preview -->
    <div class="relative w-full bg-bg-base cursor-pointer overflow-hidden" style="padding-bottom: 56.25%;" @click="$emit('play', recording)">
      <img
        v-if="thumbnailSrc && !thumbnailError"
        :src="thumbnailSrc"
        class="absolute inset-0 h-full w-full object-cover transition group-hover:scale-105"
        loading="lazy"
        @error="thumbnailError = true"
      />
      <div v-else class="absolute inset-0 flex items-center justify-center bg-bg-elevated">
        <svg class="h-10 w-10 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
          <path stroke-linecap="round" stroke-linejoin="round" d="M15 10l4.553-4.553A1 1 0 0121 6.12V17.88a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
        </svg>
      </div>

      <!-- Play overlay -->
      <div class="absolute inset-0 flex items-center justify-center bg-black/30 opacity-0 group-hover:opacity-100 transition">
        <div class="h-12 w-12 rounded-full bg-white/90 flex items-center justify-center">
          <svg class="h-6 w-6 text-bg-base ml-0.5" fill="currentColor" viewBox="0 0 24 24">
            <path d="M8 5v14l11-7z" />
          </svg>
        </div>
      </div>

      <!-- Duration badge -->
      <div v-if="recording.duration_seconds" class="absolute bottom-2 right-2 bg-black/70 text-white text-[11px] px-1.5 py-0.5 rounded font-mono">
        {{ formatDuration(recording.duration_seconds) }}
      </div>
    </div>

    <!-- Info -->
    <div class="p-3">
      <h3 class="font-semibold text-text-primary truncate">{{ recording.stream_key }}</h3>
      <p class="text-xs text-text-muted mt-1">
        {{ formatDateTime(recording.created_at) }}
      </p>
      <div class="mt-2 flex items-center justify-between">
        <span class="text-xs text-text-muted">{{ formatFileSize(recording.size_bytes) }}</span>
        <div class="flex items-center gap-1.5">
          <button
            class="inline-flex items-center gap-1 rounded-lg bg-accent-primary px-2.5 py-1 text-[11px] font-medium text-white hover:bg-accent-primary/90 transition"
            @click="$emit('play', recording)"
          >
            <svg class="h-3 w-3" fill="currentColor" viewBox="0 0 24 24">
              <path d="M8 5v14l11-7z" />
            </svg>
            Play
          </button>
          <button
            class="inline-flex items-center gap-1 rounded-lg px-2 py-1 text-[11px] font-medium border border-border-default transition"
            :class="shareCopied
              ? 'text-accent-success border-accent-success/30 bg-accent-success/10'
              : 'text-text-secondary bg-bg-elevated hover:bg-bg-overlay hover:text-text-primary'"
            @click="shareLink"
          >
            <svg v-if="shareCopied" class="h-3 w-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
            </svg>
            <svg v-else class="h-3 w-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M13.828 10.172a4 4 0 00-5.656 0l-4 4a4 4 0 105.656 5.656l1.102-1.101m-.758-4.899a4 4 0 005.656 0l4-4a4 4 0 00-5.656-5.656l-1.1 1.1" />
            </svg>
            {{ shareCopied ? 'Copied' : 'Share' }}
          </button>
          <a
            :href="recording.url"
            download
            class="inline-flex items-center rounded-lg bg-bg-elevated px-2 py-1 text-text-secondary border border-border-default hover:bg-bg-overlay hover:text-text-primary transition"
          >
            <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
            </svg>
          </a>
        </div>
      </div>
    </div>
  </BaseCard>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Recording } from '@/types'
import BaseCard from '@/components/ui/BaseCard.vue'
import { formatDateTime, formatDuration, formatFileSize } from '@/utils/format'
import { copyToClipboard } from '@/utils/clipboard'

interface Props {
  recording: Recording
}
const props = defineProps<Props>()

defineEmits<{
  play: [recording: Recording]
}>()

const thumbnailError = ref(false)
const shareCopied = ref(false)

const thumbnailSrc = computed(() => {
  if (thumbnailError.value) return ''
  return props.recording.thumbnails['480']
    || props.recording.thumbnails['320']
    || props.recording.thumbnail_url
    || ''
})

async function shareLink() {
  const url = `${window.location.origin}/recordings?play=${encodeURIComponent(props.recording.filename)}`
  const ok = await copyToClipboard(url)
  if (ok) {
    shareCopied.value = true
    setTimeout(() => { shareCopied.value = false }, 2000)
  }
}
</script>