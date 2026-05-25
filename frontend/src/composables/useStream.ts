/**
 * Composable for fetching and polling a single stream.
 */

import { getStream } from '../api/streams'
import { usePolling } from './usePolling'

export function useStream(key: string) {
  return usePolling(() => getStream(key), {
    interval: 3000,
    immediate: true,
    pollWhenHidden: false,
  })
}
