<template>
  <div class="rounded-xl border border-border-default bg-bg-surface/60 p-4">
    <h3 class="text-sm font-semibold text-text-primary mb-3">Stream Info</h3>
    <dl class="space-y-2 text-sm">
      <div class="flex justify-between gap-2">
        <dt class="text-text-muted shrink-0">Stream Key</dt>
        <dd class="text-text-primary font-mono truncate">{{ stream.stream_key }}</dd>
      </div>
      <div class="flex justify-between gap-2">
        <dt class="text-text-muted shrink-0">Status</dt>
        <dd class="text-accent-live">{{ stream.status }}</dd>
      </div>
      <div v-if="stream.started_at" class="flex justify-between gap-2">
        <dt class="text-text-muted shrink-0">Started</dt>
        <dd class="text-text-primary truncate">{{ formatDateTime(stream.started_at) }}</dd>
      </div>
      <template v-if="stream.metadata">
        <div class="flex justify-between gap-2">
          <dt class="text-text-muted shrink-0">Resolution</dt>
          <dd class="text-text-primary">{{ stream.metadata.width }}×{{ stream.metadata.height }}</dd>
        </div>
        <div v-if="stream.metadata.video_codec" class="flex justify-between gap-2">
          <dt class="text-text-muted shrink-0">Video</dt>
          <dd class="text-text-primary truncate">{{ stream.metadata.video_codec }}</dd>
        </div>
        <div v-if="stream.metadata.audio_codec" class="flex justify-between gap-2">
          <dt class="text-text-muted shrink-0">Audio</dt>
          <dd class="text-text-primary truncate">{{ stream.metadata.audio_codec }}</dd>
        </div>
        <div v-if="stream.metadata.framerate" class="flex justify-between gap-2">
          <dt class="text-text-muted shrink-0">Framerate</dt>
          <dd class="text-text-primary">{{ stream.metadata.framerate }} fps</dd>
        </div>
        <div v-if="stream.metadata.video_bitrate" class="flex justify-between gap-2">
          <dt class="text-text-muted shrink-0">Video Bitrate</dt>
          <dd class="text-text-primary">{{ formatBitrate(stream.metadata.video_bitrate) }}</dd>
        </div>
        <div v-if="stream.metadata.audio_bitrate" class="flex justify-between gap-2">
          <dt class="text-text-muted shrink-0">Audio Bitrate</dt>
          <dd class="text-text-primary">{{ formatBitrate(stream.metadata.audio_bitrate) }}</dd>
        </div>
      </template>

      <!-- Share link -->
      <div v-if="shareUrl" class="pt-2 border-t border-border-default mt-2">
        <dt class="text-text-muted shrink-0 mb-1">Share</dt>
        <div class="flex items-center gap-2">
          <input
            readonly
            :value="shareUrl"
            class="flex-1 min-w-0 bg-bg-base border border-border-default rounded px-2 py-1 text-xs text-text-primary font-mono truncate"
          />
          <button
            class="shrink-0 inline-flex items-center gap-1 rounded-lg bg-bg-elevated px-2 py-1 text-xs font-medium text-text-secondary border border-border-default hover:bg-bg-overlay hover:text-text-primary transition"
            @click="copyShareUrl"
          >
            <svg v-if="copied" class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
            </svg>
            <svg v-else class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
            </svg>
            {{ copied ? 'Copied' : 'Copy' }}
          </button>
        </div>
      </div>
    </dl>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Stream } from '@/types'
import { formatDateTime } from '@/utils/format'

interface Props {
  stream: Stream
}
const props = defineProps<Props>()

const shareUrl = computed(() => {
  if (!props.stream.player_url) return ''
  return window.location.origin + props.stream.player_url
})

const copied = ref(false)

function copyShareUrl() {
  if (!shareUrl.value) return
  navigator.clipboard.writeText(shareUrl.value).then(() => {
    copied.value = true
    setTimeout(() => { copied.value = false }, 2000)
  })
}

function formatBitrate(bps: number): string {
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(1)} Mbps`
  if (bps >= 1_000) return `${(bps / 1_000).toFixed(1)} Kbps`
  return `${bps} bps`
}
</script>
