<template>
  <component
    :is="to ? 'router-link' : 'span'"
    :to="to"
    class="inline-flex items-center gap-1.5 rounded-full bg-bg-elevated px-2.5 py-1 font-medium whitespace-nowrap text-text-secondary border border-border-default"
    :class="interactive ? 'hover:bg-bg-overlay hover:text-text-primary transition' : ''"
  >
    <slot />
  </component>
</template>

<script setup lang="ts">
interface Props {
  to?: string
  interactive?: boolean
}
withDefaults(defineProps<Props>(), {
  interactive: false,
})
</script>
