/**
 * Composable for fetching and polling the stream list.
 */

import { listStreams } from '../api/streams'
import { usePolling } from './usePolling'

export function useStreamList() {
  return usePolling(() => listStreams(), {
    interval: 3000,
    immediate: true,
    pollWhenHidden: false,
  })
}
