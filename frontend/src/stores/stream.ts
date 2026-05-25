import { defineStore } from 'pinia'
import { ref } from 'vue'
import type { Stream } from '@/types'
import { listStreams } from '@/api/streams'

export const useStreamStore = defineStore('stream', () => {
  const streams = ref<Stream[]>([])
  const loading = ref(false)
  const error = ref<Error | null>(null)

  async function fetchStreams() {
    loading.value = true
    error.value = null
    try {
      streams.value = await listStreams()
    } catch (e) {
      error.value = e instanceof Error ? e : new Error(String(e))
    } finally {
      loading.value = false
    }
  }

  return { streams, loading, error, fetchStreams }
})
