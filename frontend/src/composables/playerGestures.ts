import type { Ref } from 'vue'
import type { SeekDirection } from './playerKeyboard'

interface GestureDeps {
  containerRef: Ref<HTMLElement | null>
  dynamicSeek: (dir: SeekDirection) => void
  togglePlay: () => void
  showControls: () => void
}

export function createTouchHandlers(deps: GestureDeps) {
  let touchStartX = 0
  let touchStartY = 0
  let touchCount = 0
  let lastTapTime = 0
  let touchHandled = false
  let tapRegion: 'left' | 'center' | 'right' | null = null

  function handleTouchStart(e: TouchEvent) {
    touchHandled = false
    if ((e.target as HTMLElement).closest('button, a, input, [role="button"], [data-no-gesture]')) return
    if (e.touches.length !== 1) return
    touchStartX = e.touches[0].clientX
    touchStartY = e.touches[0].clientY

    deps.showControls()

    const now = Date.now()
    if (now - lastTapTime < 300) {
      touchCount++
    } else {
      touchCount = 1
    }
    lastTapTime = now

    const rect = deps.containerRef.value?.getBoundingClientRect()
    if (rect) {
      const x = touchStartX - rect.left
      const third = rect.width / 3
      if (x < third) tapRegion = 'left'
      else if (x > rect.width - third) tapRegion = 'right'
      else tapRegion = 'center'
    }

    if (touchCount === 2) {
      touchHandled = true
      if (!rect) return
      if (tapRegion === 'left') {
        deps.dynamicSeek('backward')
      } else if (tapRegion === 'right') {
        deps.dynamicSeek('forward')
      }
      touchCount = 0
    }
  }

  function handleTouchEnd(_e: TouchEvent) {
    // Single tap on the video area only shows controls.
    // Play/pause is handled by handleClick() based on tap region.
  }

  function handleClick() {
    if (touchHandled) {
      touchHandled = false
      tapRegion = null
      return
    }
    if (tapRegion === 'center' || tapRegion === null) {
      deps.togglePlay()
    }
    tapRegion = null
  }

  return { handleTouchStart, handleTouchEnd, handleClick }
}
