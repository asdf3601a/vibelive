<template>
  <div class="space-y-3">
    <RecordingCard
      v-for="recording in recordings"
      :key="recording.filename"
      :recording="recording"
    />
  </div>
</template>

<script setup lang="ts">
import type { Recording } from '@/types'
import RecordingCard from './RecordingCard.vue'

interface Props {
  recordings: Recording[]
}
defineProps<Props>()
</script>
