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
    </dl>
  </div>
</template>

<script setup lang="ts">
import type { Stream } from '@/types'
import { formatDateTime } from '@/utils/format'

interface Props {
  stream: Stream
}
defineProps<Props>()

function formatBitrate(bps: number): string {
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(1)} Mbps`
  if (bps >= 1_000) return `${(bps / 1_000).toFixed(1)} Kbps`
  return `${bps} bps`
}
</script>
