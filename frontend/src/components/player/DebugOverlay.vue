<template>
  <div class="absolute top-0 left-0 right-0 z-20 pointer-events-none">
    <div class="bg-black/75 px-2.5 py-1 font-mono text-[9px] text-white/70 flex items-center gap-3 flex-wrap whitespace-nowrap">
      <span>T: <span class="text-white/90">{{ t }}</span></span>
      <span>R: <span class="text-white/90">{{ videoWidth }}×{{ videoHeight }}</span></span>
      <span>V: <span class="text-white/90">{{ vol }}</span></span>
      <span>S: <span class="text-white/90">{{ playbackRate }}x</span></span>
      <span>F: <span class="text-white/90">{{ droppedFrames }}</span></span>
      <span>St: <span class="text-white/90 capitalize">{{ state }}</span></span>
      <span v-if="hlsBw">Bw: <span class="text-white/90">{{ hlsBw }}</span></span>
      <span v-if="hlsBuf">Buf: <span class="text-white/90">{{ hlsBuf }}</span></span>
      <span v-if="hlsLatency !== null">Lag: <span class="text-white/90">{{ hlsLatency }}</span></span>
      <span v-if="hlsLevel">Lvl: <span class="text-white/90">{{ hlsLevel }}</span></span>
      <span v-if="activeTrackLabel">Trk: <span class="text-white/90">{{ activeTrackLabel }}</span></span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { PlayerState } from '@/composables/usePlayer'

interface Props {
  currentTime: number
  duration: number
  videoWidth: number
  videoHeight: number
  volume: number
  isMuted: boolean
  playbackRate: number
  droppedFrames: number
  state: PlayerState
  hlsBandwidthEstimate: number
  hlsBufferLength: number
  hlsLiveLatency: number
  hlsCurrentLevel: number
  hlsLevelWidth: number
  hlsLevelHeight: number
  hlsLevelBitrate: number
  activeTrackId: number
  tracks: { track_id: number; video_codec: string | null }[]
}

const props = defineProps<Props>()

const t = computed(() => {
  const ct = props.currentTime
  const d = props.duration
  if (!isFinite(d) || d <= 0) return `${ct.toFixed(1)}`
  return `${ct.toFixed(1)} / ${d.toFixed(1)}`
})

const vol = computed(() => {
  if (props.isMuted) return 'muted'
  return `${Math.round(props.volume * 100)}%`
})

const hlsBw = computed(() => {
  if (!props.hlsBandwidthEstimate) return ''
  const bps = props.hlsBandwidthEstimate
  return bps >= 1_000_000 ? `${(bps / 1_000_000).toFixed(1)}M` : `${Math.round(bps / 1000)}k`
})

const hlsBuf = computed(() => {
  if (!props.hlsBufferLength) return ''
  return `${props.hlsBufferLength.toFixed(1)}s`
})

const hlsLatency = computed(() => {
  if (props.hlsLiveLatency == null || props.hlsLiveLatency < 0) return null
  return `${props.hlsLiveLatency.toFixed(1)}s`
})

const hlsLevel = computed(() => {
  if (!props.hlsLevelWidth) return ''
  return `${props.hlsLevelWidth}×${props.hlsLevelHeight} ${props.hlsLevelBitrate ? `${(props.hlsLevelBitrate / 1_000_000).toFixed(1)}M` : ''}`
})

const activeTrackLabel = computed(() => {
  const trk = props.tracks.find(t => t.track_id === props.activeTrackId)
  if (!trk) return ''
  return trk.track_id === 0 ? 'Default' : `Track ${trk.track_id}`
})
</script>