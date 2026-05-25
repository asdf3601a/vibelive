/**
 * Centralized type definitions for the LiveStream Platform frontend.
 */

/** Metadata extracted from an active RTMP stream. */
export interface StreamMetadata {
  width: number
  height: number
  video_codec: string
  audio_codec: string
  video_bitrate?: number
  audio_bitrate?: number
  framerate?: number
}

/** Represents a single live stream. */
export interface Stream {
  stream_key: string
  status: 'live' | 'ended'
  started_at: string | null
  metadata: StreamMetadata | null
  hls_url: string | null
  player_url: string | null
}

/** Represents a saved recording (fMP4 → MP4). */
export interface Recording {
  filename: string
  stream_key: string
  created_at: string
  size_bytes: number
  duration_seconds?: number
  url: string
  thumbnail_url: string
  thumbnails: Record<string, string>
}

/** Server health-check response. */
export interface HealthStatus {
  status: string
}
