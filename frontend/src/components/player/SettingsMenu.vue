<template>
  <div class="relative">
    <button
      class="p-1.5 rounded-lg text-white/80 hover:text-white hover:bg-white/10 transition cursor-pointer"
      title="Settings"
      @click.stop="showMenu = !showMenu"
    >
      <svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
        <path stroke-linecap="round" stroke-linejoin="round" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
        <path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
      </svg>
    </button>

    <Transition name="settings">
      <div
        v-if="showMenu"
        v-click-outside="() => showMenu = false"
        class="absolute bottom-full right-0 mb-2 w-56 bg-bg-overlay/95 border border-border-default rounded-xl shadow-2xl backdrop-blur overflow-hidden z-30"
      >
        <div class="py-1 max-h-80 overflow-y-auto overflow-x-hidden">
          <!-- Speed -->
          <div class="flex items-center justify-between px-3 py-2" @wheel.prevent="onSpeedWheel">
            <span class="text-xs text-text-secondary">Speed</span>
            <div class="flex items-center gap-1.5 min-w-0 w-[120px]">
              <input
                type="range"
                min="0"
                :max="speeds.length - 1"
                step="1"
                :value="speedIndex"
                class="flex-1 min-w-0 h-1 rounded-full accent-accent-primary cursor-pointer"
                @input="onSpeedSlider"
              />
              <span class="text-xs font-mono text-text-primary w-9 text-right shrink-0 tabular-nums">{{ playbackRate }}x</span>
            </div>
          </div>

          <div v-if="!isLive" class="border-t border-border-default" />

          <!-- A-B Loop (recordings only) -->
          <div v-if="!isLive" class="px-3 py-2">
            <div class="flex items-center justify-between mb-1.5">
              <span class="text-xs text-text-secondary">A-B Loop</span>
              <button
                v-if="loopA !== null || loopB !== null"
                class="text-[10px] text-text-muted hover:text-text-primary transition px-1.5 py-0.5 rounded hover:bg-bg-elevated"
                @click="$emit('clearLoop')"
              >Clear</button>
            </div>
            <div class="flex items-center gap-2 mb-2">
              <button
                class="flex-1 px-2 py-1 text-xs rounded-md font-medium transition"
                :class="loopA !== null
                  ? 'bg-accent-success/20 text-accent-success border border-accent-success/30'
                  : 'text-text-secondary/70 bg-bg-elevated border border-transparent hover:bg-bg-surface hover:text-text-primary'"
                @click="$emit('setLoopA')"
              >
                {{ loopA !== null ? `A ${formatTime(loopA)}` : 'Set A' }}
              </button>
              <button
                class="flex-1 px-2 py-1 text-xs rounded-md font-medium transition"
                :class="loopB !== null
                  ? 'bg-accent-live/20 text-accent-live border border-accent-live/30'
                  : 'text-text-secondary/70 bg-bg-elevated border border-transparent hover:bg-bg-surface hover:text-text-primary'"
                @click="$emit('setLoopB')"
              >
                {{ loopB !== null ? `B ${formatTime(loopB)}` : 'Set B' }}
              </button>
            </div>
          </div>

          <!-- Loop toggle (standalone row, only when A or B is set) -->
          <div
            v-if="!isLive && (loopA !== null || loopB !== null)"
            class="flex items-center justify-between px-3 py-2 cursor-pointer transition"
            :class="loopEnabled ? 'bg-accent-primary/5' : 'hover:bg-bg-elevated/50'"
            @click="$emit('setLoopEnabled', !loopEnabled)"
          >
            <span class="text-xs text-text-secondary">Loop</span>
            <span
              class="text-[10px] font-mono px-1.5 py-0.5 rounded transition"
              :class="loopEnabled
                ? 'bg-accent-primary/15 text-accent-primary'
                : 'bg-bg-elevated text-text-muted'"
            >{{ loopEnabled ? 'ON' : 'OFF' }}</span>
          </div>

          <div class="border-t border-border-default" />

          <!-- Live Lag -->
          <div v-if="isLive" class="flex items-center justify-between px-3 py-2">
            <span class="text-xs text-text-secondary">Live Lag</span>
            <div class="flex items-center gap-1.5 min-w-0 w-[120px]">
              <input
                type="range"
                min="1"
                max="60"
                step="1"
                :value="liveThreshold"
                class="flex-1 min-w-0 h-1 rounded-full accent-accent-primary cursor-pointer"
                @input="$emit('setLiveThreshold', parseInt(($event.target as HTMLInputElement).value))"
              />
              <span class="text-xs font-mono text-text-primary w-9 text-right shrink-0 tabular-nums">{{ liveThreshold }}s</span>
            </div>
          </div>

          <div v-if="isLive" class="border-t border-border-default" />

          <!-- Volume Boost -->
          <div class="flex items-center justify-between px-3 py-2 cursor-pointer" @click="$emit('toggleVolumeBoost')">
            <span class="text-xs" :class="volumeBoostEnabled ? 'text-accent-live' : 'text-text-secondary'">Volume Boost</span>
            <span
              class="text-[10px] font-mono px-1.5 py-0.5 rounded transition"
              :class="volumeBoostEnabled
                ? 'bg-accent-live/15 text-accent-live'
                : 'bg-bg-elevated text-text-muted'"
            >{{ volumeBoostEnabled ? 'ON' : 'OFF' }}</span>
          </div>

          <div class="border-t border-border-default" />

          <!-- Debug -->
          <div class="flex items-center justify-between px-3 py-2 cursor-pointer" @click="$emit('toggleDebug')">
            <span class="text-xs" :class="showDebug ? 'text-accent-primary' : 'text-text-secondary'">Debug Overlay</span>
            <span
              class="text-[10px] font-mono px-1.5 py-0.5 rounded transition"
              :class="showDebug
                ? 'bg-accent-primary/15 text-accent-primary'
                : 'bg-bg-elevated text-text-muted'"
            >{{ showDebug ? 'ON' : 'OFF' }}</span>
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'

interface Props {
  playbackRate: number
  loopA: number | null
  loopB: number | null
  loopEnabled: boolean
  showDebug: boolean
  isLive: boolean
  liveThreshold: number
  volumeBoostEnabled: boolean
}

const props = defineProps<Props>()

const emit = defineEmits<{
  setPlaybackRate: [rate: number]
  setLoopA: []
  setLoopB: []
  setLoopEnabled: [val: boolean]
  clearLoop: []
  toggleDebug: []
  setLiveThreshold: [seconds: number]
  toggleVolumeBoost: []
}>()

const showMenu = ref(false)

const speeds = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 4, 8, 16]

const speedIndex = computed(() => {
  const idx = speeds.indexOf(props.playbackRate)
  return idx >= 0 ? idx : 3
})

function onSpeedSlider(e: Event) {
  const idx = parseInt((e.target as HTMLInputElement).value)
  emit('setPlaybackRate', speeds[idx])
}

function onSpeedWheel(e: WheelEvent) {
  const dir = e.deltaY > 0 ? -1 : 1
  const newIdx = Math.max(0, Math.min(speeds.length - 1, speedIndex.value + dir))
  emit('setPlaybackRate', speeds[newIdx])
}

function formatTime(t: number): string {
  if (!isFinite(t) || t < 0) return '0:00'
  const hrs = Math.floor(t / 3600)
  const mins = Math.floor((t % 3600) / 60)
  const secs = Math.floor(t % 60)
  if (hrs > 0) return `${hrs}:${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`
  return `${mins}:${String(secs).padStart(2, '0')}`
}

const clickOutsideHandlers = new Map<HTMLElement, (e: MouseEvent) => void>()

const vClickOutside = {
  mounted(el: HTMLElement, binding: { value: () => void }) {
    const handler = (e: MouseEvent) => {
      if (!el.contains(e.target as Node)) {
        binding.value()
      }
    }
    clickOutsideHandlers.set(el, handler)
    document.addEventListener('click', handler)
  },
  unmounted(el: HTMLElement) {
    const handler = clickOutsideHandlers.get(el)
    if (handler) {
      document.removeEventListener('click', handler)
      clickOutsideHandlers.delete(el)
    }
  },
}
</script>

<style scoped>
.settings-enter-active,
.settings-leave-active {
  transition: all 0.15s ease;
}
.settings-enter-from,
.settings-leave-to {
  opacity: 0;
  transform: translateY(4px);
}
</style>