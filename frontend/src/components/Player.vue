<template>
  <div class="relative w-full overflow-hidden rounded-xl bg-black border border-border-default">
    <div class="w-full relative" style="padding-bottom: 56.25%;">
      <video
        ref="videoRef"
        class="absolute inset-0 w-full h-full object-contain bg-black"
        :autoplay="autoplay"
        :controls="controls"
        :muted="muted"
        playsinline
      ></video>
    </div>

    <div
      v-if="state === 'waiting'"
      class="absolute inset-0 flex flex-col items-center justify-center bg-bg-base/80 text-text-secondary"
    >
      <svg class="h-8 w-8 mb-2 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
        <path stroke-linecap="round" stroke-linejoin="round" d="M15 10l4.553-4.553A1 1 0 0121 6.12V17.88a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
      </svg>
      <span class="text-sm">Waiting for stream...</span>
    </div>

    <div
      v-else-if="state === 'error'"
      class="absolute inset-0 flex flex-col items-center justify-center bg-bg-base/80 text-text-secondary"
    >
      <svg class="h-8 w-8 mb-2 text-accent-live" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
        <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
      </svg>
      <span class="text-sm mb-3">Playback error</span>
      <BaseButton variant="secondary" @click="retry">
        <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
        </svg>
        Retry
      </BaseButton>
    </div>

    <div
      v-else-if="state === 'loading'"
      class="absolute inset-0 flex items-center justify-center bg-bg-base/60"
    >
      <svg class="h-8 w-8 text-text-muted animate-spin" fill="none" viewBox="0 0 24 24">
        <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
        <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>
      </svg>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, computed } from 'vue'
import Hls from 'hls.js'
import BaseButton from '@/components/ui/BaseButton.vue'

type PlayerState = 'waiting' | 'loading' | 'playing' | 'error'

interface Props {
  src: string | null
  autoplay?: boolean
  controls?: boolean
  muted?: boolean
}

const props = withDefaults(defineProps<Props>(), {
  autoplay: true,
  controls: true,
  muted: true,
})

const videoRef = ref<HTMLVideoElement | null>(null)
const internalState = ref<PlayerState>('waiting')
let hlsInstance: Hls | null = null

const state = computed<PlayerState>(() => {
  if (!props.src) return 'waiting'
  return internalState.value
})

function destroyPlayer() {
  if (hlsInstance) {
    hlsInstance.destroy()
    hlsInstance = null
  }
  if (videoRef.value) {
    videoRef.value.pause()
    videoRef.value.removeAttribute('src')
    videoRef.value.load()
  }
}

function setupPlayer() {
  if (!props.src || !videoRef.value) {
    internalState.value = 'waiting'
    return
  }
  if (hlsInstance) {
    hlsInstance.destroy()
    hlsInstance = null
    videoRef.value.removeAttribute('src')
    videoRef.value.load()
  }
  internalState.value = 'loading'

  if (Hls.isSupported()) {
    hlsInstance = new Hls({
      enableWorker: true,
      lowLatencyMode: true,
    })

    hlsInstance.on(Hls.Events.ERROR, (_event, data) => {
      if (data.fatal) {
        internalState.value = 'error'
      }
    })

    hlsInstance.on(Hls.Events.MANIFEST_PARSED, () => {
      internalState.value = 'playing'
    })

    hlsInstance.loadSource(props.src)
    hlsInstance.attachMedia(videoRef.value)
  } else if (videoRef.value.canPlayType('application/vnd.apple.mpegurl')) {
    videoRef.value.src = props.src
    videoRef.value.addEventListener('loadedmetadata', () => {
      internalState.value = 'playing'
    })
    videoRef.value.addEventListener('error', () => {
      internalState.value = 'error'
    })
    internalState.value = 'loading'
  } else {
    internalState.value = 'error'
  }
}

function retry() {
  internalState.value = 'waiting'
  setupPlayer()
}

watch(() => props.src, (newSrc, oldSrc) => {
  if (newSrc === oldSrc) return
  if (props.src) {
    setupPlayer()
  } else {
    destroyPlayer()
    internalState.value = 'waiting'
  }
})

onMounted(() => {
  if (props.src) setupPlayer()
})
onUnmounted(destroyPlayer)
</script>
