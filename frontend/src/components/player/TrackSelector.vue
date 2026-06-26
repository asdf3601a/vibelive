<template>
  <div v-if="tracks.length > 1" class="relative">
    <button
      class="flex items-center gap-1 px-2 py-1.5 rounded-lg text-white/80 hover:text-white hover:bg-white/10 transition text-xs font-medium cursor-pointer"
      title="Quality"
      @click.stop="showMenu = !showMenu"
    >
      <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
        <path stroke-linecap="round" stroke-linejoin="round" d="M9.813 15.904L9 18.75l-.813-2.846a4.5 4.5 0 00-3.09-3.09L2.25 12l2.846-.813a4.5 4.5 0 003.09-3.09L9 5.25l.813 2.846a4.5 4.5 0 003.09 3.09L15.75 12l-2.846.813a4.5 4.5 0 00-3.09 3.09z" />
        <path stroke-linecap="round" stroke-linejoin="round" d="M18.259 8.715L18 9.75l-.259-1.035a3.375 3.375 0 00-2.455-2.456L14.25 6l1.036-.259a3.375 3.375 0 002.455-2.456L18 2.25l.259 1.035a3.375 3.375 0 002.455 2.456L21.75 6l-1.036.259a3.375 3.375 0 00-2.455 2.456z" />
        <path stroke-linecap="round" stroke-linejoin="round" d="M16.894 20.567L16.5 21.75l-.394-1.183a2.25 2.25 0 00-1.423-1.423L13.5 18.75l1.183-.394a2.25 2.25 0 001.423-1.423l.394-1.183.394 1.183a2.25 2.25 0 001.423 1.423l1.183.394-1.183.394a2.25 2.25 0 00-1.423 1.423z" />
      </svg>
      <span class="hidden sm:inline">{{ activeTrackLabel }}</span>
    </button>

    <Transition name="quality">
      <div
        v-if="showMenu"
        v-click-outside="() => showMenu = false"
        class="absolute bottom-full right-0 mb-2 w-44 bg-bg-overlay/95 border border-border-default rounded-xl shadow-2xl overflow-hidden z-30"
      >
        <div class="py-1">
          <div class="px-3 py-1.5 text-[10px] text-text-muted uppercase tracking-wider font-medium">Video Track</div>
          <button
            v-for="track in tracks"
            :key="track.track_id"
            class="w-full flex items-center justify-between px-3 py-1.5 text-xs transition"
            :class="activeTrackId === track.track_id
              ? 'text-accent-primary bg-accent-primary/10'
              : 'text-text-secondary hover:bg-bg-surface hover:text-text-primary'"
            @click="select(track.track_id)"
          >
            <span>{{ track.track_id === 0 ? 'Default' : `Track ${track.track_id}` }}</span>
            <span class="font-mono text-[10px] opacity-70">{{ track.video_codec || '—' }}</span>
          </button>
        </div>
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import type { TrackInfo } from '@/types'

interface Props {
  tracks: TrackInfo[]
  activeTrackId: number
}

const props = defineProps<Props>()
const emit = defineEmits<{
  selectTrack: [trackId: number]
}>()

const showMenu = ref(false)

const activeTrackLabel = computed(() => {
  const track = props.tracks.find(t => t.track_id === props.activeTrackId)
  if (!track) return 'Auto'
  const name = track.track_id === 0 ? 'Default' : `Track ${track.track_id}`
  return `${name} (${track.video_codec || '—'})`
})

function select(trackId: number) {
  emit('selectTrack', trackId)
  showMenu.value = false
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
.quality-enter-active,
.quality-leave-active {
  transition: all 0.15s ease;
}
.quality-enter-from,
.quality-leave-to {
  opacity: 0;
  transform: translateY(4px);
}
</style>