<template>
  <div>
    <div class="flex items-center justify-between mb-6">
      <div>
        <h1 class="text-2xl font-bold text-text-primary">Recordings</h1>
        <p class="text-sm text-text-secondary mt-1">Saved stream recordings</p>
      </div>
      <router-link
        to="/"
        class="inline-flex items-center gap-1 rounded-lg bg-bg-elevated px-3 py-2 text-sm font-medium text-text-secondary border border-border-default hover:bg-bg-overlay hover:text-text-primary transition"
      >
        <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M15 19l-7-7 7-7" />
        </svg>
        Back
      </router-link>
    </div>

    <!-- Loading -->
    <div v-if="loading" class="space-y-3">
      <div v-for="i in 4" :key="i" class="rounded-lg border border-border-default bg-bg-surface/60 px-4 py-3 space-y-2">
        <BaseSkeleton variant="text" class="w-48" />
        <BaseSkeleton variant="text" class="w-32" />
      </div>
    </div>

    <!-- Error -->
    <BaseErrorState
      v-else-if="error"
      title="Failed to load recordings"
      description="Could not fetch the recordings list."
      :on-retry="refetch"
    />

    <!-- List -->
    <RecordingsList v-else-if="data?.length" :recordings="data" />

    <!-- Empty -->
    <BaseEmptyState v-else title="No recordings" description="Recordings will appear here after streams are saved." />
  </div>
</template>

<script setup lang="ts">
import RecordingsList from '@/components/RecordingsList.vue'
import BaseSkeleton from '@/components/ui/BaseSkeleton.vue'
import BaseErrorState from '@/components/ui/BaseErrorState.vue'
import BaseEmptyState from '@/components/ui/BaseEmptyState.vue'
import { listRecordings } from '@/api/streams'
import { usePolling } from '@/composables/usePolling'

const { data, error, loading, refetch } = usePolling(() => listRecordings(), {
  interval: 5000,
  immediate: true,
})
</script>
