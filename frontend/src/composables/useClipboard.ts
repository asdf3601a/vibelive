/**
 * Composable for copying text to clipboard with temporary feedback.
 */

import { ref, type Ref } from 'vue'

export interface UseClipboardResult {
  copied: Ref<boolean>
  copy: (text: string) => Promise<void>
}

export function useClipboard(timeout = 2000): UseClipboardResult {
  const copied = ref(false)
  let timer: ReturnType<typeof setTimeout> | null = null

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text)
      copied.value = true
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => {
        copied.value = false
      }, timeout)
    } catch {
      // Silently ignore copy failures
    }
  }

  return { copied, copy }
}
