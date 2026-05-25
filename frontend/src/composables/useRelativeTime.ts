/**
 * Reactive relative time string ("Just now", "2m ago", "1h ago").
 */

import { ref, onMounted, onUnmounted, type Ref } from 'vue'

export function useRelativeTime(dateStr: string | null): Ref<string> {
  const display = ref(formatRelative(dateStr))
  let timer: ReturnType<typeof setInterval> | null = null

  function update() {
    display.value = formatRelative(dateStr)
  }

  onMounted(() => {
    update()
    timer = setInterval(update, 60_000)
  })

  onUnmounted(() => {
    if (timer) clearInterval(timer)
  })

  return display
}

export function formatRelative(dateStr: string | null): string {
  if (!dateStr) return '—'
  const date = new Date(dateStr)
  const now = new Date()
  const diffSec = Math.floor((now.getTime() - date.getTime()) / 1000)
  if (diffSec < 0) return date.toLocaleDateString()
  if (diffSec < 60) return 'Just now'
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)}m ago`
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)}h ago`
  return date.toLocaleDateString()
}
