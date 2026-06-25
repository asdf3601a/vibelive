import { computed, type ComputedRef } from 'vue'

interface ThumbnailSrcsetResult {
  pngSrcset: ComputedRef<string>
  jxlSrcset: ComputedRef<string>
  avifSrcset: ComputedRef<string>
  fallbackUrl: ComputedRef<string>
}

export function useThumbnailSrcset(thumbnails: ComputedRef<Record<string, string> | undefined>): ThumbnailSrcsetResult {
  const widthEntries = computed(() => {
    const map = thumbnails.value
    if (!map) return []
    return Object.entries(map)
      .map(([w, url]) => ({ width: parseInt(w, 10), url }))
      .filter(e => !isNaN(e.width) && e.width > 0)
      .sort((a, b) => b.width - a.width)
  })

  const pngSrcset = computed(() =>
    widthEntries.value.map(e => `${e.url} ${e.width}w`).join(', ')
  )

  const jxlSrcset = computed(() =>
    widthEntries.value.map(e => `${e.url.replace(/\.png$/, '.jxl')} ${e.width}w`).join(', ')
  )

  const avifSrcset = computed(() =>
    widthEntries.value.map(e => `${e.url.replace(/\.png$/, '.avif')} ${e.width}w`).join(', ')
  )

  const fallbackUrl = computed(() => widthEntries.value[0]?.url ?? '')

  return { pngSrcset, jxlSrcset, avifSrcset, fallbackUrl }
}
