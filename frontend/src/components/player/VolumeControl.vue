<template>
  <div
    class="relative flex items-center gap-0 group/vol rounded-full bg-white/10 hover:bg-white/[0.14] transition-colors px-1.5"
    @wheel.prevent="onWheel"
  >
    <button
      class="p-1 rounded-lg transition shrink-0 cursor-pointer"
      :class="isVolumeBoosted ? 'text-accent-live hover:text-accent-live/80' : 'text-white/80 hover:text-white'"
      :title="isMuted ? 'Unmute (m)' : 'Mute (m)'"
      @click="$emit('toggleMute')"
    >
      <svg class="h-5 w-[38px]" fill="none" viewBox="0 0 38 24" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
        <!-- Speaker base -->
        <path d="M5.586 15H4a1 1 0 01-1-1v-4a1 1 0 011-1h1.586l4.707-4.707C10.923 3.663 12 4.109 12 5v14c0 .891-1.077 1.337-1.707.707L5.586 15z" />
        <!-- Mute X -->
        <path v-if="volumeStage === 0" d="M32 5l-18 18" />
        <!-- Wave 1 (25-100%) -->
        <path v-if="volumeStage >= 2" d="M17.5 16A5.5 5.5 0 0017.5 8" />
        <!-- Wave 2 (50-100%) -->
        <path v-if="volumeStage >= 3" d="M22.5 17A7.5 7.5 0 0022.5 7" />
        <!-- Wave 3 (75-150%) -->
        <path v-if="volumeStage >= 4" d="M28 18A9.5 9.5 0 0028 6" />
      </svg>
    </button>

    <div
      class="flex items-center w-0 group-hover/vol:w-28 group-focus-within/vol:w-28 overflow-hidden transition-all duration-150 origin-left"
      @wheel.prevent="onWheel"
    >
      <div class="relative w-[88px] h-5 flex items-center ml-1">
        <div class="relative w-full h-1.5 rounded-full cursor-pointer" :class="isVolumeBoosted ? 'bg-accent-live/25' : 'bg-white/15'" @click="onSliderClick">
          <div
            class="absolute left-0 top-0 h-full rounded-full pointer-events-none transition-colors"
            :class="isVolumeBoosted ? 'bg-accent-live' : 'bg-accent-primary'"
            :style="{ width: `${(volume / maxRange) * 100}%` }"
          />
          <input
            type="range"
            min="0"
            :max="maxRange"
            step="0.01"
            :value="volume"
            class="absolute inset-0 w-full h-full opacity-0 cursor-pointer z-10"
            @input="onInput"
          />
        </div>
        <span
          class="text-[10px] font-mono w-7 text-right ml-0.5 shrink-0 tabular-nums"
          :class="isVolumeBoosted ? 'text-accent-live font-bold' : 'text-white/70'"
        >{{ displayPct }}</span>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'

interface Props {
  volume: number
  volumeStage: number
  isVolumeBoosted: boolean
  isMuted: boolean
  volumeBoostEnabled: boolean
}

const props = defineProps<Props>()
const emit = defineEmits<{
  toggleMute: []
  setVolume: [v: number]
}>()

const maxRange = computed(() => props.volumeBoostEnabled ? 1.5 : 1)

const displayPct = computed(() => {
  if (props.isMuted || props.volume === 0) return '0'
  const pct = Math.round(props.volume * 100)
  return `${pct}`
})

function onInput(e: Event) {
  const val = parseFloat((e.target as HTMLInputElement).value)
  emit('setVolume', val)
}

function onWheel(e: WheelEvent) {
  const step = e.deltaY > 0 ? -0.05 : 0.05
  const newVal = Math.max(0, Math.min(maxRange.value, props.volume + step))
  emit('setVolume', Math.round(newVal * 100) / 100)
}

function onSliderClick(e: MouseEvent) {
  const el = e.currentTarget as HTMLElement
  const rect = el.getBoundingClientRect()
  const ratio = (e.clientX - rect.left) / rect.width
  emit('setVolume', Math.round(ratio * maxRange.value * 100) / 100)
}
</script>

<style scoped>
input[type="range"]::-webkit-slider-thumb {
  appearance: none;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: white;
  cursor: grab;
  border: none;
  box-shadow: 0 1px 4px rgba(0,0,0,0.4);
}
input[type="range"]::-webkit-slider-thumb:active {
  cursor: grabbing;
}
input[type="range"]::-moz-range-thumb {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: white;
  cursor: grab;
  border: none;
  box-shadow: 0 1px 4px rgba(0,0,0,0.4);
}
</style>