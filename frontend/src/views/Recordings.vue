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

      <!-- Date range -->
      <div class="flex items-center gap-2 rounded-lg border border-border-default bg-bg-surface/60 px-3 py-2">
        <svg class="h-4 w-4 text-text-muted shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
          <path stroke-linecap="round" stroke-linejoin="round" d="M8 7V3m8 4V3m-9 8h10M5 21h14a2 2 0 002-2V7a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" />
        </svg>
        <input
          v-model="startDate"
          type="date"
          class="bg-transparent text-sm text-text-primary outline-none cursor-pointer"
          placeholder="Start date"
        />
        <span class="text-xs text-text-muted">to</span>
        <input
          v-model="endDate"
          type="date"
          class="bg-transparent text-sm text-text-primary outline-none cursor-pointer"
          placeholder="End date"
        />
        <button
          v-if="startDate || endDate"
          class="text-xs text-text-muted hover:text-text-primary transition"
          @click="clearDateRange"
        >
          Clear
        </button>
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

    <!-- Active date range badge -->
    <div v-if="startDate || endDate" class="flex items-center gap-2 mb-4">
      <span class="inline-flex items-center gap-1 rounded-full bg-accent-primary/10 text-accent-primary px-2.5 py-1 text-xs font-medium">
        {{ dateRangeLabel }}
        <button class="hover:text-accent-primary/80" @click="clearDateRange">
          <svg class="h-3 w-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </span>
    </div>

    <!-- Loading -->
    <div v-if="loading && !displayedData.length" class="space-y-3">
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
      :description="displayedData.length ? 'No recordings match the selected filters.' : 'Recordings will appear here after streams are saved.'"
    />

    <!-- Player Modal -->
    <RecordingPlayer
      v-if="activeRecording"
      :recording="activeRecording"
      :initial-loop-a="activeRecordingLoopA"
      :initial-loop-b="activeRecordingLoopB"
      :initial-loop-enabled="activeRecordingLoopEnabled"
      @close="onPlayerClose"
    />

    <!-- Refresh toast -->
    <Transition name="toast">
      <div
        v-if="showRefreshToast"
        class="fixed bottom-6 left-1/2 -translate-x-1/2 z-50 flex items-center gap-3 rounded-full bg-bg-overlay border border-border-default px-4 py-2.5 shadow-lg"
      >
        <span class="text-sm text-text-primary">有新的錄影可查看</span>
        <button
          class="inline-flex items-center gap-1 rounded-full bg-accent-primary px-3 py-1 text-xs font-medium text-white hover:bg-accent-primary/90 transition"
          @click="applyUpdate"
        >
          <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
          </svg>
          重新整理
        </button>
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import RecordingsList from '@/components/RecordingsList.vue'
import RecordingPlayer from '@/components/RecordingPlayer.vue'
import BaseSkeleton from '@/components/ui/BaseSkeleton.vue'
import BaseErrorState from '@/components/ui/BaseErrorState.vue'
import BaseEmptyState from '@/components/ui/BaseEmptyState.vue'
import { listRecordings } from '@/api/streams'
import { usePolling } from '@/composables/usePolling'
import type { Recording } from '@/types'

const route = useRoute()
const router = useRouter()

const { data, error, loading, refetch } = usePolling(() => listRecordings(), {
  interval: 5000,
  immediate: true,
})

const displayedData = ref<Recording[]>([])
const showRefreshToast = ref(false)
const activeRecording = ref<Recording | null>(null)
const activeRecordingLoopA = ref<number | null>(null)
const activeRecordingLoopB = ref<number | null>(null)
const activeRecordingLoopEnabled = ref(false)
const selectedStreamKey = ref('')
const startDate = ref('')
const endDate = ref('')
const viewMode = ref<'grid' | 'list'>((localStorage.getItem('recordings_view_mode') as 'grid' | 'list') || 'grid')

watch(viewMode, (val) => {
  localStorage.setItem('recordings_view_mode', val)
})

// Only show refresh toast when recordings are actually added or removed.
// Field-level changes (timestamps, thumbnails) should not trigger the toast.
let isFirstLoad = true
watch(
  data,
  (newData) => {
    if (!newData) return
    if (isFirstLoad) {
      displayedData.value = newData
      isFirstLoad = false
    } else {
      const oldFilenames = new Set(displayedData.value.map(r => r.filename))
      const newFilenames = new Set(newData.map(r => r.filename))
      const changed = oldFilenames.size !== newFilenames.size ||
        [...oldFilenames].some(f => !newFilenames.has(f))
      if (changed) {
        showRefreshToast.value = true
      }
    }
  },
  { immediate: true },
)

function applyUpdate() {
  if (data.value) {
    displayedData.value = data.value
  }
  showRefreshToast.value = false
}

function parseLoopParams() {
  const rawA = route.query.loopA as string | undefined
  const rawB = route.query.loopB as string | undefined
  const rawLoop = route.query.loop as string | undefined
  if (rawA) activeRecordingLoopA.value = parseFloat(rawA)
  if (rawB) activeRecordingLoopB.value = parseFloat(rawB)
  activeRecordingLoopEnabled.value = rawLoop === 'true'
}

onMounted(() => {
  parseLoopParams()
  const playFilename = route.query.play as string | undefined
  if (playFilename && data.value) {
    const found = data.value.find(r => r.filename === playFilename)
    if (found) {
      activeRecording.value = found
    }
  }
})

watch(data, (newData) => {
  if (!newData) return
  const playFilename = route.query.play as string | undefined
  if (playFilename && !activeRecording.value) {
    const found = newData.find(r => r.filename === playFilename)
    if (found) {
      parseLoopParams()
      activeRecording.value = found
    }
  }
})

function onPlayerClose() {
  activeRecording.value = null
  activeRecordingLoopA.value = null
  activeRecordingLoopB.value = null
  activeRecordingLoopEnabled.value = false
  router.replace({ query: {} })
}

function clearDateRange() {
  startDate.value = ''
  endDate.value = ''
}

const streamKeys = computed(() => {
  const keys = new Set<string>()
  displayedData.value.forEach((r) => keys.add(r.stream_key))
  return Array.from(keys).sort()
})

const dateRangeLabel = computed(() => {
  if (startDate.value && endDate.value) {
    return `${startDate.value} to ${endDate.value}`
  }
  if (startDate.value) {
    return `From ${startDate.value}`
  }
  if (endDate.value) {
    return `Until ${endDate.value}`
  }
  return ''
})

const filteredRecordings = computed(() => {
  let list = displayedData.value

  if (selectedStreamKey.value) {
    list = list.filter((r) => r.stream_key === selectedStreamKey.value)
  }

  if (startDate.value) {
    const start = new Date(startDate.value)
    start.setHours(0, 0, 0, 0)
    list = list.filter((r) => {
      const created = new Date(r.created_at)
      return created >= start
    })
  }

  if (endDate.value) {
    const end = new Date(endDate.value)
    end.setHours(23, 59, 59, 999)
    list = list.filter((r) => {
      const created = new Date(r.created_at)
      return created <= end
    })
  }

  return list
})
</script>

<style>
.toast-enter-active,
.toast-leave-active {
  transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translate(-50%, 16px);
}
</style>
