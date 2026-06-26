import type { Ref } from 'vue'

const SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 4, 8, 16] as const

export type SeekDirection = 'forward' | 'backward'

interface KeyboardDeps {
  enableKeyboard: boolean
  state: Ref<string>
  volume: Ref<number>
  playbackRate: Ref<number>
  duration: Ref<number>
  currentTime: Ref<number>
  lastSeekTime: Ref<number>
  seekCount: Ref<number>
  seekDecayTimer: { value: ReturnType<typeof setTimeout> | null }
  togglePlay: () => void
  dynamicSeek: (dir: SeekDirection) => void
  seekRelative: (seconds: number) => void
  setVolume: (v: number) => void
  toggleMute: () => void
  toggleFullscreen: () => void
  showControls: () => void
  setPlaybackRate: (rate: number) => void
  seekTo: (time: number) => void
}

export function createKeydownHandler(deps: KeyboardDeps): (e: KeyboardEvent) => void {
  return function handleKeydown(e: KeyboardEvent) {
    if (!deps.enableKeyboard) return
    const tag = (e.target as HTMLElement)?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return

    switch (e.key) {
      case ' ':
      case 'k':
        e.preventDefault()
        deps.togglePlay()
        break
      case 'j':
        deps.dynamicSeek('backward')
        break
      case 'l':
        deps.dynamicSeek('forward')
        break
      case 'ArrowLeft':
        e.preventDefault()
        deps.seekRelative(e.shiftKey ? -10 : -5)
        break
      case 'ArrowRight':
        e.preventDefault()
        deps.seekRelative(e.shiftKey ? 10 : 5)
        break
      case 'ArrowUp':
        e.preventDefault()
        deps.setVolume(deps.volume.value + 0.1)
        break
      case 'ArrowDown':
        e.preventDefault()
        deps.setVolume(deps.volume.value - 0.1)
        break
      case 'm':
        deps.toggleMute()
        break
      case 'f':
        deps.toggleFullscreen()
        break
      case 'Escape':
        if (document.fullscreenElement) {
          document.exitFullscreen()
          deps.showControls()
        }
        break
      case ',':
        if (deps.state.value === 'paused') {
          deps.seekRelative(-1 / 24)
        }
        break
      case '.':
        if (deps.state.value === 'paused') {
          deps.seekRelative(1 / 24)
        }
        break
      case '>':
        if (e.shiftKey) {
          const idx = SPEEDS.indexOf(deps.playbackRate.value as any)
          if (idx < SPEEDS.length - 1) deps.setPlaybackRate(SPEEDS[idx + 1])
        }
        break
      case '<':
        if (e.shiftKey) {
          const idx = SPEEDS.indexOf(deps.playbackRate.value as any)
          if (idx > 0) deps.setPlaybackRate(SPEEDS[idx - 1])
        }
        break
      default:
        if (e.key >= '0' && e.key <= '9') {
          const pct = parseInt(e.key) / 10
          deps.seekTo(deps.duration.value * pct)
        }
        break
    }
  }
}
