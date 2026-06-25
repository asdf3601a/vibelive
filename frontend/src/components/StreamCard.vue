<template>
  <BaseCard hoverable>
    <router-link :to="`/live/${stream.stream_key}`" class="block">
      <ThumbnailImg
        :src="stream.thumbnail_url"
        :thumbnails="stream.thumbnails"
        sizes="(max-width: 640px) 100vw, (max-width: 1024px) 50vw, 33vw"
        :fallback-text="stream.stream_key"
        aspect-ratio="16/9"
      />
    </router-link>

    <div class="p-4 min-w-0">
      <div class="flex items-center justify-between gap-3">
        <h3 class="font-semibold text-text-primary truncate">{{ stream.stream_key }}</h3>
        <BaseBadge :status="stream.status === 'live' ? 'live' : 'ended'" />
      </div>

      <p v-if="stream.started_at" class="text-xs text-text-muted mt-1">
        {{ formatDateTime(stream.started_at) }}
      </p>

      <div v-if="stream.metadata" class="mt-2 flex flex-wrap gap-2">
        <BaseTag>
          {{ stream.metadata.width }}×{{ stream.metadata.height }}
        </BaseTag>
        <BaseTag>
          {{ stream.metadata.video_codec }}
        </BaseTag>
        <BaseTag v-if="stream.tracks && stream.tracks.length > 1">
          {{ stream.tracks.length }} tracks
        </BaseTag>
      </div>

      <div class="mt-3 flex items-center justify-between min-w-0">
        <span v-if="stream.status === 'live'" class="inline-flex items-center gap-1.5 text-xs font-medium text-accent-live">
          <span class="h-1.5 w-1.5 rounded-full bg-accent-live animate-pulse"></span>
          LIVE
        </span>
        <span v-else class="text-xs text-text-muted truncate mr-2">
          {{ relativeTime }}
        </span>

        <BaseButton
          v-if="stream.hls_url"
          :to="`/live/${stream.stream_key}`"
          variant="primary"
          class="!text-xs"
        >
          <svg class="h-3.5 w-3.5" fill="currentColor" viewBox="0 0 24 24">
            <path d="M8 5v14l11-7z" />
          </svg>
          Watch
        </BaseButton>
      </div>
    </div>
  </BaseCard>
</template>

<script setup lang="ts">
import type { Stream } from '@/types'
import BaseCard from '@/components/ui/BaseCard.vue'
import BaseBadge from '@/components/ui/BaseBadge.vue'
import BaseTag from '@/components/ui/BaseTag.vue'
import BaseButton from '@/components/ui/BaseButton.vue'
import ThumbnailImg from '@/components/ThumbnailImg.vue'
import { useRelativeTime } from '@/composables/useRelativeTime'
import { formatDateTime } from '@/utils/format'

interface Props {
  stream: Stream
}

const props = defineProps<Props>()

const relativeTime = useRelativeTime(props.stream.started_at)
</script>
