<template>
  <BaseCard hoverable>
    <router-link :to="`/live/${stream.stream_key}`" class="block">
      <ThumbnailImg
        :src="thumbnailUrl"
        :fallback-text="stream.stream_key"
        aspect-ratio="16/9"
      />
    </router-link>

    <div class="p-4 min-w-0">
      <div class="flex items-center justify-between gap-3">
        <h3 class="font-semibold text-text-primary truncate">{{ stream.stream_key }}</h3>
        <BaseBadge :status="stream.status === 'live' ? 'live' : 'ended'" />
      </div>

      <div v-if="stream.metadata" class="mt-2 flex flex-wrap gap-2">
        <BaseTag>
          {{ stream.metadata.width }}×{{ stream.metadata.height }}
        </BaseTag>
        <BaseTag>
          {{ stream.metadata.video_codec }}
        </BaseTag>
      </div>

      <div class="mt-3 flex items-center justify-between min-w-0">
        <span class="text-xs text-text-muted truncate mr-2">
          {{ relativeTime }}
        </span>

        <router-link
          v-if="stream.hls_url"
          :to="`/live/${stream.stream_key}`"
          class="inline-flex shrink-0 items-center gap-1 rounded-lg bg-accent-primary px-3 py-1.5 text-xs font-medium text-white hover:bg-accent-primary/90 transition"
        >
          <svg class="h-3.5 w-3.5" fill="currentColor" viewBox="0 0 24 24">
            <path d="M8 5v14l11-7z" />
          </svg>
          Watch
        </router-link>
      </div>
    </div>
  </BaseCard>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { Stream } from '@/types'
import BaseCard from '@/components/ui/BaseCard.vue'
import BaseBadge from '@/components/ui/BaseBadge.vue'
import BaseTag from '@/components/ui/BaseTag.vue'
import ThumbnailImg from '@/components/ThumbnailImg.vue'
import { getThumbnailUrl } from '@/api/streams'
import { useRelativeTime } from '@/composables/useRelativeTime'

interface Props {
  stream: Stream
}

const props = defineProps<Props>()

const thumbnailUrl = computed(() =>
  getThumbnailUrl(props.stream.stream_key, 400),
)

const relativeTime = useRelativeTime(props.stream.started_at)
</script>
