/**
 * Generic polling composable with visibility-aware pause/resume.
 * Only updates reactive state when fetched data actually changes (deep equality).
 */

import { ref, onMounted, onUnmounted, type Ref } from 'vue'
import { deepEqual } from '@/utils/deepEqual'

export interface UsePollingOptions {
  /** Interval in milliseconds (default: 3000) */
  interval?: number
  /** Whether to start immediately (default: true) */
  immediate?: boolean
  /** Continue polling when the page/tab is hidden (default: false) */
  pollWhenHidden?: boolean
}

export interface UsePollingResult<T> {
  data: Ref<T | null>
  error: Ref<Error | null>
  loading: Ref<boolean>
  refetch: () => Promise<void>
  pause: () => void
  resume: () => void
}

export function usePolling<T>(
  fetchFn: () => Promise<T>,
  opts: UsePollingOptions = {},
): UsePollingResult<T> {
  const { interval = 3000, immediate = true, pollWhenHidden = false } = opts

  const data = ref<T | null>(null) as Ref<T | null>
  const error = ref<Error | null>(null)
  const loading = ref(false)
  let timer: ReturnType<typeof setInterval> | null = null
  let aborted = false

  async function tick() {
    if (!pollWhenHidden && document.hidden) return
    loading.value = true
    try {
      const result = await fetchFn()
      if (!aborted) {
        // Only update if data actually changed to avoid unnecessary re-renders
        if (!deepEqual(data.value, result)) {
          data.value = result
        }
        error.value = null
      }
    } catch (e) {
      if (!aborted) {
        error.value = e instanceof Error ? e : new Error(String(e))
      }
    } finally {
      if (!aborted) loading.value = false
    }
  }

  async function refetch() {
    await tick()
  }

  function start() {
    if (timer) return
    timer = setInterval(tick, interval)
  }

  function stop() {
    if (timer) {
      clearInterval(timer)
      timer = null
    }
  }

  function pause() {
    stop()
  }

  function resume() {
    start()
  }

  onMounted(() => {
    aborted = false
    if (immediate) {
      tick().catch(() => {
        // swallow initial error to avoid unhandled rejection
      })
    }
    start()
  })

  onUnmounted(() => {
    aborted = true
    stop()
  })

  return { data, error, loading, refetch, pause, resume }
}
