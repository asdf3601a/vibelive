<template>
  <div>
    <div class="flex items-center justify-between mb-6">
      <div>
        <h1 class="text-2xl font-bold text-text-primary">Live Streams</h1>
        <p class="text-sm text-text-secondary mt-1">
          {{ liveCount }} active {{ liveCount === 1 ? 'publisher' : 'publishers' }}
        </p>
      </div>
      <div class="flex items-center gap-2">
        <span class="relative flex h-3 w-3">
          <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-accent-success opacity-75"></span>
          <span class="relative inline-flex rounded-full h-3 w-3 bg-accent-success"></span>
        </span>
        <span class="text-xs font-medium text-accent-success">Polling</span>
      </div>
    </div>

    <!-- Loading skeletons -->
    <div v-if="loading && !displayedData?.length" class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
      <div v-for="i in 3" :key="i">
        <BaseCard hoverable>
          <BaseSkeleton variant="video" />
          <div class="p-4 space-y-3">
            <BaseSkeleton variant="text" class="w-32" />
            <div class="flex gap-2">
              <BaseSkeleton variant="text" class="w-20" />
              <BaseSkeleton variant="text" class="w-16" />
            </div>
          </div>
        </BaseCard>
      </div>
    </div>

    <!-- Error state -->
    <BaseErrorState
      v-else-if="error"
      title="Failed to load streams"
      description="Could not fetch the active stream list. The server may be unreachable."
      :on-retry="refetch"
    />

    <!-- Stream grid -->
    <TransitionGroup
      v-else-if="displayedData?.length"
      name="stream-list"
      tag="div"
      class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4"
    >
      <StreamCard v-for="stream in displayedData" :key="stream.stream_key" :stream="stream" />
    </TransitionGroup>

    <!-- Empty state -->
    <BaseEmptyState v-else title="No active streams" description="Start streaming to see it here.">
      <template #icon>
        <svg class="h-6 w-6 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
          <path stroke-linecap="round" stroke-linejoin="round" d="M15 10l4.553-4.553A1 1 0 0121 6.12V17.88a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
        </svg>
      </template>
      <template #action>
        <div class="flex items-center justify-center gap-2">
          <BaseCodeBlock text="rtmp://localhost:1935/live/" />
          <span class="font-mono text-xs text-text-muted">{any-key}</span>
        </div>
      </template>
    </BaseEmptyState>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import StreamCard from '@/components/StreamCard.vue'
import BaseCard from '@/components/ui/BaseCard.vue'
import BaseSkeleton from '@/components/ui/BaseSkeleton.vue'
import BaseEmptyState from '@/components/ui/BaseEmptyState.vue'
import BaseErrorState from '@/components/ui/BaseErrorState.vue'
import BaseCodeBlock from '@/components/ui/BaseCodeBlock.vue'
import { useStreamList } from '@/composables/useStreamList'
import type { Stream } from '@/types'

const { data, error, loading, refetch } = useStreamList()

// Two-stage data: only update displayed list when polled data actually changes
const displayedData = ref<Stream[]>([])

watch(
  data,
  (newData) => {
    if (newData) {
      displayedData.value = newData
    }
  },
  { immediate: true },
)

const liveCount = computed(() => displayedData.value?.length ?? 0)

watch(
  () => liveCount.value,
  () => {
    document.title = `LiveStream Platform — ${liveCount.value} live`
  },
  { immediate: true },
)
</script>

<style>
.stream-list-move,
.stream-list-enter-active,
.stream-list-leave-active {
  transition: all 0.3s ease;
}
.stream-list-enter-from,
.stream-list-leave-to {
  opacity: 0;
  transform: translateY(12px);
}
.stream-list-leave-active {
  position: absolute;
}
</style>
