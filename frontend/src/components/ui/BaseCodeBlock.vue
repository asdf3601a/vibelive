<template>
  <div class="flex items-start gap-1.5 rounded-lg bg-bg-base px-3 py-2 font-mono text-xs text-text-secondary border border-border-default">
    <code :class="multiline ? 'whitespace-pre-wrap break-all' : 'truncate'">{{ text }}</code>
    <BaseButton
      v-if="copyable"
      variant="ghost"
      class="!px-1.5 !py-0.5 shrink-0"
      @click="copy(text)"
    >
      <svg class="h-3.5 w-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
        <path stroke-linecap="round" stroke-linejoin="round" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
      </svg>
      {{ copied ? 'Copied!' : 'Copy' }}
    </BaseButton>
  </div>
</template>

<script setup lang="ts">
import BaseButton from './BaseButton.vue'
import { useClipboard } from '@/composables/useClipboard'

interface Props {
  text: string
  copyable?: boolean
  multiline?: boolean
}
withDefaults(defineProps<Props>(), {
  copyable: true,
  multiline: false,
})

const { copied, copy } = useClipboard()
</script>
