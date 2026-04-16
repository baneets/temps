//! Wire protocol between the host-side handler and the in-sandbox agent.
//!
//! Frame: `u32 length (BE) | u8 type | payload`. The `length` field counts
//! `type + payload`, so the wire-side reader reads 4 bytes, then exactly
//! `length` more bytes. Control messages (OPEN, OPENED, TABS, EXIT, ERROR)
//! carry a JSON payload; data messages (INPUT, OUTPUT) carry raw bytes; RESIZE
//! carries a fixed 4-byte `u16 cols | u16 rows`. Binary-safe, no escaping.
//!
//! See docs/adr/008-pty-agent.md for the full design.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Hard cap on a single frame, matches the websocket frame cap in the
/// terminal handler. Prevents a malicious or buggy client from asking the
/// agent to allocate gigabytes before we know what's in the frame.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

// Client → Agent
pub const OP_OPEN: u8 = 0x01;
pub const OP_INPUT: u8 = 0x02;
pub const OP_RESIZE: u8 = 0x03;
pub const OP_DETACH: u8 = 0x04;
pub const OP_KILL: u8 = 0x05;
pub const OP_LIST: u8 = 0x06;
pub const OP_PING: u8 = 0x07;

// Agent → Client
pub const OP_OUTPUT: u8 = 0x81;
pub const OP_OPENED: u8 = 0x82;
pub const OP_EXIT: u8 = 0x83;
pub const OP_TABS: u8 = 0x84;
pub const OP_PONG: u8 = 0x85;
pub const OP_ERROR: u8 = 0x8f;

/// Payload for `OPEN`. `cmd` is the shell to run under the PTY; agent does
/// not interpret it — caller ships the same string the dtach wrapper would
/// have used. `replay_bytes` is clamped server-side to the ring size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRequest {
    pub tab_id: String,
    pub kind: String,
    pub cmd: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub replay_bytes: u32,
    /// Optional label for enumeration; host sets this from the frontend tab
    /// label so `LIST` can populate the tab strip on session open without
    /// round-tripping to the frontend state.
    #[serde(default)]
    pub label: Option<String>,
    /// Working directory for the spawned shell. Defaults to `/workspace`
    /// (the sandbox mount point) when absent so production callers don't
    /// have to set it.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Environment pairs to hand to the child. When absent the agent uses
    /// its default (TERM + sandbox PATH + HOME). Tests override this to
    /// supply a minimal env that works on the test host.
    #[serde(default)]
    pub env: Option<Vec<(String, String)>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenedResponse {
    pub tab_id: String,
    pub pid: i32,
    /// True if this OPEN attached to a pre-existing tab rather than creating
    /// one. Host can distinguish "re-attach" (no welcome banner needed) from
    /// "fresh spawn" (banner desired) without sniffing PTY output.
    pub existed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitEvent {
    pub tab_id: String,
    /// Unix exit code if the child exited normally.
    pub code: Option<i32>,
    /// Signal number if the child died by signal.
    pub signal: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabInfo {
    pub tab_id: String,
    pub kind: String,
    pub label: Option<String>,
    pub pid: i32,
    pub created_at_ms: u64,
    pub subscriber_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("io error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
    #[error("frame too large: {size} bytes")]
    Oversized { size: usize },
    #[error("short frame: expected at least 1 byte for type")]
    Empty,
    #[error("json decode failed: {source}")]
    Json {
        #[from]
        source: serde_json::Error,
    },
}

/// Read one frame from `r`. Returns `(type_byte, payload)`. Returns
/// `Ok(None)` on clean EOF before any bytes arrive (the peer closed the
/// socket between frames — normal shutdown). Returns `Err` if EOF hits
/// mid-frame (truncated) or the length header is over the cap.
pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
) -> Result<Option<(u8, Vec<u8>)>, FrameError> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Err(FrameError::Empty);
    }
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized { size: len });
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).await?;
    let type_byte = payload[0];
    payload.remove(0);
    Ok(Some((type_byte, payload)))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    type_byte: u8,
    payload: &[u8],
) -> Result<(), FrameError> {
    let len = 1 + payload.len();
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized { size: len });
    }
    let header = (len as u32).to_be_bytes();
    w.write_all(&header).await?;
    w.write_all(&[type_byte]).await?;
    if !payload.is_empty() {
        w.write_all(payload).await?;
    }
    w.flush().await?;
    Ok(())
}

/// Shorthand for JSON-typed payloads.
pub async fn write_json_frame<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    type_byte: u8,
    value: &T,
) -> Result<(), FrameError> {
    let bytes = serde_json::to_vec(value)?;
    write_frame(w, type_byte, &bytes).await
}

/// Decode the RESIZE payload: `u16 cols | u16 rows`.
pub fn decode_resize(payload: &[u8]) -> Option<(u16, u16)> {
    if payload.len() != 4 {
        return None;
    }
    let cols = u16::from_be_bytes([payload[0], payload[1]]);
    let rows = u16::from_be_bytes([payload[2], payload[3]]);
    Some((cols, rows))
}

pub fn encode_resize(cols: u16, rows: u16) -> [u8; 4] {
    let mut out = [0u8; 4];
    out[0..2].copy_from_slice(&cols.to_be_bytes());
    out[2..4].copy_from_slice(&rows.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn roundtrip_raw_frame() {
        let (mut a, mut b) = duplex(64 * 1024);
        let payload = b"hello world".to_vec();
        tokio::spawn(async move {
            write_frame(&mut a, OP_OUTPUT, &payload).await.unwrap();
        });
        let (ty, got) = read_frame(&mut b).await.unwrap().unwrap();
        assert_eq!(ty, OP_OUTPUT);
        assert_eq!(got, b"hello world");
    }

    #[tokio::test]
    async fn roundtrip_json_frame() {
        let (mut a, mut b) = duplex(64 * 1024);
        let req = OpenRequest {
            tab_id: "main".into(),
            kind: "claude".into(),
            cmd: "bash".into(),
            cols: 80,
            rows: 24,
            replay_bytes: 4096,
            label: Some("claude".into()),
            cwd: None,
            env: None,
        };
        let req_clone = req.clone();
        tokio::spawn(async move {
            write_json_frame(&mut a, OP_OPEN, &req_clone).await.unwrap();
        });
        let (ty, got) = read_frame(&mut b).await.unwrap().unwrap();
        assert_eq!(ty, OP_OPEN);
        let decoded: OpenRequest = serde_json::from_slice(&got).unwrap();
        assert_eq!(decoded.tab_id, req.tab_id);
        assert_eq!(decoded.cols, 80);
    }

    #[tokio::test]
    async fn resize_payload_roundtrip() {
        let p = encode_resize(214, 22);
        let (c, r) = decode_resize(&p).unwrap();
        assert_eq!((c, r), (214, 22));
    }

    #[tokio::test]
    async fn clean_eof_between_frames_returns_none() {
        let (a, mut b) = duplex(64 * 1024);
        drop(a);
        let got = read_frame(&mut b).await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn oversized_header_rejected() {
        let (mut a, mut b) = duplex(16);
        let bad_len = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = a.write_all(&bad_len).await;
            // Never send the body; reader must reject on header alone.
        });
        let err = read_frame(&mut b).await.unwrap_err();
        assert!(matches!(err, FrameError::Oversized { .. }));
    }
}
