/**
 * Thin typed fetch wrapper with centralized error handling.
 */

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

/**
 * Perform a typed JSON API request.
 */
export async function apiFetch<T>(path: string, opts?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    headers: { Accept: 'application/json' },
    ...opts,
  })

  if (!res.ok) {
    let message = `Request failed with ${res.status}`
    try {
      const body = await res.json()
      if (body && typeof body.error === 'string') {
        message = body.error
      }
    } catch {
      /* ignore non-JSON error bodies */
    }
    throw new ApiError(res.status, message)
  }

  return res.json() as Promise<T>
}
