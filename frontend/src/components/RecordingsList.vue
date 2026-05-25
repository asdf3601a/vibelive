<template>
  <div v-if="view === 'grid'" class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
    <RecordingGridCard
      v-for="recording in recordings"
      :key="recording.filename"
      :recording="recording"
      @play="$emit('play', $event)"
    />
  </div>
  <div v-else class="space-y-3">
    <RecordingCard
      v-for="recording in recordings"
      :key="recording.filename"
      :recording="recording"
      @play="$emit('play', $event)"
    />
  </div>
</template>

<script setup lang="ts">
import type { Recording } from '@/types'
import RecordingCard from './RecordingCard.vue'
import RecordingGridCard from './RecordingGridCard.vue'

interface Props {
  recordings: Recording[]
  view: 'grid' | 'list'
}
defineProps<Props>()

defineEmits<{
  play: [recording: Recording]
}>()
</script>
