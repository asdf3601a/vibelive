<template>
  <div class="flex items-center gap-2 sm:gap-4 rounded-lg border border-border-default bg-bg-surface/60 px-4 py-3 hover:bg-bg-surface transition">
    <!-- Preview thumbnail -->
    <div class="shrink-0 w-24 h-14 rounded-md overflow-hidden bg-bg-base relative group cursor-pointer" @click="$emit('play', recording)">
      <picture v-if="fallbackUrl && !thumbnailError" class="w-full h-full">
        <source :srcset="jxlSrcset" :sizes="thumbnailSizes" type="image/jxl">
        <source :srcset="avifSrcset" :sizes="thumbnailSizes" type="image/avif">
        <img
          :src="fallbackUrl"
          :srcset="pngSrcset"
          :sizes="thumbnailSizes"
          class="w-full h-full object-cover"
          loading="lazy"
          @error="thumbnailError = true"
        />
      </picture>
      <div v-else class="w-full h-full flex items-center justify-center bg-bg-elevated">
        <svg class="h-5 w-5 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
          <path stroke-linecap="round" stroke-linejoin="round" d="M15 10l4.553-4.553A1 1 0 0121 6.12V17.88a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
        </svg>
      </div>
      <div class="absolute inset-0 flex items-center justify-center bg-black/40 opacity-0 group-hover:opacity-100 transition">
        <svg class="h-6 w-6 text-white" fill="currentColor" viewBox="0 0 24 24">
          <path d="M8 5v14l11-7z" />
        </svg>
      </div>
      <div v-if="recording.duration_seconds" class="absolute bottom-1 right-1 bg-black/70 text-white text-[10px] px-1 rounded-md">
        {{ formatDuration(recording.duration_seconds) }}
      </div>
    </div>

    <!-- Info -->
    <div class="min-w-0 flex-1">
      <p class="text-sm font-medium text-text-primary truncate">{{ recording.stream_key }}</p>
      <p class="text-xs text-text-muted mt-0.5">
        {{ formatDateTime(recording.created_at) }}
      </p>
    </div>

    <!-- Meta -->
    <div class="hidden sm:flex flex-col items-end gap-1 text-xs text-text-muted shrink-0">
      <span>{{ formatFileSize(recording.size_bytes) }}</span>
      <span v-if="recording.duration_seconds">{{ formatDuration(recording.duration_seconds) }}</span>
    </div>

    <!-- Actions -->
    <div class="flex items-center gap-2 shrink-0">
      <BaseButton
        variant="primary"
        class="!text-xs"
        @click="$emit('play', recording)"
      >
        <svg class="h-3.5 w-3.5" fill="currentColor" viewBox="0 0 24 24">
          <path d="M8 5v14l11-7z" />
        </svg>
        Play
      </BaseButton>
      <button
        class="hidden sm:inline-flex items-center gap-1 rounded-lg bg-bg-elevated px-2.5 py-1.5 text-xs font-medium border border-border-default transition shrink-0"
        :class="shareCopied
          ? 'text-accent-success border-accent-success/30 bg-accent-success/10'
          : 'text-text-secondary hover:bg-bg-overlay hover:text-text-primary'"
        @click="shareLink"
      >
        <svg v-if="shareCopied" class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
        </svg>
        <svg v-else class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M13.828 10.172a4 4 0 00-5.656 0l-4 4a4 4 0 105.656 5.656l1.102-1.101m-.758-4.899a4 4 0 005.656 0l4-4a4 4 0 00-5.656-5.656l-1.1 1.1" />
        </svg>
        {{ shareCopied ? 'Copied' : 'Share' }}
      </button>
      <a
        :href="recording.url"
        download
        class="hidden sm:inline-flex items-center gap-1 rounded-lg bg-bg-elevated px-3 py-1.5 text-xs font-medium text-text-secondary border border-border-default hover:bg-bg-overlay hover:text-text-primary transition"
      >
        <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
        </svg>
      </a>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Recording } from '@/types'
import { formatDateTime, formatDuration, formatFileSize } from '@/utils/format'
import { copyToClipboard } from '@/utils/clipboard'
import { useThumbnailSrcset } from '@/composables/useThumbnailSrcset'
import BaseButton from '@/components/ui/BaseButton.vue'
interface Props {
  recording: Recording
}
const props = defineProps<Props>()

defineEmits<{
  play: [recording: Recording]
}>()

const thumbnailError = ref(false)
const shareCopied = ref(false)
const thumbnailSizes = '96px'

const thumbnailsRef = computed(() => props.recording.thumbnails)
const { pngSrcset, jxlSrcset, avifSrcset, fallbackUrl } = useThumbnailSrcset(thumbnailsRef)

async function shareLink() {
  const url = `${window.location.origin}/recordings?play=${encodeURIComponent(props.recording.filename)}`
  const ok = await copyToClipboard(url)
  if (ok) {
    shareCopied.value = true
    setTimeout(() => { shareCopied.value = false }, 2000)
  }
}
</script>