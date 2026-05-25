<template>
  <div>
    <div class="flex flex-col sm:flex-row sm:items-center justify-between gap-4 mb-6">
      <div>
        <h1 class="text-2xl font-bold text-text-primary">Recordings</h1>
        <p class="text-sm text-text-secondary mt-1">Saved stream recordings</p>
      </div>
      <router-link
        to="/"
        class="inline-flex self-start items-center gap-1 rounded-lg bg-bg-elevated px-3 py-2 text-sm font-medium text-text-secondary border border-border-default hover:bg-bg-overlay hover:text-text-primary transition"
      >
        <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M15 19l-7-7 7-7" />
        </svg>
        Back
      </router-link>
    </div>

    <!-- Filters -->
    <div class="flex flex-col sm:flex-row gap-3 mb-6">
      <div class="flex items-center gap-2 rounded-lg border border-border-default bg-bg-surface/60 px-3 py-2 min-w-[180px]">
        <svg class="h-4 w-4 text-text-muted shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
        </svg>
        <select
          v-model="selectedStreamKey"
          class="bg-transparent text-sm text-text-primary outline-none w-full cursor-pointer"
        >
          <option value="">All streams</option>
          <option v-for="key in streamKeys" :key="key" :value="key">{{ key }}</option>
        </select>
      </div>

      <div class="flex items-center gap-2 rounded-lg border border-border-default bg-bg-surface/60 px-3 py-2 min-w-[180px]">
        <svg class="h-4 w-4 text-text-muted shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M8 7V3m8 4V3m-9 8h10M5 21h14a2 2 0 002-2V7a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" />
        </svg>
        <select
          v-model="selectedTimeRange"
          class="bg-transparent text-sm text-text-primary outline-none w-full cursor-pointer"
        >
          <option value="all">All time</option>
          <option value="today">Today</option>
          <option value="week">Last 7 days</option>
          <option value="month">Last 30 days</option>
        </select>
      </div>

      <div class="flex items-center gap-2 ml-auto">
        <!-- View toggle -->
        <div class="flex items-center rounded-lg border border-border-default bg-bg-surface/60 overflow-hidden">
          <button
            class="p-2 transition"
            :class="viewMode === 'grid' ? 'bg-accent-primary text-white' : 'text-text-muted hover:text-text-primary hover:bg-bg-elevated'"
            @click="viewMode = 'grid'"
            title="Grid view"
          >
            <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M4 6a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2H6a2 2 0 01-2-2V6zM14 6a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2h-2a2 2 0 01-2-2V6zM4 16a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2H6a2 2 0 01-2-2v-2zM14 16a2 2 0 012-2h2a2 2 0 012 2v2a2 2 0 01-2 2h-2a2 2 0 01-2-2v-2z" />
            </svg>
          </button>
          <button
            class="p-2 transition"
            :class="viewMode === 'list' ? 'bg-accent-primary text-white' : 'text-text-muted hover:text-text-primary hover:bg-bg-elevated'"
            @click="viewMode = 'list'"
            title="List view"
          >
            <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
              <path stroke-linecap="round" stroke-linejoin="round" d="M4 6h16M4 12h16M4 18h16" />
            </svg>
          </button>
        </div>

        <span class="text-xs text-text-muted">
          {{ filteredRecordings.length }} result{{ filteredRecordings.length === 1 ? '' : 's' }}
        </span>
      </div>
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
    <RecordingsList
      v-else-if="filteredRecordings.length"
      :recordings="filteredRecordings"
      :view="viewMode"
      @play="activeRecording = $event"
    />

    <!-- Empty -->
    <BaseEmptyState
      v-else
      title="No recordings"
      :description="data?.length ? 'No recordings match the selected filters.' : 'Recordings will appear here after streams are saved.'"
    />

    <!-- Player Modal -->
    <RecordingPlayer
      v-if="activeRecording"
      :recording="activeRecording"
      @close="activeRecording = null"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import RecordingsList from '@/components/RecordingsList.vue'
import RecordingPlayer from '@/components/RecordingPlayer.vue'
import BaseSkeleton from '@/components/ui/BaseSkeleton.vue'
import BaseErrorState from '@/components/ui/BaseErrorState.vue'
import BaseEmptyState from '@/components/ui/BaseEmptyState.vue'
import { listRecordings } from '@/api/streams'
import { usePolling } from '@/composables/usePolling'
import type { Recording } from '@/types'

const { data, error, loading, refetch } = usePolling(() => listRecordings(), {
  interval: 5000,
  immediate: true,
})

const activeRecording = ref<Recording | null>(null)
const selectedStreamKey = ref('')
const selectedTimeRange = ref('all')
const viewMode = ref<'grid' | 'list'>('grid')

const streamKeys = computed(() => {
  const keys = new Set<string>()
  data.value?.forEach((r) => keys.add(r.stream_key))
  return Array.from(keys).sort()
})

const filteredRecordings = computed(() => {
  let list = data.value ?? []

  if (selectedStreamKey.value) {
    list = list.filter((r) => r.stream_key === selectedStreamKey.value)
  }

  const now = new Date()
  if (selectedTimeRange.value === 'today') {
    const startOfDay = new Date(now.getFullYear(), now.getMonth(), now.getDate())
    list = list.filter((r) => new Date(r.created_at) >= startOfDay)
  } else if (selectedTimeRange.value === 'week') {
    const weekAgo = new Date(now.getTime() - 7 * 24 * 60 * 60 * 1000)
    list = list.filter((r) => new Date(r.created_at) >= weekAgo)
  } else if (selectedTimeRange.value === 'month') {
    const monthAgo = new Date(now.getTime() - 30 * 24 * 60 * 60 * 1000)
    list = list.filter((r) => new Date(r.created_at) >= monthAgo)
  }

  return list
})
</script>
