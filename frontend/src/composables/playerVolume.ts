import { ref } from 'vue'

function safeGetItem(storage: Storage, key: string, fallback: string = ''): string {
  try { return storage.getItem(key) ?? fallback } catch { return fallback }
}

function safeSetItem(storage: Storage, key: string, value: string): void {
  try { storage.setItem(key, value) } catch { /* Safari private browsing */ }
}

export function usePlayerVolume(videoRef: { value: HTMLVideoElement | null }) {
  const volume = ref(Math.min(1.5, parseFloat(safeGetItem(localStorage, 'player_volume', '1'))))
  const previousVolume = ref(1)
  const isMuted = ref(false)
  const displayVolume = ref(volume.value)
  const volumeBoostEnabled = ref(safeGetItem(localStorage, 'player_volume_boost') === 'true')

  let audioCtx: AudioContext | null = null
  let gainNode: GainNode | null = null
  let audioBoostConnected = false

  function setupAudioBoost() {
    if (audioBoostConnected || !videoRef.value) return
    try {
      audioCtx = new (window.AudioContext || (window as any).webkitAudioContext)()
      const gain = audioCtx.createGain()
      gain.gain.value = 1
      const source = audioCtx.createMediaElementSource(videoRef.value)
      source.connect(gain)
      gain.connect(audioCtx.destination)
      gainNode = gain
      audioBoostConnected = true
      videoRef.value.volume = 1
    } catch {
      audioBoostConnected = false
    }
  }

  function destroyAudioBoost() {
    if (audioCtx) {
      audioCtx.close()
      audioCtx = null
    }
    gainNode = null
    audioBoostConnected = false
  }

  function setVolume(v: number) {
    const maxVol = volumeBoostEnabled.value ? 1.5 : 1
    let clamped = Math.max(0, Math.min(maxVol, v))

    if (volumeBoostEnabled.value && clamped > 0.95 && clamped < 1.05) {
      clamped = 1
    }

    displayVolume.value = clamped
    volume.value = clamped
    if (clamped > 0) {
      isMuted.value = false
      previousVolume.value = clamped
    }
    safeSetItem(localStorage, 'player_volume', String(clamped))

    const video = videoRef.value
    if (!video) return

    video.muted = clamped === 0

    if (clamped > 1) {
      if (!audioBoostConnected) setupAudioBoost()
      if (gainNode && audioCtx) {
        video.volume = 1
        if (audioCtx.state === 'suspended') audioCtx.resume()
        gainNode.gain.value = clamped
        return
      }
    }

    if (gainNode && audioCtx) {
      gainNode.gain.value = Math.min(1, clamped)
      return
    }

    video.volume = Math.min(1, clamped)
  }

  function toggleMute() {
    if (isMuted.value) {
      const restore = previousVolume.value || 1
      setVolume(restore)
      isMuted.value = false
    } else {
      previousVolume.value = volume.value || 1
      setVolume(0)
      isMuted.value = true
    }
  }

  function toggleVolumeBoost() {
    volumeBoostEnabled.value = !volumeBoostEnabled.value
    safeSetItem(localStorage, 'player_volume_boost', String(volumeBoostEnabled.value))
    if (!volumeBoostEnabled.value && volume.value > 1) {
      setVolume(1)
    }
  }

  return {
    volume,
    previousVolume,
    isMuted,
    displayVolume,
    volumeBoostEnabled,
    setVolume,
    toggleMute,
    toggleVolumeBoost,
    setupAudioBoost,
    destroyAudioBoost,
  }
}
