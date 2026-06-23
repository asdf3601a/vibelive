<template>
  <div
    class="rounded-xl border border-border-default bg-bg-surface/60 overflow-hidden transition"
    :class="[
      hoverable ? 'hover:border-border-hover hover:bg-bg-surface' : '',
      padding ? 'p-4' : '',
    ]"
  >
    <slot />
  </div>
</template>

<script setup lang="ts">
interface Props {
  hoverable?: boolean
  padding?: boolean
}
withDefaults(defineProps<Props>(), {
  hoverable: true,
  padding: false,
})
</script>
