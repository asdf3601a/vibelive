<template>
  <div
    ref="progressRef"
    :class="['relative w-full h-1.5 group/progress', isDragging ? 'cursor-grabbing' : 'cursor-grab']"
    @mousedown.prevent="startDrag"
    @touchstart.prevent="startDrag"
    @mousemove="onHover"
    @mouseleave="hoverTime = null"
  >
    <!-- Background track -->
    <div class="absolute inset-0 rounded-full bg-white/10">
      <!-- Buffered (single merged bar) -->
      <div
        v-if="bufferedEnd > 0"
        class="absolute inset-y-0 left-0 rounded-full bg-white/20"
        :style="{ width: pct(bufferedEnd / (duration || 1)) }"
      />
      <!-- Progress -->
      <div
        class="absolute inset-y-0 left-0 rounded-full bg-accent-primary transition-[width] duration-100"
        :style="{ width: pct(progress) }"
      >
        <div class="absolute right-0 top-1/2 -translate-y-1/2 w-3 h-3 rounded-full bg-accent-primary opacity-0 group-hover/progress:opacity-100 transition shadow-lg" />
      </div>
    </div>

    <!-- A-B loop markers -->
    <div
      v-if="loopA !== null"
      class="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 z-10"
      :style="{ left: pct(ratioAtTime(loopA)) }"
    >
      <div class="w-0 h-0 border-l-[6px] border-r-[6px] border-b-[8px] border-transparent border-b-accent-success" />
    </div>
    <div
      v-if="loopB !== null"
      class="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 z-10"
      :style="{ left: pct(ratioAtTime(loopB)) }"
    >
      <div class="w-0 h-0 border-l-[6px] border-r-[6px] border-t-[8px] border-transparent border-t-accent-live" />
    </div>
    <div
      v-if="loopActive && loopA !== null && loopB !== null"
      class="absolute inset-y-0 rounded-full bg-accent-success/20"
      :style="{ left: pct(ratioAtTime(loopA)), width: pct(ratioAtTime(loopB) - ratioAtTime(loopA)) }"
    />

    <!-- Hover / drag time tooltip -->
    <div
      v-if="hoverTime !== null && !isNaN(hoverTime)"
      class="absolute -top-9 -translate-x-1/2 z-20 pointer-events-none"
      :style="{ left: pct(hoverRatio) }"
    >
      <div class="bg-black/90 text-white text-xs font-mono px-2 py-1 rounded shadow-lg whitespace-nowrap">
        {{ formatTime(hoverTime) }}
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'

interface Props {
  progress: number
  duration: number
  currentTime: number
  buffered: { start: number; end: number }[]
  bufferedEnd: number
  loopA: number | null
  loopB: number | null
  loopActive: boolean
  liveStart?: number
}

const props = defineProps<Props>()
const emit = defineEmits<{
  seek: [time: number]
}>()

const progressRef = ref<HTMLElement | null>(null)
const hoverTime = ref<number | null>(null)
const hoverRatio = ref(0)
const isDragging = ref(false)

function pct(val: number): string {
  return `${Math.max(0, Math.min(100, val * 100))}%`
}

function ratioAtTime(t: number): number {
  if (props.duration <= 0) return 0
  return Math.max(0, Math.min(1, t / props.duration))
}

function formatTime(t: number): string {
  if (!isFinite(t) || t < 0) return '0:00'
  const hrs = Math.floor(t / 3600)
  const mins = Math.floor((t % 3600) / 60)
  const secs = Math.floor(t % 60)
  if (hrs > 0) return `${hrs}:${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`
  return `${mins}:${String(secs).padStart(2, '0')}`
}

function clientXToTime(clientX: number, rect: DOMRect): number {
  const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width))
  return ratio * props.duration + (props.liveStart ?? 0)
}

function onHover(e: MouseEvent) {
  const rect = progressRef.value?.getBoundingClientRect()
  if (!rect) return
  const ratio = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width))
  hoverRatio.value = ratio
  hoverTime.value = ratio * props.duration + (props.liveStart ?? 0)
}

function startDrag(e: MouseEvent | TouchEvent) {
  const rect = progressRef.value?.getBoundingClientRect()
  if (!rect) return
  isDragging.value = true

  const moveHandler = (ev: MouseEvent | TouchEvent) => {
    ev.preventDefault()
    const clientX = 'touches' in ev ? ev.touches[0].clientX : ev.clientX
    const time = clientXToTime(clientX, rect)
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width))
    hoverTime.value = time
    hoverRatio.value = ratio
    emit('seek', time)
  }

  const upHandler = () => {
    isDragging.value = false
    document.removeEventListener('mousemove', moveHandler)
    document.removeEventListener('mouseup', upHandler)
    document.removeEventListener('touchmove', moveHandler)
    document.removeEventListener('touchend', upHandler)
  }

  document.addEventListener('mousemove', moveHandler)
  document.addEventListener('mouseup', upHandler)
  document.addEventListener('touchmove', moveHandler, { passive: false })
  document.addEventListener('touchend', upHandler)

  const clientX = 'touches' in e ? e.touches[0].clientX : e.clientX
  const time = clientXToTime(clientX, rect)
  const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width))
  hoverTime.value = time
  hoverRatio.value = ratio
  emit('seek', time)
}
</script>