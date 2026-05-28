import type { Stream, Recording, HealthStatus } from '../types'
import { apiFetch } from './client'

export interface ServerConfig {
  rtmp_url_template: string
  multitrack_supported: boolean
  enhanced_rtmp: boolean
  supported_video_codecs: string[]
  supported_audio_codecs: string[]
  example_ffmpeg_single: string
  example_ffmpeg_multitrack: string
}

export function listStreams(): Promise<Stream[]> {
  return apiFetch<Stream[]>('/api/streams')
}

export function getStream(key: string): Promise<Stream> {
  return apiFetch<Stream>(`/api/streams/${encodeURIComponent(key)}`)
}

export function getHealth(): Promise<HealthStatus> {
  return apiFetch<HealthStatus>('/api/health')
}

export function getConfig(): Promise<ServerConfig> {
  return apiFetch<ServerConfig>('/api/config')
}

export function getPlayerUrl(key: string): string {
  return `/live/${encodeURIComponent(key)}`
}

// Recordings endpoints (require server-side support)
export function listRecordings(): Promise<Recording[]> {
  return apiFetch<Recording[]>('/api/recordings')
}

export function getRecordingUrl(filename: string): string {
  return `/api/recordings/${encodeURIComponent(filename)}`
}
