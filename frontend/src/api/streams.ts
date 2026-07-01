import type { Stream, Recording, HealthStatus } from '../types'
import { apiFetch } from './client'

export function listStreams(): Promise<Stream[]> {
  return apiFetch<Stream[]>('/api/streams')
}

export function getStream(key: string): Promise<Stream> {
  return apiFetch<Stream>(`/api/streams/${encodeURIComponent(key)}`)
}

export function getHealth(): Promise<HealthStatus> {
  return apiFetch<HealthStatus>('/api/health')
}

export function getPlayerUrl(key: string): string {
  return `/live/${encodeURIComponent(key)}`
}

// Recordings endpoints (require server-side support)
export function listRecordings(): Promise<Recording[]> {
  return apiFetch<Recording[]>('/api/recordings')
}
