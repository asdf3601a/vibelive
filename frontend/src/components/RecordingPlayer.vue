<template>
  <Teleport to="body">
    <div
      class="fixed inset-0 z-[100] flex items-center justify-center bg-black/80 p-4"
      @click.self="$emit('close')"
    >
      <div class="w-full max-w-5xl bg-bg-surface shadow-2xl" :class="isFullscreen ? '' : 'rounded-xl border border-border-default'">
        <!-- Header -->
        <div class="flex items-center justify-between px-4 py-3">
          <div class="flex items-center gap-2 min-w-0">
            <router-link
              to="/"
              class="flex items-center gap-1 text-xs text-text-muted hover:text-text-primary transition shrink-0"
            >
              <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
                <path stroke-linecap="round" stroke-linejoin="round" d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6" />
              </svg>
              Home
            </router-link>
            <span class="text-text-muted/30">/</span>
            <h3 class="text-sm font-semibold text-text-primary truncate">{{ recording.filename }}</h3>
          </div>
          <button
            class="rounded-lg p-1.5 text-text-muted hover:text-text-primary hover:bg-bg-elevated transition"
            @click="$emit('close')"
          >
            <svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <!-- Player -->
        <Player
          :src="recording.url"
          :muted="false"
          :autoplay="true"
          :is-live="false"
          :initial-loop-a="initialLoopA"
          :initial-loop-b="initialLoopB"
          :initial-loop-enabled="initialLoopEnabled ?? false"
          @loop-update="onLoopUpdate"
        />

        <!-- Footer -->
        <div class="px-4 py-3 flex items-center gap-3 text-xs text-text-secondary flex-wrap">
          <button
            class="inline-flex items-center gap-1 rounded-lg px-3 py-1.5 font-medium border shrink-0 transition"
            :class="copiedStatus
              ? 'text-accent-success border-accent-success/30 bg-accent-success/10'
              : 'text-text-secondary border-border-default bg-bg-elevated hover:bg-bg-overlay hover:text-text-primary'"
            @click="copyShareLink()"
          >
            <svg v-if="copiedStatus" class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
            </svg>
            <svg v-else class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M13.828 10.172a4 4 0 00-5.656 0l-4 4a4 4 0 105.656 5.656l1.102-1.101m-.758-4.899a4 4 0 005.656 0l4-4a4 4 0 00-5.656-5.656l-1.1 1.1" />
            </svg>
            {{ copiedStatus ? 'Copied!' : 'Share' }}
          </button>
          <a
            :href="recording.url"
            download
            class="inline-flex items-center gap-1 rounded-lg bg-bg-elevated px-3 py-1.5 font-medium text-text-secondary border border-border-default hover:bg-bg-overlay hover:text-text-primary transition shrink-0"
          >
            <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
            </svg>
            Download
          </a>
          <span class="font-mono shrink-0">Stream: {{ recording.stream_key }}</span>
          <span>Recorded: {{ formatDateTime(recording.created_at) }}</span>
          <span v-if="recording.duration_seconds" class="ml-auto">Duration: {{ formatDuration(recording.duration_seconds) }}</span>
        </div>

        <!-- Share options -->
        <div
          v-if="showShareOptions"
          class="px-4 py-2 bg-bg-elevated/50 flex items-center gap-3 text-xs text-text-secondary"
        >
          <label class="flex items-center gap-2 cursor-pointer select-none">
            <input
              type="checkbox"
              v-model="includeLoopInLink"
              class="rounded border-border-default bg-bg-surface accent-accent-primary"
            />
            Include A-B loop in link
          </label>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted } from 'vue'
import type { Recording } from '@/types'
import { formatDateTime, formatDuration } from '@/utils/format'
import { copyToClipboard } from '@/utils/clipboard'
import Player from '@/components/Player.vue'

interface Props {
  recording: Recording
  initialLoopA?: number | null
  initialLoopB?: number | null
  initialLoopEnabled?: boolean
}

const props = withDefaults(defineProps<Props>(), {
  initialLoopA: null,
  initialLoopB: null,
  initialLoopEnabled: false,
})

defineEmits<{
  close: []
}>()

const copiedStatus = ref(false)
const includeLoopInLink = ref(false)

const isFullscreen = ref(false)

function onFullscreenChange() {
  isFullscreen.value = !!document.fullscreenElement
}

onMounted(() => {
  document.addEventListener('fullscreenchange', onFullscreenChange)
})

onUnmounted(() => {
  document.removeEventListener('fullscreenchange', onFullscreenChange)
})

// Current loop state from Player (live, not just initial)
const currentLoopA = ref<number | null>(props.initialLoopA)
const currentLoopB = ref<number | null>(props.initialLoopB)
const currentLoopEnabled = ref(props.initialLoopEnabled)

function onLoopUpdate(data: { loopA: number | null; loopB: number | null; loopEnabled: boolean }) {
  currentLoopA.value = data.loopA
  currentLoopB.value = data.loopB
  currentLoopEnabled.value = data.loopEnabled
}

const showShareOptions = computed(() =>
  currentLoopA.value !== null || currentLoopB.value !== null
)

async function copyShareLink() {
  let shareUrl = `${window.location.origin}/recordings?play=${encodeURIComponent(props.recording.filename)}`
  if (includeLoopInLink.value && (currentLoopA.value !== null || currentLoopB.value !== null)) {
    if (currentLoopA.value !== null) shareUrl += `&loopA=${currentLoopA.value}`
    if (currentLoopB.value !== null) shareUrl += `&loopB=${currentLoopB.value}`
    if (currentLoopEnabled.value) shareUrl += `&loop=true`
  }

  const ok = await copyToClipboard(shareUrl)
  if (ok) {
    copiedStatus.value = true
    setTimeout(() => { copiedStatus.value = false }, 2000)
  }
}
</script>