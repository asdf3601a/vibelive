<template>
  <div class="group relative overflow-hidden rounded-xl bg-bg-base border border-border-default">
    <div class="w-full relative" :style="{ paddingBottom: aspectRatioPadding }">
      <img
        v-if="src && !error"
        :src="src"
        alt="Stream thumbnail"
        class="absolute inset-0 h-full w-full object-cover transition group-hover:scale-105"
        loading="lazy"
        @error="error = true"
      />
      <div v-else class="absolute inset-0 flex items-center justify-center bg-bg-base">
        <div class="text-center px-4">
          <div class="mx-auto mb-2 flex h-10 w-10 items-center justify-center rounded-full bg-bg-elevated">
            <svg class="h-5 w-5 text-text-muted" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
              <path stroke-linecap="round" stroke-linejoin="round" d="M15 10l4.553-4.553A1 1 0 0121 6.12V17.88a1 1 0 01-1.447.894L15 14M5 18h8a2 2 0 002-2V8a2 2 0 00-2-2H5a2 2 0 00-2 2v8a2 2 0 002 2z" />
            </svg>
          </div>
          <span class="text-xs text-text-muted font-mono block truncate max-w-[200px]">{{ fallbackText }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'

interface Props {
  src: string | null
  fallbackText?: string
  aspectRatio?: '16/9' | '4/3' | '1/1'
}

const props = withDefaults(defineProps<Props>(), {
  aspectRatio: '16/9',
  fallbackText: 'No preview',
})

const error = ref(false)

const aspectRatioPadding = computed(() => {
  switch (props.aspectRatio) {
    case '4/3':
      return '75%'
    case '1/1':
      return '100%'
    default:
      return '56.25%'
  }
})
</script>
