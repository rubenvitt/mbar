//! AeroSpace's socket wire format (AeroSpace `docs/guide.adoc`, "Socket protocol"):
//!
//! * handshake: the client writes its `SOCKET_PROTOCOL_VERSION` as `u32` (host order,
//!   little-endian on every Mac), the server answers with its own and closes the
//!   connection when it does not support the client's;
//! * then every message is a frame: `u32 LE` payload length + UTF-8 JSON payload;
//! * `ClientRequest {"args":[…],"stdin":"","windowId":null,"workspace":null}` is answered
//!   with one `ServerAnswer {"exitCode","stdout","stderr","serverVersionAndHash"}`,
//!   except `subscribe`, which turns the connection into a stream of `ServerEvent` frames.

use std::io::{self, Read, Write};

use serde_json::Value;

use crate::{Answer, PROTOCOL_VERSION};

/// Upper bound for one frame (and one CLI output line). AeroSpace's answers are far
/// smaller; the cap only protects mbar from allocating gigabytes for a garbage length.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// Writes one frame (length prefix and payload in a single write, like AeroSpace's
/// `writeAtomic`).
pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame too large",
        ));
    }
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)?;
    w.flush()
}

/// Reads one frame. A read timeout is an error (`WouldBlock`/`TimedOut`); a length above
/// [`MAX_FRAME_LEN`] is `InvalidData`; EOF is `UnexpectedEof`.
pub fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    read_frame_with(r, &|| false)
}

/// [`read_frame`] that keeps waiting through read timeouts as long as `keep_waiting`
/// returns true (the subscription polls its stop flag this way). Partial reads are kept
/// across timeouts, so the framing never gets out of sync.
pub(crate) fn read_frame_with(
    r: &mut impl Read,
    keep_waiting: &dyn Fn() -> bool,
) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    read_full(r, &mut len, keep_waiting)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the {MAX_FRAME_LEN} byte limit"),
        ));
    }
    let mut buf = vec![0u8; len];
    read_full(r, &mut buf, keep_waiting)?;
    Ok(buf)
}

fn read_full(r: &mut impl Read, buf: &mut [u8], keep_waiting: &dyn Fn() -> bool) -> io::Result<()> {
    let mut off = 0;
    while off < buf.len() {
        match r.read(&mut buf[off..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => off += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if is_timeout(&e) && keep_waiting() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Whether `e` is a socket read/write timeout (`SO_RCVTIMEO` reports `EAGAIN`).
pub(crate) fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// The client side of the handshake. Every failure (write error, EOF, read timeout, a
/// different server version) means "this server does not speak protocol
/// [`PROTOCOL_VERSION`]"; the message says which one it was.
pub fn handshake<S: Read + Write>(stream: &mut S) -> Result<(), String> {
    stream
        .write_all(&PROTOCOL_VERSION.to_le_bytes())
        .and_then(|()| stream.flush())
        .map_err(|e| format!("sending the socket protocol handshake failed: {e}"))?;
    let mut version = [0u8; 4];
    read_full(stream, &mut version, &|| false).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            "AeroSpace closed the connection during the socket protocol handshake".to_string()
        } else if is_timeout(&e) {
            "AeroSpace did not answer the socket protocol handshake".to_string()
        } else {
            format!("reading the socket protocol handshake failed: {e}")
        }
    })?;
    let server = u32::from_le_bytes(version);
    if server != PROTOCOL_VERSION {
        return Err(format!(
            "AeroSpace speaks socket protocol version {server}, mbar speaks {PROTOCOL_VERSION}"
        ));
    }
    Ok(())
}

/// The JSON of a `ClientRequest`. `windowId` and `workspace` are explicit `null`s: mbar
/// is not an AeroSpace callback, so there is no `AEROSPACE_WINDOW_ID` /
/// `AEROSPACE_WORKSPACE` to forward (the server warns when the fields are missing).
pub fn client_request(args: &[String], stdin: &str) -> String {
    serde_json::json!({
        "args": args,
        "stdin": stdin,
        "windowId": null,
        "workspace": null,
    })
    .to_string()
}

/// Parses a `ServerAnswer` frame. `exitCode` is required; missing `stdout`/`stderr`
/// are empty, a missing `serverVersionAndHash` is `None`.
pub fn parse_server_answer(payload: &[u8]) -> Result<Answer, String> {
    let v: Value = serde_json::from_slice(payload)
        .map_err(|e| format!("AeroSpace sent an invalid answer ({e})"))?;
    let exit_code = v
        .get("exitCode")
        .and_then(Value::as_i64)
        .and_then(|c| i32::try_from(c).ok())
        .ok_or_else(|| "AeroSpace sent an answer without exitCode".to_string())?;
    let text = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(Answer {
        exit_code,
        stdout: text("stdout"),
        stderr: text("stderr"),
        server_version: v
            .get("serverVersionAndHash")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// Whether a frame of the event stream is a `ServerAnswer` instead of an event: the
/// server answers `subscribe` like a normal command when it rejects the arguments (or,
/// on servers without `subscribe`, does not know the command).
pub(crate) fn is_server_answer(payload: &[u8]) -> bool {
    serde_json::from_slice::<Value>(payload)
        .map(|v| v.get("exitCode").is_some() && v.get("_event").is_none())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"{\"a\":1}").unwrap();
        assert_eq!(&buf[..4], &7u32.to_le_bytes());
        assert_eq!(read_frame(&mut buf.as_slice()).unwrap(), b"{\"a\":1}");
        let mut empty = Vec::new();
        write_frame(&mut empty, b"").unwrap();
        assert_eq!(read_frame(&mut empty.as_slice()).unwrap(), b"");
    }

    #[test]
    fn oversized_and_truncated_frames_are_errors() {
        let huge = (MAX_FRAME_LEN as u32 + 1).to_le_bytes();
        let e = read_frame(&mut huge.as_slice()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        let mut truncated = 10u32.to_le_bytes().to_vec();
        truncated.extend_from_slice(b"abc");
        let e = read_frame(&mut truncated.as_slice()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn request_json_matches_the_documented_shape() {
        assert_eq!(
            client_request(&["workspace".into(), "1".into()], ""),
            r#"{"args":["workspace","1"],"stdin":"","windowId":null,"workspace":null}"#
        );
        assert_eq!(
            client_request(&[], ""),
            r#"{"args":[],"stdin":"","windowId":null,"workspace":null}"#
        );
    }

    #[test]
    fn parses_answers() {
        let a = parse_server_answer(
            br#"{"exitCode":0,"stdout":"1\n2","stderr":"","serverVersionAndHash":"0.20.0-Beta 33fa0643"}"#,
        )
        .unwrap();
        assert_eq!(
            a,
            Answer {
                exit_code: 0,
                stdout: "1\n2".into(),
                stderr: String::new(),
                server_version: Some("0.20.0-Beta 33fa0643".into()),
            }
        );
        assert_eq!(
            parse_server_answer(br#"{"exitCode":2}"#).unwrap().exit_code,
            2
        );
        assert!(parse_server_answer(br#"{"stdout":"x"}"#).is_err());
        assert!(parse_server_answer(b"nope").is_err());
        assert!(is_server_answer(br#"{"exitCode":2,"stderr":"x"}"#));
        assert!(!is_server_answer(br#"{"_event":"mode-changed"}"#));
        assert!(!is_server_answer(b"garbage"));
    }

    #[test]
    fn handshake_outcomes() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || {
            let mut v = [0u8; 4];
            server.read_exact(&mut v).unwrap();
            assert_eq!(u32::from_le_bytes(v), 1);
            server.write_all(&1u32.to_le_bytes()).unwrap();
        });
        handshake(&mut client).unwrap();
        t.join().unwrap();

        let (mut client, mut server) = UnixStream::pair().unwrap();
        server.write_all(&2u32.to_le_bytes()).unwrap();
        assert!(handshake(&mut client).unwrap_err().contains("version 2"));

        let (mut client, server) = UnixStream::pair().unwrap();
        drop(server);
        assert!(handshake(&mut client).is_err());

        let (mut client, _server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        assert!(handshake(&mut client)
            .unwrap_err()
            .contains("did not answer"));
    }

    #[test]
    fn read_frame_with_survives_timeouts() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let t = std::thread::spawn(move || {
            server.write_all(&5u32.to_le_bytes()[..2]).unwrap();
            std::thread::sleep(Duration::from_millis(40));
            server.write_all(&5u32.to_le_bytes()[2..]).unwrap();
            server.write_all(b"he").unwrap();
            std::thread::sleep(Duration::from_millis(40));
            server.write_all(b"llo").unwrap();
        });
        assert_eq!(read_frame_with(&mut client, &|| true).unwrap(), b"hello");
        t.join().unwrap();
    }
}
