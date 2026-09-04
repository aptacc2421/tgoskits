//! REST client for the axvisor control plane.
//!
//! Paths are relative (the dashboard is served from the same origin), so the dev
//! server proxy and the bundle embedded in the hypervisor both work without a
//! build-time base URL. The method set stays generic — adding a panel does not
//! change this module.

import { useRef } from 'react'
import { ApiError } from './types'

export class ApiClient {
  get<T>(path: string, signal?: AbortSignal): Promise<T> {
    return this.request<T>('GET', path, undefined, signal)
  }

  post<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return this.request<T>('POST', path, body, signal)
  }

  /** `DELETE` returns 204 with an empty body, so the caller gets the status only. */
  async del(path: string, signal?: AbortSignal): Promise<number> {
    const res = await fetch(path, { method: 'DELETE', signal })
    if (!res.ok) throw await parseError(res)
    return res.status
  }

  private async request<T>(
    method: 'GET' | 'POST',
    path: string,
    body: unknown,
    signal?: AbortSignal,
  ): Promise<T> {
    const headers: Record<string, string> = { Accept: 'application/json' }
    const init: RequestInit = { method, headers, signal }
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json'
      init.body = JSON.stringify(body)
    }

    const res = await fetch(path, init)
    if (!res.ok) throw await parseError(res)
    // A successful response with no body would break `res.json()`; the control
    // plane always answers with JSON on 2xx, so an empty body is treated as such.
    const text = await res.text()
    return (text.length > 0 ? JSON.parse(text) : undefined) as T
  }
}

/**
 * The body of a failed response is read exactly once here, then presented as two
 * dimensions: the HTTP status and the backend's `error` field when it has one.
 */
async function parseError(res: Response): Promise<ApiError> {
  const raw = await res.text()
  let detail = raw
  try {
    const parsed: unknown = JSON.parse(raw)
    if (parsed !== null && typeof parsed === 'object' && 'error' in parsed) {
      const value = (parsed as { error: unknown }).error
      if (typeof value === 'string') detail = value
    }
  } catch {
    // Not JSON: keep the text as-is so the context is not lost.
  }
  return new ApiError(res.status, detail.trim() || res.statusText)
}

/**
 * One client instance for the lifetime of the shell: it holds no state, but
 * keeping the identity stable keeps panel effects from re-running on every render.
 */
export function useApiClient(): ApiClient {
  const ref = useRef<ApiClient | null>(null)
  if (ref.current === null) ref.current = new ApiClient()
  return ref.current
}
