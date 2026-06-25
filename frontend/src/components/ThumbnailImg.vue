<template>
  <div class="group relative overflow-hidden rounded-xl bg-bg-base border border-border-default">
    <div class="w-full relative" :style="{ paddingBottom: aspectRatioPadding }">
      <!-- Multi-resolution mode (srcset + sizes) -->
      <picture v-if="thumbnails && fallbackUrl && !error">
        <source :srcset="jxlSrcset" :sizes="sizes" type="image/jxl">
        <source :srcset="avifSrcset" :sizes="sizes" type="image/avif">
        <img
          :key="retryKey"
          :src="fallbackUrl"
          :srcset="pngSrcset"
          :sizes="sizes"
          alt="Stream thumbnail"
          class="absolute inset-0 h-full w-full object-cover transition group-hover:scale-105"
          loading="lazy"
          @load="handleLoad"
          @error="handleError"
        />
      </picture>
      <!-- Single-URL mode (backward compatible) -->
      <picture v-else-if="effectiveSrc && !error">
        <source :srcset="singleJxlSrc" type="image/jxl">
        <source :srcset="singleAvifSrc" type="image/avif">
        <img
          :key="retryKey"
          :src="effectiveSrc"
          alt="Stream thumbnail"
          class="absolute inset-0 h-full w-full object-cover transition group-hover:scale-105"
          loading="lazy"
          @load="handleLoad"
          @error="handleError"
        />
      </picture>
      <!-- Loading / Placeholder state -->
      <div v-if="!effectiveSrc || error || loading" class="absolute inset-0 flex items-center justify-center bg-bg-base">
        <div class="text-center px-4">
          <div class="mx-auto mb-2 flex h-10 w-10 items-center justify-center rounded-full bg-bg-elevated">
            <svg v-if="loading && effectiveSrc" class="h-5 w-5 text-text-muted animate-spin" fill="none" viewBox="0 0 24 24">
              <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
              <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>
            </svg>
            <svg v-else class="h-5 w-5 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
              <path stroke-linecap="round" stroke-linejoin="round" d="M15 10l4.553-4.553A1 1 0 0121 6.12V17.88a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
            </svg>
          </div>
          <span class="text-xs text-text-muted font-mono block truncate max-w-[200px]">{{ displayText }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, onUnmounted } from 'vue'
import { useThumbnailSrcset } from '@/composables/useThumbnailSrcset'

interface Props {
  src: string | null
  thumbnails?: Record<string, string>
  sizes?: string
  fallbackText?: string
  aspectRatio?: '16/9' | '4/3' | '1/1'
  retryIntervalMs?: number
  maxRetries?: number
}

const props = withDefaults(defineProps<Props>(), {
  aspectRatio: '16/9',
  sizes: '100vw',
  fallbackText: 'No preview',
  retryIntervalMs: 5000,
  maxRetries: 12,
})

const loading = ref(true)
const error = ref(false)
const retryCount = ref(0)
const retryKey = ref(0)
let retryTimer: ReturnType<typeof setInterval> | null = null

const thumbnailsRef = computed(() => props.thumbnails)
const { pngSrcset, jxlSrcset, avifSrcset, fallbackUrl } = useThumbnailSrcset(thumbnailsRef)

const singlePngSrc = computed(() => props.src)
const singleJxlSrc = computed(() => singlePngSrc.value?.replace(/\.png$/, '.jxl'))
const singleAvifSrc = computed(() => singlePngSrc.value?.replace(/\.png$/, '.avif'))

const effectiveSrc = computed(() => fallbackUrl.value || singlePngSrc.value)

const displayText = computed(() => {
  if (loading.value && effectiveSrc.value) return 'Generating preview...'
  return props.fallbackText
})

function handleLoad() {
  loading.value = false
  error.value = false
  stopRetry()
}

function handleError() {
  loading.value = false
  error.value = true
  startRetry()
}

function startRetry() {
  if (retryTimer) return
  retryTimer = setInterval(() => {
    retryCount.value++
    retryKey.value++
    loading.value = true
    error.value = false
    if (retryCount.value >= props.maxRetries) {
      stopRetry()
      error.value = true
      loading.value = false
    }
  }, props.retryIntervalMs)
}

function stopRetry() {
  if (retryTimer) {
    clearInterval(retryTimer)
    retryTimer = null
  }
}

watch(effectiveSrc, (newSrc, oldSrc) => {
  if (newSrc !== oldSrc) {
    loading.value = true
    error.value = false
    retryCount.value = 0
    retryKey.value = 0
    stopRetry()
  }
})

onUnmounted(() => {
  stopRetry()
})

const aspectRatioPadding = computed(() => {
  switch (props.aspectRatio) {
    case '4/3':
      return '75%'
    case '1/1':
      return '100%'
    default:
      return '56.25%'
  }
})
</script>
