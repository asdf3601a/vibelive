<template>
  <div>
    <router-link
      to="/"
      class="inline-flex items-center gap-1 text-sm text-text-secondary hover:text-text-primary transition mb-4"
    >
      <svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
        <path stroke-linecap="round" stroke-linejoin="round" d="M15 19l-7-7 7-7" />
      </svg>
      Back to streams
    </router-link>

    <!-- Error state -->
    <BaseErrorState
      v-if="error"
      title="Failed to load stream"
      description="Could not fetch stream details. The stream may have ended or the server may be unreachable."
      :on-retry="refetch"
    />

    <!-- Main layout -->
    <div v-else-if="stream" class="grid grid-cols-1 lg:grid-cols-3 gap-6">
      <!-- Video + details -->
      <div class="lg:col-span-2 space-y-4 min-w-0">
        <Player :src="hlsUrl" />

        <div class="rounded-xl border border-border-default bg-bg-surface/60 p-4">
          <div class="flex items-start justify-between gap-4">
            <div class="min-w-0">
              <h1 class="text-xl font-bold text-text-primary truncate">{{ stream.stream_key }}</h1>
              <div class="mt-1 flex flex-wrap items-center gap-3 text-sm text-text-secondary">
                <BaseBadge :status="stream.status === 'live' ? 'live' : 'ended'" />
                <span v-if="stream.started_at">Started {{ formatDateTime(stream.started_at) }}</span>
              </div>
            </div>
          </div>

          <div v-if="stream.metadata" class="mt-4 flex flex-wrap gap-2">
            <BaseTag>{{ stream.metadata.width }}×{{ stream.metadata.height }}</BaseTag>
            <BaseTag>{{ stream.metadata.video_codec }}</BaseTag>
            <BaseTag v-if="stream.metadata.audio_codec">{{ stream.metadata.audio_codec }}</BaseTag>
            <BaseTag v-if="stream.metadata.framerate">{{ stream.metadata.framerate }} fps</BaseTag>
          </div>

          <div class="mt-4 flex items-center gap-2 min-w-0">
            <span class="text-xs text-text-muted shrink-0">HLS:</span>
            <BaseCodeBlock :text="stream.hls_url ?? '—'" />
          </div>
        </div>
      </div>

      <!-- Sidebar -->
      <div class="space-y-4 min-w-0">
        <StreamInfo :stream="stream" />

        <div v-if="recordings.length" class="rounded-xl border border-border-default bg-bg-surface/60 p-4">
          <h3 class="text-sm font-semibold text-text-primary mb-3">Recordings</h3>
          <RecordingsList :recordings="recordings" view="list" @play="activeRecording = $event" />
        </div>
      </div>
    </div>

    <!-- Loading skeleton -->
    <div v-else-if="loading" class="grid grid-cols-1 lg:grid-cols-3 gap-6">
      <div class="lg:col-span-2 space-y-4 min-w-0">
        <BaseSkeleton variant="video" />
        <div class="rounded-xl border border-border-default bg-bg-surface/60 p-4 space-y-3">
          <BaseSkeleton variant="text" class="w-48" />
          <BaseSkeleton variant="text" class="w-32" />
          <div class="flex gap-2">
            <BaseSkeleton variant="text" class="w-20" />
            <BaseSkeleton variant="text" class="w-20" />
          </div>
        </div>
      </div>
      <div class="space-y-4 min-w-0">
        <div class="rounded-xl border border-border-default bg-bg-surface/60 p-4 space-y-3">
          <BaseSkeleton variant="text" class="w-24" />
          <div class="space-y-2">
            <BaseSkeleton variant="text" />
            <BaseSkeleton variant="text" />
            <BaseSkeleton variant="text" class="w-3/4" />
          </div>
        </div>
      </div>
    </div>

    <!-- Not found -->
    <BaseEmptyState v-else title="Stream not found" description="This stream is not currently active.">
      <template #action>
        <router-link
          to="/"
          class="inline-flex items-center gap-1 rounded-lg bg-accent-primary px-4 py-2 text-sm font-medium text-white hover:bg-accent-primary/90 transition"
        >
          Back to streams
        </router-link>
      </template>
    </BaseEmptyState>

    <!-- Player Modal for recordings -->
    <RecordingPlayer
      v-if="activeRecording"
      :recording="activeRecording"
      @close="activeRecording = null"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import Player from '@/components/Player.vue'
import StreamInfo from '@/components/StreamInfo.vue'
import RecordingsList from '@/components/RecordingsList.vue'
import RecordingPlayer from '@/components/RecordingPlayer.vue'
import BaseBadge from '@/components/ui/BaseBadge.vue'
import BaseTag from '@/components/ui/BaseTag.vue'
import BaseSkeleton from '@/components/ui/BaseSkeleton.vue'
import BaseEmptyState from '@/components/ui/BaseEmptyState.vue'
import BaseErrorState from '@/components/ui/BaseErrorState.vue'
import BaseCodeBlock from '@/components/ui/BaseCodeBlock.vue'
import { useStream } from '@/composables/useStream'
import { formatDateTime } from '@/utils/format'
import type { Recording } from '@/types'

const route = useRoute()
const key = computed(() => route.params.key as string)

const { data, error, loading, refetch } = useStream(key.value)
const stream = computed(() => data.value ?? null)
const hlsUrl = computed(() => stream.value?.hls_url ?? null)

const recordings = ref<Recording[]>([])
const activeRecording = ref<Recording | null>(null)

watch(
  () => stream.value?.stream_key,
  (name) => {
    document.title = name ? `Live Watch — ${name}` : 'Live Watch'
  },
  { immediate: true },
)
</script>
