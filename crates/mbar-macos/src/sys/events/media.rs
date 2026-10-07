//! `media_change` and `media.artwork` (`docs/spec/events.md` §5.10).
//!
//! Two backends:
//!
//! * **Direct** (macOS < 15.4): private MediaRemote, exactly like SketchyBar —
//!   `MRMediaRemoteRegisterForNowPlayingNotifications(main)` plus local
//!   `NSNotificationCenter` observers for the info/application/is-playing notifications,
//!   then `MRMediaRemoteGetNowPlayingApplicationDisplayName` → `MRMediaRemoteGetNowPlayingInfo`.
//! * **Helper** (macOS ≥ 15.4, where MediaRemote rejects non-Apple processes): a long-lived
//!   `/usr/bin/osascript -l JavaScript` helper. `osascript` is an Apple platform binary and
//!   may still use MediaRemote; the JXA script loads the framework through `NSBundle` and
//!   polls `MRNowPlayingRequest.localNowPlayingItem` / `localNowPlayingPlayerPath` /
//!   `localIsPlaying` once per second, printing one JSON line per change (artwork as base64,
//!   only when the track changes). The `/usr/bin/perl` + DynaLoader workaround used by
//!   `mediaremote-adapter` needs a separately compiled native dylib to load into perl, which
//!   this crate cannot ship, so it is not used.
//!
//! `MBAR_MEDIA_BACKEND=direct|helper` overrides the choice. Both feed the same state and
//! produce the exact SketchyBar INFO format.

use crate::sys::util::{self, private_fns, MEDIA_REMOTE};
use crate::sys::{Sink, SysEvent};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2_core_foundation::{CFData, CFDictionary, CFRetained, CFString};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSNotificationCenter, NSString};
use objc2_image_io::CGImageSource;
use std::ffi::c_void;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

private_fns! { MEDIA_REMOTE =>
    fn MRMediaRemoteRegisterForNowPlayingNotifications(*const c_void) -> ();
    fn MRMediaRemoteGetNowPlayingInfo(*const c_void, *const c_void) -> ();
    fn MRMediaRemoteGetNowPlayingApplicationIsPlaying(*const c_void, *const c_void) -> ();
    fn MRMediaRemoteGetNowPlayingApplicationDisplayName(i32, *const c_void, *const c_void) -> ();
}

/// The last complete now-playing snapshot (for the `media` provider).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NowPlaying {
    pub app: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
}

impl NowPlaying {
    /// `"playing"` / `"paused"`.
    pub fn state(&self) -> &'static str {
        if self.playing {
            "playing"
        } else {
            "paused"
        }
    }

    /// The `media_change` INFO.
    pub fn info(&self) -> String {
        media_info_json(self.playing, &self.title, &self.album, &self.artist, &self.app)
    }
}

/// SketchyBar's `escape_string`: only `"` → `\"` and LF → `\n`.
pub fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// `media_change` INFO, exact bytes (key order state, title, album, artist, app; no
/// trailing newline; `app` not escaped).
pub fn media_info_json(playing: bool, title: &str, album: &str, artist: &str, app: &str) -> String {
    format!(
        "{{\n\t\"state\": \"{}\",\n\t\"title\": \"{}\",\n\t\"album\": \"{}\",\n\t\"artist\": \"{}\",\n\t\"app\": \"{}\"\n}}",
        if playing { "playing" } else { "paused" },
        escape_string(title),
        escape_string(album),
        escape_string(artist),
        app
    )
}

/// Standard base64 decoder (padding optional, whitespace ignored); `None` on bad input.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        if c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// One line printed by the JXA helper.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HelperSample {
    pub app: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playing: bool,
    /// Base64 artwork, present only when the track changed.
    pub artwork: Option<String>,
}

/// Parses a helper line (`{"app":…,"title":…,"artist":…,"album":…,"playing":…,"artwork":…}`).
pub fn parse_helper_line(line: &str) -> Option<HelperSample> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
    Some(HelperSample {
        app: s("app"),
        title: s("title"),
        artist: s("artist"),
        album: s("album"),
        playing: v.get("playing").and_then(|x| x.as_bool()).unwrap_or(false),
        artwork: s("artwork"),
    })
}

const HELPER_SCRIPT: &str = r#"
ObjC.import('Foundation');
const bundle = $.NSBundle.bundleWithPath('/System/Library/PrivateFrameworks/MediaRemote.framework/');
bundle.load;
const MR = $.NSClassFromString('MRNowPlayingRequest');
const out = $.NSFileHandle.fileHandleWithStandardOutput;
function str(v) {
  if (v === undefined || v === null) return null;
  try { if (v.isNil && v.isNil()) return null; } catch (e) {}
  const u = ObjC.unwrap(v);
  return (u === undefined || u === null) ? null : String(u);
}
let last = '';
let lastTrack = '';
while (true) {
  const obj = { playing: false };
  try {
    const item = MR.localNowPlayingItem;
    const info = (item && !item.isNil()) ? item.nowPlayingInfo : null;
    const path = MR.localNowPlayingPlayerPath;
    const client = (path && !path.isNil()) ? path.client : null;
    obj.app = (client && !client.isNil()) ? str(client.displayName) : null;
    obj.playing = MR.localIsPlaying ? true : false;
    let art = null;
    if (info && !info.isNil()) {
      obj.title = str(info.valueForKey('kMRMediaRemoteNowPlayingInfoTitle'));
      obj.artist = str(info.valueForKey('kMRMediaRemoteNowPlayingInfoArtist'));
      obj.album = str(info.valueForKey('kMRMediaRemoteNowPlayingInfoAlbum'));
      art = info.valueForKey('kMRMediaRemoteNowPlayingInfoArtworkData');
    }
    const core = JSON.stringify(obj);
    const track = [obj.app, obj.title, obj.artist, obj.album, (art && !art.isNil()) ? art.length : 0].join('\u0001');
    if (core !== last || track !== lastTrack) {
      if (track !== lastTrack && art && !art.isNil()) {
        obj.artwork = ObjC.unwrap(art.base64EncodedStringWithOptions(0));
      }
      last = core;
      lastTrack = track;
      out.writeData($(JSON.stringify(obj) + '\n').dataUsingEncoding($.NSUTF8StringEncoding));
    }
  } catch (e) {}
  delay(1);
}
"#;

#[derive(Default)]
struct State {
    sink: Option<Sink>,
    started: bool,
    app: Option<String>,
    playing: bool,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    last_info: Option<String>,
    helper: Option<Child>,
    stopped: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

/// The last complete snapshot, if any.
pub fn now_playing() -> Option<NowPlaying> {
    with_state(|s| {
        Some(NowPlaying {
            app: s.app.clone()?,
            title: s.title.clone()?,
            artist: s.artist.clone()?,
            album: s.album.clone()?,
            playing: s.playing,
        })
    })
}

fn decode_image(bytes: &[u8]) -> Option<CFRetained<CGImage>> {
    let data = CFData::from_bytes(bytes);
    // SAFETY: `data` is a valid CFData; no options.
    unsafe {
        let src = CGImageSource::with_data(&data, None)?;
        src.image_at_index(0, None)
    }
}

/// `-update`: posts artwork (always, when present) and the INFO when it changed.
fn update(artwork: Option<CFRetained<CGImage>>) {
    let (sink, info) = with_state(|s| {
        let np = NowPlaying {
            app: s.app.clone().unwrap_or_default(),
            title: s.title.clone().unwrap_or_default(),
            artist: s.artist.clone().unwrap_or_default(),
            album: s.album.clone().unwrap_or_default(),
            playing: s.playing,
        };
        let complete = s.app.is_some() && s.title.is_some() && s.artist.is_some() && s.album.is_some();
        if !complete || !s.started {
            return (None, None);
        }
        let info = np.info();
        let changed = s.last_info.as_deref() != Some(info.as_str());
        if changed {
            s.last_info = Some(info.clone());
        }
        (s.sink.clone(), changed.then_some(info))
    });
    let Some(sink) = sink else { return };
    if let Some(img) = artwork {
        sink(SysEvent::MediaArtwork(Some(img)));
    }
    if let Some(info) = info {
        sink(SysEvent::MediaChange(info));
    }
}

// ----- direct backend ------------------------------------------------------------------

fn main_queue_ptr() -> *const c_void {
    DispatchQueue::main() as *const DispatchQueue as *const c_void
}

fn direct_media_change() {
    if !with_state(|s| s.started) {
        return;
    }
    let Some(display_name) = MRMediaRemoteGetNowPlayingApplicationDisplayName() else {
        return;
    };
    let block = RcBlock::new(|name: *const c_void| {
        // SAFETY: MediaRemote passes a CFStringRef or NULL, valid during the callback.
        let name = unsafe { util::borrowed(name as *const CFString) };
        let Some(name) = name else { return };
        with_state(|s| s.app = Some(name.to_string()));
        direct_fetch_info();
    });
    // SAFETY: main queue + heap block; MediaRemote copies the block.
    unsafe { display_name(0, main_queue_ptr(), &*block as *const _ as *const c_void) };
}

fn direct_fetch_info() {
    let Some(get_info) = MRMediaRemoteGetNowPlayingInfo() else {
        return;
    };
    let block = RcBlock::new(|dict: *const c_void| {
        // SAFETY: MediaRemote passes a CFDictionaryRef or NULL, valid during the callback.
        let Some(dict) = (unsafe { util::borrowed(dict as *const CFDictionary) }) else {
            return;
        };
        let title = util::dict_string(&dict, "kMRMediaRemoteNowPlayingInfoTitle");
        let artist = util::dict_string(&dict, "kMRMediaRemoteNowPlayingInfoArtist");
        let album = util::dict_string(&dict, "kMRMediaRemoteNowPlayingInfoAlbum");
        if title.is_none() || artist.is_none() || album.is_none() {
            return;
        }
        let artwork = if util::dict_get(&dict, "kMRMediaRemoteNowPlayingInfoArtworkMIMEType").is_some() {
            util::dict_get(&dict, "kMRMediaRemoteNowPlayingInfoArtworkData")
                .and_then(util::downcast::<CFData>)
                .and_then(|d| decode_image(&d.to_vec()))
        } else {
            None
        };
        with_state(|s| {
            s.title = title;
            s.artist = artist;
            s.album = album;
        });
        update(artwork);
    });
    // SAFETY: main queue + heap block; MediaRemote copies the block.
    unsafe { get_info(main_queue_ptr(), &*block as *const _ as *const c_void) };
}

fn direct_playing_change() {
    if !with_state(|s| s.started) {
        return;
    }
    let Some(is_playing) = MRMediaRemoteGetNowPlayingApplicationIsPlaying() else {
        return;
    };
    let block = RcBlock::new(|playing: u8| {
        with_state(|s| s.playing = playing != 0);
        direct_media_change();
    });
    // SAFETY: main queue + heap block; MediaRemote copies the block.
    unsafe { is_playing(main_queue_ptr(), &*block as *const _ as *const c_void) };
}

fn start_direct() {
    let Some(register) = MRMediaRemoteRegisterForNowPlayingNotifications() else {
        log::warn!("MediaRemote unavailable; media_change disabled");
        return;
    };
    // SAFETY: main queue pointer is valid forever.
    unsafe { register(main_queue_ptr()) };
    let center = NSNotificationCenter::defaultCenter();
    for (name, playing) in [
        ("kMRMediaRemoteNowPlayingInfoDidChangeNotification", false),
        ("kMRMediaRemoteNowPlayingApplicationDidChangeNotification", false),
        ("kMRMediaRemoteNowPlayingApplicationIsPlayingDidChangeNotification", true),
    ] {
        let obs = super::Observer::new(&center, Some(&NSString::from_str(name)), move |_| {
            if playing {
                direct_playing_change();
            } else {
                direct_media_change();
            }
        });
        // Media observers live for the rest of the process (never unsubscribed).
        std::mem::forget(obs);
    }
    direct_playing_change();
}

// ----- helper backend ------------------------------------------------------------------

fn apply_helper_sample(sample: HelperSample) {
    with_state(|s| {
        if sample.app.is_some() {
            s.app = sample.app.clone();
        }
        s.playing = sample.playing;
        if sample.title.is_some() && sample.artist.is_some() && sample.album.is_some() {
            s.title = sample.title.clone();
            s.artist = sample.artist.clone();
            s.album = sample.album.clone();
        }
    });
    let artwork = sample
        .artwork
        .as_deref()
        .and_then(base64_decode)
        .and_then(|b| decode_image(&b));
    update(artwork);
}

fn spawn_helper() -> Option<Child> {
    let mut child = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", HELPER_SCRIPT])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| log::warn!("media helper spawn failed: {e}"))
        .ok()?;
    let stdout = child.stdout.take()?;
    std::thread::Builder::new()
        .name("mbar-media".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(sample) = parse_helper_line(&line) {
                    apply_helper_sample(sample);
                }
            }
            helper_exited();
        })
        .ok()?;
    Some(child)
}

fn helper_exited() {
    static RESTARTS: Mutex<Vec<Instant>> = Mutex::new(Vec::new());
    let allow = {
        let mut r = RESTARTS.lock().unwrap_or_else(|e| e.into_inner());
        r.retain(|t| t.elapsed() < Duration::from_secs(60));
        r.push(Instant::now());
        r.len() <= 5
    };
    let (old, stopped) = with_state(|s| (s.helper.take(), s.stopped));
    if let Some(mut c) = old {
        let _ = c.kill();
        let _ = c.wait();
    }
    if stopped {
        return;
    }
    if !allow {
        log::warn!("media helper keeps exiting; giving up");
        return;
    }
    std::thread::sleep(Duration::from_secs(2));
    let child = spawn_helper();
    with_state(|s| s.helper = child);
}

fn use_helper() -> bool {
    match std::env::var("MBAR_MEDIA_BACKEND").as_deref() {
        Ok("direct") => false,
        Ok("helper") => true,
        _ => util::os_at_least(15, 4),
    }
}

/// `begin_receiving_media_events` (idempotent). Call on the main thread.
pub fn start(sink: Sink) {
    let first = with_state(|s| {
        let first = !s.started;
        s.started = true;
        s.sink = Some(sink);
        first
    });
    if !first {
        return;
    }
    if use_helper() {
        let child = spawn_helper();
        with_state(|s| s.helper = child);
    } else {
        start_direct();
    }
}

/// `forced_media_change_event` (`--update`, `--trigger media_change`): clears the de-dup
/// state and re-queries (async; the event arrives later).
pub fn refresh() {
    if !with_state(|s| {
        s.last_info = None;
        s.started
    }) {
        return;
    }
    if with_state(|s| s.helper.is_some()) {
        update(None);
    } else {
        direct_playing_change();
    }
}

/// Kills the helper process (call on exit).
pub fn stop() {
    if let Some(mut c) = with_state(|s| {
        s.stopped = true;
        s.helper.take()
    }) {
        let _ = c.kill();
        let _ = c.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_format() {
        assert_eq!(
            media_info_json(true, "T \"x\"", "Al\nb", "Ar\\", "Music"),
            "{\n\t\"state\": \"playing\",\n\t\"title\": \"T \\\"x\\\"\",\n\t\"album\": \"Al\\nb\",\n\t\"artist\": \"Ar\\\",\n\t\"app\": \"Music\"\n}"
        );
        assert!(media_info_json(false, "", "", "", "").contains("\"paused\""));
    }

    #[test]
    fn base64() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVs\nbG8h").unwrap(), b"hello!");
        assert!(base64_decode("@@").is_none());
    }

    #[test]
    fn helper_line() {
        let s = parse_helper_line(r#"{"playing":true,"app":"Music","title":"A","artist":"B","album":"C"}"#)
            .unwrap();
        assert!(s.playing);
        assert_eq!(s.app.as_deref(), Some("Music"));
        assert_eq!(s.artwork, None);
        assert!(parse_helper_line("garbage").is_none());
    }
}
