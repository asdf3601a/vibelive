<template>
  <div class="animate-pulse rounded-lg bg-bg-elevated" :class="classes">
    <slot />
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'

interface Props {
  variant?: 'text' | 'rect' | 'circle' | 'video'
  width?: string
  height?: string
}

const props = withDefaults(defineProps<Props>(), {
  variant: 'rect',
})

const classes = computed(() => {
  const base: string[] = []
  switch (props.variant) {
    case 'text':
      base.push('h-4 w-3/4')
      break
    case 'circle':
      base.push('rounded-full aspect-square w-10')
      break
    case 'video':
      base.push('aspect-video w-full')
      break
    default:
      base.push('h-20 w-full')
  }
  return base
})
</script>
