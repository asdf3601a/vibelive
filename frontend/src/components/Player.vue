<template>
  <div
    ref="containerRef"
    class="relative w-full rounded-xl bg-black border border-border-default select-none"
    :class="{ 'cursor-none': !controlsVisible && isPlaying }"
    @mouseenter="onMouseMove"
    @mousemove="onMouseMove"
    @mouseleave="hideControls"
    @keydown="handleKeydown"
    @touchstart="handleTouchStart"
    @touchend="handleTouchEnd"
    tabindex="0"
  >
    <!-- Video element (clipped to rounded corners) -->
    <div class="w-full relative overflow-hidden rounded-[inherit]" style="padding-bottom: 56.25%;">
      <video
        ref="videoRef"
        class="absolute inset-0 w-full h-full object-contain bg-black"
        :autoplay="autoplay"
        :poster="poster"
        playsinline
        preload="auto"
      ></video>

      <!-- overlays inside video area -->

      <!-- Buffering spinner -->
      <div
        v-if="state === 'loading' || state === 'buffering'"
        class="absolute inset-0 flex items-center justify-center bg-black/40"
      >
        <svg class="h-10 w-10 text-white/60 animate-spin" fill="none" viewBox="0 0 24 24">
          <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="3" />
          <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
        </svg>
      </div>

      <!-- Error overlay -->
      <div
        v-if="state === 'error'"
        class="absolute inset-0 flex flex-col items-center justify-center bg-black/60"
      >
        <svg class="h-10 w-10 mb-2 text-accent-live" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
          <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
        </svg>
        <span class="text-sm text-white/80 mb-3">Playback error</span>
        <button
          class="inline-flex items-center gap-1.5 rounded-lg bg-white/20 px-3 py-1.5 text-sm font-medium text-white hover:bg-white/30 transition"
          @click="loadSource(src, isLive)"
        >
          <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
          </svg>
          Retry
        </button>
      </div>

      <!-- Ended overlay -->
      <div
        v-if="state === 'ended'"
        class="absolute inset-0 flex items-center justify-center bg-black/40"
      >
        <button
          class="h-16 w-16 rounded-full bg-white/90 flex items-center justify-center hover:bg-white transition shadow-xl hover:scale-105 active:scale-95"
          @click="togglePlay"
        >
          <svg class="h-7 w-7 text-bg-base ml-1" fill="currentColor" viewBox="0 0 24 24">
            <path d="M8 5v14l11-7z" />
          </svg>
        </button>
      </div>

      <!-- Big center play button -->
      <Transition name="fade">
        <div
          v-if="!isPlaying && state !== 'loading' && state !== 'buffering' && state !== 'ended' && state !== 'error'"
          class="absolute inset-0 flex items-center justify-center cursor-pointer"
          @click="togglePlay"
          @touchend.stop
        >
          <div class="h-16 w-16 rounded-full bg-accent-primary/90 flex items-center justify-center hover:bg-accent-primary transition shadow-xl hover:scale-105 active:scale-95">
            <svg class="h-7 w-7 text-white ml-1" fill="currentColor" viewBox="0 0 24 24">
              <path d="M8 5v14l11-7z" />
            </svg>
          </div>
        </div>
      </Transition>
    </div>

    <!-- Top gradient -->
    <div class="absolute top-0 left-0 right-0 h-16 bg-gradient-to-b from-black/50 to-transparent pointer-events-none" :class="controlsVisible ? 'opacity-100' : 'opacity-0'" />

    <!-- Seek indicator -->
      <Transition name="seek-indicator">
        <div
          v-if="seekIndicator"
          class="absolute top-1/2 -translate-y-1/2 z-20 pointer-events-none"
          :class="seekIndicator.dir === 'forward' ? 'right-8' : 'left-8'"
        >
          <div
            class="flex items-center gap-2 bg-black/70 backdrop-blur rounded-xl px-4 py-2.5 shadow-2xl"
            :class="seekIndicator.dir === 'forward' ? 'flex-row' : 'flex-row-reverse'"
          >
            <svg class="h-5 w-5 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path v-if="seekIndicator.dir === 'forward'" stroke-linecap="round" stroke-linejoin="round" d="M13 5l7 7-7 7M5 5l7 7-7 7" />
              <path v-else stroke-linecap="round" stroke-linejoin="round" d="M11 19l-7-7 7-7m8 14l-7-7 7-7" />
            </svg>
            <span class="text-white font-semibold text-lg tabular-nums">{{ seekIndicator.dir === 'forward' ? '+' : '-' }}{{ seekIndicator.amount }}s</span>
          </div>
        </div>
      </Transition>

      <!-- Debug overlay -->
    <DebugOverlay
      v-if="showDebug"
      :current-time="currentTime"
      :duration="progressBarDuration"
      :video-width="videoWidth"
      :video-height="videoHeight"
      :volume="volume"
      :is-muted="isMuted"
      :playback-rate="playbackRate"
      :dropped-frames="droppedFrames"
      :state="state"
      :hls-bandwidth-estimate="hlsBandwidthEstimate"
      :hls-buffer-length="hlsBufferLength"
      :hls-live-latency="hlsLiveLatency"
      :hls-current-level="currentHlsLevel"
      :hls-level-width="hlsLevelWidth"
      :hls-level-height="hlsLevelHeight"
      :hls-level-bitrate="hlsLevelBitrate"
      :active-track-id="activeTrackId"
      :tracks="tracks"
    />

    <!-- Bottom controls -->
    <Transition name="controls">
      <div
        v-if="controlsVisible"
        class="absolute bottom-0 left-0 right-0"
      >
        <!-- Bottom gradient -->
        <div class="absolute bottom-0 left-0 right-0 h-24 bg-gradient-to-t from-black/60 to-transparent pointer-events-none" />

        <div class="relative z-10 px-3 pb-2.5 pt-10">
          <!-- Progress bar -->
          <div class="mb-2">
            <ProgressBar
              :progress="progress"
              :duration="progressBarDuration"
              :current-time="currentTime"
              :buffered="buffered"
              :buffered-end="bufferedEnd"
              :live-start="isLive ? liveStart : undefined"
              :loop-a="loopA"
              :loop-b="loopB"
              :loop-active="loopA !== null && loopB !== null"
              @seek="seekTo"
            />
          </div>

          <!-- Controls row -->
          <div class="flex items-center gap-1">
            <!-- Play/Pause -->
            <button
              class="p-1.5 rounded-lg text-white/80 hover:text-white hover:bg-white/10 transition cursor-pointer"
              :title="isPlaying ? 'Pause (Space)' : 'Play (Space)'"
              @click="togglePlay"
            >
              <svg v-if="isPlaying" class="h-5 w-5" fill="currentColor" viewBox="0 0 24 24">
                <path d="M6 4h4v16H6V4zm8 0h4v16h-4V4z" />
              </svg>
              <svg v-else class="h-5 w-5" fill="currentColor" viewBox="0 0 24 24">
                <path d="M8 5v14l11-7z" />
              </svg>
            </button>

            <!-- Time display -->
            <div class="text-[11px] font-mono text-white/70 mr-1 whitespace-nowrap tabular-nums flex items-center gap-1">
              <span v-if="isLive">
                <button
                  class="flex items-center gap-1.5"
                  :title="isBehind ? `Behind by ${Math.round(liveEdge - currentTime)}s — click to catch up` : 'Live'"
                  @click="seekToLiveEdge"
                >
                  <span
                    class="h-2 w-2 rounded-full shrink-0 transition-colors"
                    :class="isBehind ? 'bg-text-muted' : 'bg-accent-live animate-pulse'"
                  />
                  <span>{{ formattedCurrentTime }}</span>
                </button>
              </span>
              <span v-else>
                {{ formattedCurrentTime }} / {{ formattedDuration }}
              </span>
            </div>

            <!-- Speed badge -->
            <button
              v-if="playbackRate !== 1"
              class="px-1.5 py-0.5 rounded text-[10px] font-mono font-medium bg-white/10 text-white/70 hover:bg-white/20 transition cursor-pointer shrink-0 mr-0.5"
              title="Reset speed to 1x"
              @click="setPlaybackRate(1)"
              @wheel.prevent="onSpeedWheel"
            >{{ playbackRate }}x</button>

            <!-- Volume -->
            <VolumeControl
              :volume="volume"
              :volume-stage="volumeStage"
              :is-volume-boosted="isVolumeBoosted"
              :volume-boost-enabled="volumeBoostEnabled"
              :is-muted="isMuted"
              @toggle-mute="toggleMute"
              @set-volume="setVolume"
            />

            <!-- Spacer -->
            <div class="flex-1" />

            <!-- Quality / Track (right side, only when multitrack) -->
            <TrackSelector
              :tracks="tracks"
              :active-track-id="activeTrackId"
              @select-track="handleTrackSelect"
            />

            <!-- Settings (right side) -->
            <SettingsMenu
              :playback-rate="playbackRate"
              :loop-a="loopA"
              :loop-b="loopB"
              :loop-enabled="loopEnabled"
              :show-debug="showDebug"
              :is-live="isLive"
              :live-threshold="liveThreshold"
              :volume-boost-enabled="volumeBoostEnabled"
              @set-playback-rate="setPlaybackRate"
              @set-loop-a="setLoopA"
              @set-loop-b="setLoopB"
              @set-loop-enabled="setLoopEnabled"
              @clear-loop="clearLoop"
              @toggle-debug="showDebug = !showDebug"
              @set-live-threshold="setLiveThreshold"
              @toggle-volume-boost="toggleVolumeBoost"
            />

            <!-- Fullscreen -->
            <button
              class="p-1.5 rounded-lg text-white/80 hover:text-white hover:bg-white/10 transition cursor-pointer"
              title="Fullscreen (f)"
              @click="toggleFullscreen"
            >
              <svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
                <path stroke-linecap="round" stroke-linejoin="round" d="M4 8V4m0 0h4M4 4l5 5m11-1V4m0 0h-4m4 0l-5 5M4 16v4m0 0h4m-4 0l5-5m11 5l-5-5m5 5v-4m0 4h-4" />
              </svg>
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted } from 'vue'
import type { TrackInfo } from '@/types'
import { usePlayer } from '@/composables/usePlayer'
import ProgressBar from '@/components/player/ProgressBar.vue'
import VolumeControl from '@/components/player/VolumeControl.vue'
import SettingsMenu from '@/components/player/SettingsMenu.vue'
import TrackSelector from '@/components/player/TrackSelector.vue'
import DebugOverlay from '@/components/player/DebugOverlay.vue'

interface Props {
  src: string | null
  autoplay?: boolean
  muted?: boolean
  poster?: string
  tracks?: TrackInfo[]
  isLive?: boolean
  initialLoopA?: number | null
  initialLoopB?: number | null
  initialLoopEnabled?: boolean
}

const props = withDefaults(defineProps<Props>(), {
  autoplay: true,
  muted: true,
  poster: '',
  tracks: () => [],
  isLive: false,
})

const emit = defineEmits<{
  trackChange: [trackId: number]
  loopUpdate: [data: { loopA: number | null; loopB: number | null; loopEnabled: boolean }]
}>()

const {
  videoRef,
  containerRef,
  state,
  isPlaying,
  currentTime,
  duration,
  volume,
  volumeStage,
  isVolumeBoosted,
  volumeBoostEnabled,
  toggleVolumeBoost,
  isMuted,
  playbackRate,
  buffered,
  videoWidth,
  videoHeight,
  droppedFrames,
  loopA,
  loopB,
  loopEnabled,
  setLoopEnabled,
  showDebug,
  controlsVisible,
  hlsLevels,
currentHlsLevel,
    hlsBandwidthEstimate,
    hlsBufferLength,
    hlsLiveLatency,
    hlsLevelWidth,
    hlsLevelHeight,
    hlsLevelBitrate,
    activeTrack,
    progress,
  progressBarDuration,
  formattedCurrentTime,
  formattedDuration,
  seekIndicator,
  bufferedEnd,
  liveEdge,
  liveThreshold,
  isBehind,
  liveStart,
  seekToLiveEdge,
  setLiveThreshold,
  tracks: playerTracks,
  activeTrackId,
  loadSource,
  play,
  pause,
  togglePlay,
  seekTo,
  setVolume,
  toggleMute,
  setPlaybackRate,
  setLoopA,
  setLoopB,
  clearLoop,
  toggleFullscreen,
  onMouseMove,
  hideControls,
  handleKeydown,
  handleTouchStart,
  handleTouchEnd,
  attachVideoEvents,
  detachVideoEvents,
  destroy,
  requestAutoplayPermission,
} = usePlayer({ enableKeyboard: true })

// Sync tracks from props to composable
watch(() => props.tracks, (val) => {
  playerTracks.value = val
}, { immediate: true })

watch(() => props.src, (val) => {
  loadSource(val, props.isLive)
})

watch(() => props.muted, (val) => {
  if (val) {
    if (videoRef.value) videoRef.value.muted = true
    isMuted.value = true
  } else {
    if (videoRef.value) videoRef.value.muted = false
    isMuted.value = false
  }
}, { immediate: true })

watch(() => props.initialLoopA, (val) => {
  if (val != null) loopA.value = val
}, { immediate: true })

watch(() => props.initialLoopB, (val) => {
  if (val != null) loopB.value = val
}, { immediate: true })

watch(() => props.initialLoopEnabled, (val) => {
  if (val != null) loopEnabled.value = val
}, { immediate: true })

watch([loopA, loopB, loopEnabled], () => {
  emit('loopUpdate', {
    loopA: loopA.value,
    loopB: loopB.value,
    loopEnabled: loopEnabled.value,
  })
})

const SPEED_STEPS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 4, 8, 16]

function getSpeedIndex(rate: number): number {
  const idx = SPEED_STEPS.indexOf(rate)
  return idx >= 0 ? idx : 3
}

function onSpeedWheel(e: WheelEvent) {
  const dir = e.deltaY > 0 ? -1 : 1
  const newIdx = Math.max(0, Math.min(SPEED_STEPS.length - 1, getSpeedIndex(playbackRate.value) + dir))
  setPlaybackRate(SPEED_STEPS[newIdx])
}

function handleTrackSelect(trackId: number) {
  activeTrackId.value = trackId
  emit('trackChange', trackId)
}

onMounted(() => {
  if (videoRef.value) {
    videoRef.value.muted = props.muted
  }
  attachVideoEvents()
  if (props.src) {
    if (props.initialLoopA != null) loopA.value = props.initialLoopA
    if (props.initialLoopB != null) loopB.value = props.initialLoopB
    if (props.initialLoopEnabled != null) loopEnabled.value = props.initialLoopEnabled

    loadSource(props.src, props.isLive, true)
  }
  containerRef.value?.focus()

  // First user interaction → proactively request autoplay permission
  function onInteraction() {
    requestAutoplayPermission()
  }
  containerRef.value?.addEventListener('pointerdown', onInteraction, { once: true })
})

onUnmounted(() => {
  destroy()
})
</script>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.controls-enter-active,
.controls-leave-active {
  transition: opacity 0.2s ease;
}
.controls-enter-from,
.controls-leave-to {
  opacity: 0;
}

.seek-indicator-enter-active {
  transition: all 0.15s ease-out;
}
.seek-indicator-leave-active {
  transition: all 0.4s ease-in;
}
.seek-indicator-enter-from {
  opacity: 0;
  transform: translateY(8px);
}
.seek-indicator-leave-to {
  opacity: 0;
  transform: translateY(-8px) scale(0.95);
}
</style>