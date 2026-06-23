<template>
  <component
    :is="to ? 'router-link' : 'button'"
    :to="to"
    class="inline-flex items-center justify-center gap-1.5 rounded-lg px-3 py-1.5 text-sm font-medium transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-primary focus-visible:ring-offset-2 focus-visible:ring-offset-bg-base disabled:opacity-50 disabled:cursor-not-allowed"
    :class="variantClasses[variant]"
    :disabled="to ? undefined : disabled"
    @click="$emit('click', $event)"
  >
    <slot />
  </component>
</template>

<script setup lang="ts">
type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger'

interface Props {
  variant?: ButtonVariant
  disabled?: boolean
  to?: string
}

withDefaults(defineProps<Props>(), {
  variant: 'primary',
  disabled: false,
})

defineEmits<{
  click: [event: MouseEvent]
}>()

const variantClasses: Record<ButtonVariant, string> = {
  primary:
    'bg-accent-primary text-white hover:bg-accent-primary/90 active:bg-accent-primary/80',
  secondary:
    'bg-bg-elevated text-text-primary border border-border-default hover:bg-bg-overlay hover:border-border-hover active:bg-bg-surface',
  ghost:
    'bg-transparent text-text-secondary hover:text-text-primary hover:bg-bg-elevated active:bg-bg-surface',
  danger:
    'bg-accent-live/10 text-accent-live border border-accent-live/20 hover:bg-accent-live/20 active:bg-accent-live/30',
}
</script>
