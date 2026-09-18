//! WebSocket plumbing shared by the VM registry feed and every terminal.
//!
//! Two rules come from the axvisor side and shape this module:
//!
//! - a console socket accepts at most 4096 bytes per frame
//!   (`BROWSER_INPUT_CAPACITY`), so user input is chunked on **character**
//!   boundaries — a multi-byte character must not be split across frames;
//! - a browser WebSocket reports every failed handshake as 1006, so "this lane
//!   is already taken" (the server's 409) is discovered with a plain `fetch`
//!   probe against the same route before upgrading.

/** Same-origin `ws(s)` URL: no token, no query, no build-time host. */
export function wsUrl(path: string): string {
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${scheme}//${location.host}${path}`
}

/** The `http(s)` form of the same route, for the pre-upgrade probe. */
function httpUrl(path: string): string {
  const scheme = location.protocol === 'https:' ? 'https:' : 'http:'
  return `${scheme}//${location.host}${path}`
}

export type SocketStatus = 'connecting' | 'open' | 'busy' | 'closed'

export interface SocketHandlers {
  /** Decoded text of one data frame (the console lanes send binary frames). */
  onData: (text: string) => void
  onStatus: (status: SocketStatus, detail?: string) => void
}

/** Longest frame the console gateway accepts, one frame per chunk. */
const MAX_FRAME_BYTES = 4096

/**
 * A console socket with chunked writes, streaming UTF-8 decoding and a manual
 * lifecycle: the owner decides when to reconnect, so a lane that disappeared
 * (its VM was closed) is not hammered forever.
 */
export class ConsoleSocket {
  private socket: WebSocket | null = null
  private closedByUs = false
  private readonly encoder = new TextEncoder()
  private readonly decoder = new TextDecoder()

  constructor(
    private readonly path: string,
    private readonly handlers: SocketHandlers,
  ) {
    void this.connect()
  }

  /** Sends user input as one or more binary frames, never splitting a character. */
  send(text: string): void {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return
    let chunk: number[] = []
    for (const character of text) {
      const bytes = this.encoder.encode(character)
      if (chunk.length + bytes.length > MAX_FRAME_BYTES) {
        this.socket.send(new Uint8Array(chunk))
        chunk = []
      }
      chunk.push(...bytes)
    }
    if (chunk.length > 0) this.socket.send(new Uint8Array(chunk))
  }

  close(): void {
    this.closedByUs = true
    this.socket?.close()
    this.socket = null
  }

  private async connect(): Promise<void> {
    this.handlers.onStatus('connecting')

    try {
      const probe = await fetch(httpUrl(this.path))
      if (probe.status === 409) {
        // The lane exists but someone else holds it: the console lanes are exclusive.
        this.handlers.onStatus('busy', '该终端已被占用（独占订阅）')
        return
      }
    } catch {
      // A failed probe (the server may be restarting) must not block the upgrade
      // attempt: let the WebSocket report its own error.
    }
    if (this.closedByUs) return

    const socket = new WebSocket(wsUrl(this.path))
    this.socket = socket
    socket.binaryType = 'arraybuffer'

    socket.onopen = () => this.handlers.onStatus('open')
    socket.onmessage = (event: MessageEvent) => {
      const text =
        typeof event.data === 'string'
          ? event.data
          : this.decoder.decode(event.data as ArrayBuffer, { stream: true })
      this.handlers.onData(text)
    }
    socket.onerror = () => {
      if (!this.closedByUs) this.handlers.onStatus('closed', '连接错误')
    }
    socket.onclose = () => {
      if (!this.closedByUs) this.handlers.onStatus('closed', '连接已断开')
    }
  }
}
