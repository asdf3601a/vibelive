import { ref, computed, watch, onUnmounted } from 'vue'
import Hls from 'hls.js'
import type { TrackInfo } from '@/types'
import { usePlayerVolume } from './playerVolume'
import { createKeydownHandler, type SeekDirection } from './playerKeyboard'
import { createTouchHandlers } from './playerGestures'

export type PlayerState = 'idle' | 'loading' | 'playing' | 'paused' | 'buffering' | 'ended' | 'error'
export type { SeekDirection }

interface UsePlayerOptions {
  enableKeyboard?: boolean
  enableAutoHide?: boolean
}

function safeGetItem(storage: Storage, key: string, fallback: string = ''): string {
  try { return storage.getItem(key) ?? fallback } catch { return fallback }
}
function safeSetItem(storage: Storage, key: string, value: string): void {
  try { storage.setItem(key, value) } catch { /* Safari private browsing */ }
}

export function usePlayer(opts: UsePlayerOptions = {}) {
  const { enableKeyboard = true, enableAutoHide = true } = opts

  // --- Core refs ---
  const videoRef = ref<HTMLVideoElement | null>(null)
  const containerRef = ref<HTMLElement | null>(null)
  const src = ref<string | null>(null)
  const tracks = ref<TrackInfo[]>([])
  const isLive = ref(false)

  const state = ref<PlayerState>('idle')
  const isPlaying = ref(false)
  const currentTime = ref(0)
  const duration = ref(0)
  const autoplayAllowed = ref(safeGetItem(sessionStorage, 'player_autoplay_allowed') === 'true')
  const aspectFit = ref<'contain' | 'cover' | 'fill'>(
    (safeGetItem(localStorage, 'player_aspect_fit') as 'contain' | 'cover' | 'fill') || 'contain'
  )

  // --- Volume (delegated) ---
  const vol = usePlayerVolume(videoRef)

  const volumeStage = computed(() => {
    const v = vol.volume.value
    if (vol.isMuted.value || v === 0) return 0
    if (v <= 0.25) return 1
    if (v <= 0.50) return 2
    if (v <= 0.75) return 3
    if (v <= 1.00) return 4
    return 5
  })
  const isVolumeBoosted = computed(() => vol.volume.value > 1)

  const playbackRate = ref(1)
  const buffered = ref<{ start: number; end: number }[]>([])
  const videoWidth = ref(0)
  const videoHeight = ref(0)
  const droppedFrames = ref(0)

  // A-B loop
  const loopA = ref<number | null>(null)
  const loopB = ref<number | null>(null)
  const loopEnabled = ref(false)

  // Seek indicator overlay
  const seekIndicator = ref<{ dir: 'forward' | 'backward'; amount: number } | null>(null)
  let seekIndicatorTimer: ReturnType<typeof setTimeout> | null = null

  // Live stream
  const liveDuration = ref(0)
  const liveEdge = ref(0)
  const liveStart = ref(0)
  const liveThreshold = ref(parseFloat(safeGetItem(localStorage, 'player_live_threshold', '10')))
  const isBehind = computed(() => isLive.value && liveEdge.value > 0 && (liveEdge.value - currentTime.value) > liveThreshold.value)

  // Debug
  const showDebug = ref(safeGetItem(localStorage, 'player_show_debug') === 'true')
  watch(showDebug, (val) => { safeSetItem(localStorage, 'player_show_debug', String(val)) })

  watch(aspectFit, (val) => { safeSetItem(localStorage, 'player_aspect_fit', val) })

  // Controls visibility
  const controlsVisible = ref(true)
  const showSettings = ref(false)
  let hideTimer: ReturnType<typeof setTimeout> | null = null

  // Dynamic seek acceleration
  const lastSeekTime = ref(0)
  const seekCount = ref(0)
  let seekDecayTimer: ReturnType<typeof setTimeout> | null = null

  // quality/track
  const activeTrackId = ref(0)

  // HLS instance
  let hlsInstance: Hls | null = null
  const hlsLevels = ref<{ width: number; height: number; bitrate: number; id: number }[]>([])
  const currentHlsLevel = ref<number>(-1)
  const hlsBandwidthEstimate = ref(0)
  const hlsBufferLength = ref(0)
  const hlsLiveLatency = ref(-1)
  const hlsLevelWidth = ref(0)
  const hlsLevelHeight = ref(0)
  const hlsLevelBitrate = ref(0)

  // --- Computed ---
  const progress = computed(() => {
    if (isLive.value && liveEdge.value > liveStart.value) {
      return (currentTime.value - liveStart.value) / (liveEdge.value - liveStart.value)
    }
    if (duration.value <= 0 || !isFinite(duration.value)) return 0
    return currentTime.value / duration.value
  })

  const bufferedEnd = computed(() => {
    const b = buffered.value
    if (b.length === 0) return 0
    if (isLive.value) return b[b.length - 1].end - liveStart.value
    return b[b.length - 1].end
  })

  const progressBarDuration = computed(() => {
    if (isLive.value && liveEdge.value > liveStart.value) return liveEdge.value - liveStart.value
    if (!isFinite(duration.value)) return liveDuration.value || 0
    return duration.value
  })

  const formattedCurrentTime = computed(() => formatTime(currentTime.value))
  const formattedDuration = computed(() => formatTime(duration.value))

  const activeTrack = computed(() => tracks.value.find(t => t.track_id === activeTrackId.value) ?? null)

  // --- Internal ---
  function formatTime(t: number): string {
    if (!isFinite(t) || t < 0) return '0:00'
    const hrs = Math.floor(t / 3600)
    const mins = Math.floor((t % 3600) / 60)
    const secs = Math.floor(t % 60)
    if (hrs > 0) return `${hrs}:${String(mins).padStart(2, '0')}:${String(secs).padStart(2, '0')}`
    return `${mins}:${String(secs).padStart(2, '0')}`
  }

  // --- HLS lifecycle ---
  function setupHls(url: string) {
    destroyHls()
    if (!videoRef.value) return

    if (Hls.isSupported()) {
      const hls = new Hls({
        enableWorker: true,
        lowLatencyMode: !isLive.value,
      })

      hls.on(Hls.Events.ERROR, (_e, data) => {
        if (data.fatal) {
          if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
            hls.startLoad()
            return
          }
          state.value = 'error'
        }
      })

      hls.on(Hls.Events.MANIFEST_PARSED, () => {
        state.value = isPlaying.value ? 'playing' : 'paused'
        hlsLevels.value = hls.levels.map((l, i) => ({
          width: l.width, height: l.height, bitrate: l.bitrate, id: i,
        }))
      })

      hls.on(Hls.Events.LEVEL_SWITCHED, (_e, data) => {
        currentHlsLevel.value = data.level
        const l = hls.levels[data.level]
        if (l) {
          hlsLevelWidth.value = l.width
          hlsLevelHeight.value = l.height
          hlsLevelBitrate.value = l.bitrate
        }
      })

      hls.on(Hls.Events.BUFFER_APPENDED, () => { updateBuffered() })

      hls.on(Hls.Events.FRAG_LOADING, () => {
        if (state.value === 'playing') return
        state.value = 'buffering'
      })

      hls.on(Hls.Events.FRAG_BUFFERED, () => {
        hlsBandwidthEstimate.value = hls.bandwidthEstimate || 0
        updateBuffered()
        updateLiveBounds()
      })

      hls.on(Hls.Events.LEVEL_UPDATED, () => { updateLiveBounds() })

      hls.loadSource(url)
      hls.attachMedia(videoRef.value)
      hlsInstance = hls

      videoRef.value.play().catch(() => {})
    } else if (videoRef.value.canPlayType('application/vnd.apple.mpegurl')) {
      videoRef.value.src = url
    } else {
      state.value = 'error'
    }
  }

  function destroyHls() {
    if (hlsInstance) {
      hlsInstance.detachMedia()
      hlsInstance.destroy()
      hlsInstance = null
    }
    hlsLevels.value = []
    currentHlsLevel.value = -1
    hlsBandwidthEstimate.value = 0
    hlsBufferLength.value = 0
    hlsLiveLatency.value = -1
    hlsLevelWidth.value = 0
    hlsLevelHeight.value = 0
    hlsLevelBitrate.value = 0
  }

  function updateLiveBounds() {
    if (!isLive.value) return
    const buf = buffered.value
    const last = buf[buf.length - 1]
    const first = buf[0]
    if (last) liveEdge.value = last.end
    liveStart.value = first ? first.start : 0
  }

  function updateBuffered() {
    const v = videoRef.value
    if (!v || !v.buffered || v.buffered.length === 0) { buffered.value = []; return }
    const ranges: { start: number; end: number }[] = []
    for (let i = 0; i < v.buffered.length; i++) {
      ranges.push({ start: v.buffered.start(i), end: v.buffered.end(i) })
    }
    buffered.value = ranges
  }

  // --- Player methods ---
  function loadSource(url: string | null, isLiveStream = false, preserveLoop = false): boolean {
    src.value = url
    isLive.value = isLiveStream
    state.value = 'loading'
    isPlaying.value = false
    currentTime.value = 0
    duration.value = 0
    buffered.value = []
    droppedFrames.value = 0
    if (!preserveLoop) {
      loopA.value = null; loopB.value = null; loopEnabled.value = false
    }
    if (!url || !videoRef.value) { state.value = 'idle'; destroyHls(); return false }

    destroyHls()
    hlsBandwidthEstimate.value = 0; hlsBufferLength.value = 0; hlsLiveLatency.value = -1
    hlsLevelWidth.value = 0; hlsLevelHeight.value = 0; hlsLevelBitrate.value = 0
    startDebugPoll()
    if (videoRef.value) { videoRef.value.removeAttribute('src'); videoRef.value.load() }

    const isHls = url.includes('.m3u8')
    if (isHls) { setupHls(url) } else { videoRef.value!.src = url; state.value = 'playing' }
    return true
  }

  function play() {
    if (!videoRef.value) return
    if (loopA.value !== null && videoRef.value.readyState >= 1) {
      videoRef.value.currentTime = loopA.value
    }
    videoRef.value.play().then(() => { state.value = 'playing'; isPlaying.value = true }).catch(() => {})
  }

  function pause() {
    if (!videoRef.value) return
    videoRef.value.pause()
    state.value = 'paused'; isPlaying.value = false
  }

  function togglePlay() {
    if (state.value === 'ended') { seekTo(0); play(); return }
    if (isPlaying.value) pause(); else play()
  }

  function seekTo(time: number) {
    if (!videoRef.value) return
    const seekMax = isLive.value && liveEdge.value > 0 ? liveEdge.value : (isFinite(duration.value) ? duration.value : 0)
    if (seekMax <= 0) return
    const seekMin = isLive.value && buffered.value.length > 0 ? buffered.value[0].start : 0
    const clamped = Math.max(seekMin, Math.min(time, seekMax))
    videoRef.value.currentTime = clamped
    currentTime.value = clamped
  }

  function showSeekIndicator(dir: 'forward' | 'backward', amount: number) {
    seekIndicator.value = { dir, amount }
    if (seekIndicatorTimer) clearTimeout(seekIndicatorTimer)
    seekIndicatorTimer = setTimeout(() => { seekIndicator.value = null }, 800)
  }

  function seekRelative(seconds: number) {
    const prev = currentTime.value
    seekTo(prev + seconds)
    const jumped = Math.round(Math.abs(currentTime.value - prev))
    if (jumped > 0) showSeekIndicator(seconds > 0 ? 'forward' : 'backward', jumped)
  }

  function dynamicSeek(dir: SeekDirection) {
    const now = Date.now()
    const delta = now - lastSeekTime.value
    lastSeekTime.value = now
    if (delta < 800) { seekCount.value = Math.min(seekCount.value + 1, 6) } else { seekCount.value = 1 }
    if (seekDecayTimer) clearTimeout(seekDecayTimer)
    seekDecayTimer = setTimeout(() => { seekCount.value = 0 }, 1500)
    const baseJump = dir === 'forward' ? 5 : -5
    const jump = baseJump * seekCount.value
    const prev = currentTime.value
    seekTo(currentTime.value + jump)
    const jumped = Math.round(Math.abs(currentTime.value - prev))
    if (jumped > 0) showSeekIndicator(dir, jumped)
  }

  async function requestAutoplayPermission() {
    if (autoplayAllowed.value) return
    try {
      const ctx = new (window.AudioContext || (window as any).webkitAudioContext)()
      await ctx.resume()
      ctx.close()
      autoplayAllowed.value = true
      safeSetItem(sessionStorage, 'player_autoplay_allowed', 'true')
    } catch { /* permission not granted */ }
  }

  function setPlaybackRate(rate: number) {
    if (!videoRef.value) return
    videoRef.value.playbackRate = rate
    videoRef.value.preservesPitch = true
    playbackRate.value = rate
  }

  function setAspectFit(val: 'contain' | 'cover' | 'fill') { aspectFit.value = val }

  // A-B loop
  function setLoopA() {
    loopA.value = currentTime.value
    if (loopB.value !== null && loopA.value >= loopB.value) {
      const tmp = loopA.value; loopA.value = loopB.value; loopB.value = tmp
    }
  }
  function setLoopB() {
    loopB.value = currentTime.value
    if (loopA.value !== null && loopB.value <= loopA.value) {
      const tmp = loopA.value; loopA.value = loopB.value; loopB.value = tmp
    }
  }
  function clearLoop() { loopA.value = null; loopB.value = null; loopEnabled.value = false }
  function setLoopEnabled(val: boolean) { loopEnabled.value = val }

  // Live edge
  function seekToLiveEdge() { if (liveEdge.value > 0) seekTo(liveEdge.value - 1) }
  function setLiveThreshold(seconds: number) {
    liveThreshold.value = Math.max(1, Math.min(60, seconds))
    safeSetItem(localStorage, 'player_live_threshold', String(liveThreshold.value))
  }

  // Track / quality switching
  function selectTrack(trackId: number) {
    const track = tracks.value.find(t => t.track_id === trackId)
    if (!track) return
    activeTrackId.value = trackId
    if (track.hls_url) loadSource(track.hls_url, isLive.value)
  }

  // Fullscreen
  function toggleFullscreen() {
    if (!containerRef.value) return
    if (document.fullscreenElement) { document.exitFullscreen() } else { containerRef.value.requestFullscreen() }
  }

  // Controls visibility
  function showControls() {
    controlsVisible.value = true
    if (enableAutoHide && isPlaying.value) resetHideTimer()
  }
  function hideControls() { if (showSettings.value) return; controlsVisible.value = false }
  function resetHideTimer() {
    if (hideTimer) clearTimeout(hideTimer)
    hideTimer = setTimeout(() => { if (!showSettings.value) hideControls() }, 2500)
  }
  function onMouseMove() {
    if (!controlsVisible.value) showControls()
    else if (enableAutoHide && isPlaying.value) resetHideTimer()
  }

  // --- Keyboard (delegated) ---
  const handleKeydown = createKeydownHandler({
    enableKeyboard,
    state,
    volume: vol.volume,
    playbackRate,
    duration,
    currentTime,
    lastSeekTime,
    seekCount,
    seekDecayTimer: { get value() { return seekDecayTimer }, set value(v) { seekDecayTimer = v } },
    togglePlay,
    dynamicSeek,
    seekRelative,
    setVolume: vol.setVolume,
    toggleMute: vol.toggleMute,
    toggleFullscreen,
    showControls,
    setPlaybackRate,
    seekTo,
  })

  // --- Gestures (delegated) ---
  const gestures = createTouchHandlers({
    containerRef,
    dynamicSeek,
    togglePlay,
    showControls,
  })

  // --- Video event handlers ---
  function onVideoPlay() { isPlaying.value = true; state.value = 'playing'; showControls() }
  function onVideoPause() { isPlaying.value = false; state.value = 'paused'; controlsVisible.value = true }
  function onVideoEnded() {
    if (loopEnabled.value) { seekTo(loopA.value ?? 0); play(); return }
    isPlaying.value = false; state.value = 'ended'; controlsVisible.value = true
    if (hideTimer) clearTimeout(hideTimer)
  }
  function onVideoTimeUpdate() {
    const v = videoRef.value; if (!v) return
    currentTime.value = v.currentTime; updateBuffered()
    if (isLive.value) {
      updateLiveBounds()
      hlsLiveLatency.value = liveEdge.value > 0 ? liveEdge.value - currentTime.value : -1
      hlsBufferLength.value = liveEdge.value > 0 ? Math.max(0, liveEdge.value - currentTime.value) : 0
    }
    if (loopB.value !== null && v.currentTime >= loopB.value) {
      if (loopEnabled.value) { v.currentTime = loopA.value ?? 0 } else { v.currentTime = loopB.value; pause() }
    }
  }
  function onVideoLoadedMetadata() {
    const v = videoRef.value; if (!v) return
    duration.value = v.duration || 0; videoWidth.value = v.videoWidth || 0; videoHeight.value = v.videoHeight || 0
    v.volume = Math.min(1, vol.volume.value); setPlaybackRate(playbackRate.value); state.value = 'playing'
    if (loopA.value !== null) v.currentTime = loopA.value
    if ('webkitDroppedFrameCount' in v) {
      const pollDropped = () => {
        droppedFrames.value = (v as any).webkitDroppedFrameCount || 0
        if (isPlaying.value) requestAnimationFrame(pollDropped)
      }
      requestAnimationFrame(pollDropped)
    }
  }
  function onVideoWaiting() { if (isPlaying.value) state.value = 'buffering' }
  function onVideoCanPlay() { if (state.value === 'buffering' || state.value === 'loading') state.value = isPlaying.value ? 'playing' : 'paused' }
  function onVideoError() { state.value = 'error' }

  function attachVideoEvents() {
    const v = videoRef.value; if (!v) return
    v.addEventListener('play', onVideoPlay); v.addEventListener('pause', onVideoPause)
    v.addEventListener('ended', onVideoEnded); v.addEventListener('timeupdate', onVideoTimeUpdate)
    v.addEventListener('loadedmetadata', onVideoLoadedMetadata); v.addEventListener('waiting', onVideoWaiting)
    v.addEventListener('canplay', onVideoCanPlay); v.addEventListener('error', onVideoError)
    v.addEventListener('progress', updateBuffered)
  }
  function detachVideoEvents() {
    const v = videoRef.value; if (!v) return
    v.removeEventListener('play', onVideoPlay); v.removeEventListener('pause', onVideoPause)
    v.removeEventListener('ended', onVideoEnded); v.removeEventListener('timeupdate', onVideoTimeUpdate)
    v.removeEventListener('loadedmetadata', onVideoLoadedMetadata); v.removeEventListener('waiting', onVideoWaiting)
    v.removeEventListener('canplay', onVideoCanPlay); v.removeEventListener('error', onVideoError)
    v.removeEventListener('progress', updateBuffered)
  }

  // Debug poll
  let debugPollTimer: ReturnType<typeof setInterval> | null = null
  function startDebugPoll() {
    stopDebugPoll()
    debugPollTimer = setInterval(() => {
      if (!videoRef.value) return; updateBuffered()
      if (isLive.value) {
        updateLiveBounds()
        hlsLiveLatency.value = liveEdge.value > 0 ? liveEdge.value - currentTime.value : -1
        hlsBufferLength.value = liveEdge.value > 0 ? Math.max(0, liveEdge.value - currentTime.value) : 0
      }
    }, 1000)
  }
  function stopDebugPoll() { if (debugPollTimer) { clearInterval(debugPollTimer); debugPollTimer = null } }

  // Cleanup
  function destroy() {
    stopDebugPoll(); destroyHls(); vol.destroyAudioBoost(); detachVideoEvents()
    if (hideTimer) clearTimeout(hideTimer)
    if (videoRef.value) { videoRef.value.pause(); videoRef.value.removeAttribute('src'); videoRef.value.load() }
  }

  return {
    // refs
    videoRef, containerRef, src, tracks, isLive, state, isPlaying, currentTime, duration,
    volume: vol.volume, displayVolume: vol.displayVolume, volumeStage, isVolumeBoosted,
    volumeBoostEnabled: vol.volumeBoostEnabled, isMuted: vol.isMuted, playbackRate,
    buffered, videoWidth, videoHeight, droppedFrames,
    loopA, loopB, loopEnabled, showDebug, controlsVisible, showSettings,
    hlsLevels, currentHlsLevel, hlsBandwidthEstimate, hlsBufferLength, hlsLiveLatency,
    hlsLevelWidth, hlsLevelHeight, hlsLevelBitrate, activeTrackId, activeTrack,
    progress, bufferedEnd, progressBarDuration, formattedCurrentTime, formattedDuration,
    seekIndicator, liveDuration, liveEdge, liveStart, liveThreshold, isBehind,
    seekToLiveEdge, setLiveThreshold, toggleVolumeBoost: vol.toggleVolumeBoost, aspectFit,
    // methods
    loadSource, play, pause, togglePlay, seekTo, seekRelative, dynamicSeek,
    setVolume: vol.setVolume, toggleMute: vol.toggleMute, setPlaybackRate, setAspectFit,
    setLoopA, setLoopB, setLoopEnabled, clearLoop, selectTrack, toggleFullscreen,
    showControls, hideControls, onMouseMove,
    handleKeydown,
    handleTouchStart: gestures.handleTouchStart,
    handleTouchEnd: gestures.handleTouchEnd,
    handleClick: gestures.handleClick,
    attachVideoEvents, detachVideoEvents, destroy,
    autoplayAllowed, requestAutoplayPermission,
  }
}
