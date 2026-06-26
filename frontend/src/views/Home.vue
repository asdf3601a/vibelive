<template>
  <div>
    <div class="flex items-center justify-between mb-6">
      <div>
        <h1 class="text-2xl font-bold text-text-primary">Live Streams</h1>
        <p class="text-sm text-text-secondary mt-1">
          {{ liveCount }} active {{ liveCount === 1 ? 'publisher' : 'publishers' }}
        </p>
      </div>
      <div class="flex items-center gap-2">
        <span class="relative flex h-3 w-3">
          <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-accent-success opacity-75"></span>
          <span class="relative inline-flex rounded-full h-3 w-3 bg-accent-success"></span>
        </span>
        <span class="text-xs font-medium text-accent-success">Polling</span>
      </div>
    </div>

    <!-- Loading skeletons -->
    <div v-if="loading && !displayedData?.length" class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
      <div v-for="i in 3" :key="i">
        <BaseCard hoverable>
          <BaseSkeleton variant="video" />
          <div class="p-4 space-y-3">
            <BaseSkeleton variant="text" class="w-32" />
            <div class="flex gap-2">
              <BaseSkeleton variant="text" class="w-20" />
              <BaseSkeleton variant="text" class="w-16" />
            </div>
          </div>
        </BaseCard>
      </div>
    </div>

    <!-- Error state -->
    <BaseErrorState
      v-else-if="error"
      title="Failed to load streams"
      description="Could not fetch the active stream list. The server may be unreachable."
      :on-retry="refetch"
    />

    <!-- Has data -->
    <template v-else-if="displayedData?.length">
      <!-- Stream setup info -->
      <BaseCard padding :hoverable="false" class="mb-6">
        <div class="flex items-center justify-between cursor-pointer" @click="showSetup = !showSetup">
          <div class="flex items-center gap-2">
            <svg class="h-4 w-4 text-accent-primary" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
            <span class="text-sm font-medium text-text-primary">Stream Setup</span>
            <BaseTag>Multitrack</BaseTag>
          </div>
          <svg class="h-4 w-4 text-text-muted transition" :class="showSetup ? 'rotate-180' : ''" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M19 9l-7 7-7-7" />
          </svg>
        </div>
        <div v-if="showSetup" class="mt-3 space-y-3 text-sm text-text-secondary">
          <div>
            <span class="font-medium">RTMP URL:</span>
            <BaseCodeBlock :text="rtmpUrl" />
          </div>
          <div>
            <span class="font-medium">Supported codecs:</span>
            <span class="ml-1">Video: {{ supportedVideoCodecs.join(', ') }} / Audio: {{ supportedAudioCodecs.join(', ') }}</span>
          </div>
          <div>
            <span class="font-medium">Single-track ffmpeg example:</span>
            <BaseCodeBlock :text="exampleFfmpegSingle" :multiline="true" />
          </div>
          <div>
            <span class="font-medium">Multitrack ffmpeg example:</span>
            <BaseCodeBlock :text="exampleFfmpegMultitrack" :multiline="true" />
          </div>
        </div>
      </BaseCard>

      <TransitionGroup
        name="stream-list"
        tag="div"
        class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4"
      >
        <StreamCard v-for="stream in displayedData" :key="stream.stream_key" :stream="stream" />
      </TransitionGroup>
    </template>

    <!-- Empty state -->
    <BaseEmptyState v-else title="No active streams" description="Start streaming to see it here.">
      <template #icon>
        <svg class="h-6 w-6 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
          <path stroke-linecap="round" stroke-linejoin="round" d="M15 10l4.553-4.553A1 1 0 0121 6.12V17.88a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
        </svg>
      </template>
      <template #action>
        <div class="space-y-2">
          <BaseCodeBlock :text="rtmpUrl" />
        </div>
      </template>
    </BaseEmptyState>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import StreamCard from '@/components/StreamCard.vue'
import BaseCard from '@/components/ui/BaseCard.vue'
import BaseSkeleton from '@/components/ui/BaseSkeleton.vue'
import BaseEmptyState from '@/components/ui/BaseEmptyState.vue'
import BaseErrorState from '@/components/ui/BaseErrorState.vue'
import BaseCodeBlock from '@/components/ui/BaseCodeBlock.vue'
import BaseTag from '@/components/ui/BaseTag.vue'
import { useStreamList } from '@/composables/useStreamList'
import type { Stream } from '@/types'

const { data, error, loading, refetch } = useStreamList()

// Only re-render TransitionGroup when the stream set structurally changes
// (new stream added / existing stream removed), not on field-level metadata changes
const displayedData = ref<Stream[]>([])

watch(
  data,
  (newData) => {
    if (!newData) return
    const oldKeys = new Set(displayedData.value.map(s => s.stream_key))
    const newKeys = new Set(newData.map(s => s.stream_key))
    if (oldKeys.size !== newKeys.size || [...oldKeys].some(k => !newKeys.has(k))) {
      displayedData.value = newData
    } else {
      // Update fields in-place without replacing the array reference
      for (const stream of newData) {
        const idx = displayedData.value.findIndex(s => s.stream_key === stream.stream_key)
        if (idx >= 0) displayedData.value[idx] = stream
      }
    }
  },
  { immediate: true, deep: true },
)

const liveCount = computed(() => displayedData.value?.length ?? 0)

watch(
  () => liveCount.value,
  () => {
    document.title = `LiveStream Platform — ${liveCount.value} live`
  },
  { immediate: true },
)

const hostname = window.location.hostname
const rtmpPort = 1935
const rtmpUrl = `rtmp://${hostname}:${rtmpPort}/live/{stream_key}`
const supportedVideoCodecs = ['H264', 'HEVC', 'AV1']
const supportedAudioCodecs = ['AAC', 'Opus', 'FLAC']
const exampleFfmpegSingle = `ffmpeg -re -f lavfi -i testsrc=duration=30:size=1280x720:rate=30 \\
  -f lavfi -i "sine=frequency=440:duration=30" \\
  -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency \\
  -c:a aac -ar 44100 \\
  -f flv rtmp://${hostname}:${rtmpPort}/live/testkey`
const exampleFfmpegMultitrack = `ffmpeg -re \\
  -f lavfi -i "testsrc=duration=30:size=1280x720:rate=30" \\
  -f lavfi -i "testsrc=duration=30:size=640x360:rate=30" \\
  -f lavfi -i "sine=frequency=440:duration=30" \\
  -f lavfi -i "sine=frequency=880:duration=30" \\
  -map 0:v -c:v:0 libsvtav1 -preset:v:0 12 -pix_fmt:v:0 yuv420p -b:v:0 1500k -g:v:0 60 \\
  -map 1:v -c:v:1 libx264 -preset:v:1 ultrafast -pix_fmt:v:1 yuv420p -b:v:1 500k -g:v:1 60 \\
  -map 2:a -c:a:0 libopus -ar:a:0 48000 -b:a:0 128k \\
  -map 3:a -c:a:1 aac -ar:a:1 44100 -b:a:1 128k \\
  -f flv rtmp://${hostname}:${rtmpPort}/live/testkey`

const showSetup = ref(false)
</script>

<style>
.stream-list-move,
.stream-list-enter-active,
.stream-list-leave-active {
  transition: all 0.3s ease;
}
.stream-list-enter-from,
.stream-list-leave-to {
  opacity: 0;
  transform: translateY(12px);
}
.stream-list-leave-active {
  position: absolute;
}
</style>
