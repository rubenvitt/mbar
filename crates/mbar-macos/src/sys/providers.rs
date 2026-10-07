//! Native data providers (`docs/EXTENSIONS.md`, `provider=`): in-process samplers that
//! replace common polling scripts.
//!
//! [`Providers`] owns one background thread that samples only the subscribed polling
//! providers (`cpu`, `memory`, `network`, `disk`, plus a slow fallback poll for `battery`)
//! at their requested frequency and posts [`SysEvent::ProviderSample`]. Event-driven
//! providers (`volume`, `wifi`, `front_app`, `media`, `battery`) are sampled once on
//! subscription and then from the matching system events: the integration layer must pass
//! every [`SysEvent`] through [`Providers::on_event`] (cheap when nothing is subscribed).
//! `clock` is formatted by the core and rejected here.
//!
//! Value keys are those of the EXTENSIONS table; numbers are pre-formatted strings.

use super::events::{self, media, power, volume, wifi};
use super::{Sink, SysEvent};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Provider kinds handled natively by the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Cpu,
    Memory,
    Battery,
    Network,
    Disk,
    Volume,
    Wifi,
    FrontApp,
    Media,
}

impl Kind {
    pub fn parse(name: &str) -> Option<Kind> {
        Some(match name {
            "cpu" => Kind::Cpu,
            "memory" => Kind::Memory,
            "battery" => Kind::Battery,
            "network" => Kind::Network,
            "disk" => Kind::Disk,
            "volume" => Kind::Volume,
            "wifi" => Kind::Wifi,
            "front_app" => Kind::FrontApp,
            "media" => Kind::Media,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Cpu => "cpu",
            Kind::Memory => "memory",
            Kind::Battery => "battery",
            Kind::Network => "network",
            Kind::Disk => "disk",
            Kind::Volume => "volume",
            Kind::Wifi => "wifi",
            Kind::FrontApp => "front_app",
            Kind::Media => "media",
        }
    }

    /// Polling interval when `provider.freq` is not given; `None` = purely event driven.
    pub fn default_freq(self) -> Option<Duration> {
        match self {
            Kind::Cpu | Kind::Network => Some(Duration::from_secs(2)),
            Kind::Memory => Some(Duration::from_secs(5)),
            Kind::Disk => Some(Duration::from_secs(60)),
            // Fallback poll; IOPS notifications drive it normally.
            Kind::Battery => Some(Duration::from_secs(120)),
            Kind::Volume | Kind::Wifi | Kind::FrontApp | Kind::Media => None,
        }
    }

    /// Event-driven providers ignore `provider.freq`.
    pub fn event_driven(self) -> bool {
        matches!(
            self,
            Kind::Volume | Kind::Wifi | Kind::FrontApp | Kind::Media | Kind::Battery
        )
    }
}

// ---------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------

/// Aggregate CPU ticks (`HOST_CPU_LOAD_INFO`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuTicks {
    pub user: u64,
    pub system: u64,
    pub idle: u64,
    pub nice: u64,
}

/// `(percent, user, sys)` between two tick snapshots (0..100). Counter wrap-around of the
/// 32-bit kernel counters is handled.
pub fn cpu_usage(prev: &CpuTicks, cur: &CpuTicks) -> (f64, f64, f64) {
    let d = |a: u64, b: u64| (b as u32).wrapping_sub(a as u32) as f64;
    let user = d(prev.user, cur.user) + d(prev.nice, cur.nice);
    let sys = d(prev.system, cur.system);
    let idle = d(prev.idle, cur.idle);
    let total = user + sys + idle;
    if total <= 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let u = user / total * 100.0;
    let s = sys / total * 100.0;
    (u + s, u, s)
}

/// Bytes/s from two 32-bit interface counters (wrap-around safe).
pub fn counter_delta(prev: u32, cur: u32) -> u64 {
    cur.wrapping_sub(prev) as u64
}

/// Human-readable rate (`512 B/s`, `12 KB/s`, `1.5 MB/s`, `1.1 GB/s`; 1024 based).
pub fn human_rate(bytes_per_sec: f64) -> String {
    const K: f64 = 1024.0;
    let b = bytes_per_sec.max(0.0);
    if b < K {
        format!("{:.0} B/s", b)
    } else if b < K * K {
        format!("{:.0} KB/s", b / K)
    } else if b < K * K * K {
        format!("{:.1} MB/s", b / (K * K))
    } else {
        format!("{:.1} GB/s", b / (K * K * K))
    }
}

/// `%.1f` gigabytes (1024³).
pub fn format_gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// Rounded percentage `part/total`.
pub fn percent_of(part: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    ((part as f64 / total as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u32
}

/// Activity-Monitor style "memory used" pages: app memory (internal − purgeable) + wired +
/// compressed.
pub fn memory_used_pages(internal: u64, purgeable: u64, wired: u64, compressed: u64) -> u64 {
    internal.saturating_sub(purgeable) + wired + compressed
}

// ---------------------------------------------------------------------------------------
// Readers
// ---------------------------------------------------------------------------------------

fn host() -> libc::mach_port_t {
    static HOST: std::sync::OnceLock<libc::mach_port_t> = std::sync::OnceLock::new();
    // SAFETY: returns a send right to the host port (cached: each call leaks a ref).
    // mach2 0.4 has no binding for it, hence the deprecated libc one.
    #[allow(deprecated)]
    let get = || unsafe { libc::mach_host_self() };
    *HOST.get_or_init(get)
}

/// Aggregate CPU ticks of all processors.
pub fn read_cpu_ticks() -> Option<CpuTicks> {
    let mut info = libc::host_cpu_load_info { cpu_ticks: [0; 4] };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: `info` has room for HOST_CPU_LOAD_INFO_COUNT integers.
    let kr = unsafe {
        libc::host_statistics(
            host(),
            libc::HOST_CPU_LOAD_INFO,
            &mut info as *mut _ as libc::host_info_t,
            &mut count,
        )
    };
    if kr != 0 {
        return None;
    }
    let t = info.cpu_ticks;
    Some(CpuTicks {
        user: t[libc::CPU_STATE_USER as usize] as u64,
        system: t[libc::CPU_STATE_SYSTEM as usize] as u64,
        idle: t[libc::CPU_STATE_IDLE as usize] as u64,
        nice: t[libc::CPU_STATE_NICE as usize] as u64,
    })
}

/// The classic (rev 1) prefix of `vm_statistics64` (`mach/vm_statistics.h`).
#[repr(C, align(8))]
#[derive(Default)]
struct VmStats64 {
    free_count: u32,
    active_count: u32,
    inactive_count: u32,
    wire_count: u32,
    zero_fill_count: u64,
    reactivations: u64,
    pageins: u64,
    pageouts: u64,
    faults: u64,
    cow_faults: u64,
    lookups: u64,
    hits: u64,
    purges: u64,
    purgeable_count: u32,
    speculative_count: u32,
    decompressions: u64,
    compressions: u64,
    swapins: u64,
    swapouts: u64,
    compressor_page_count: u32,
    throttled_count: u32,
    external_page_count: u32,
    internal_page_count: u32,
    total_uncompressed_pages_in_compressor: u64,
}

fn sysctl_u64(name: &str) -> Option<u64> {
    let c = CString::new(name).ok()?;
    let mut v: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: valid name, out buffer and length.
    let r = unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            &mut v as *mut u64 as *mut _,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0).then_some(v)
}

/// `(used_bytes, total_bytes)`.
pub fn read_memory() -> Option<(u64, u64)> {
    let total = sysctl_u64("hw.memsize")?;
    let mut vm = VmStats64::default();
    let mut count = (std::mem::size_of::<VmStats64>() / std::mem::size_of::<i32>())
        as libc::mach_msg_type_number_t;
    // SAFETY: `vm` has room for `count` integers; the kernel fills at most that many.
    let kr = unsafe {
        libc::host_statistics64(
            host(),
            libc::HOST_VM_INFO64,
            &mut vm as *mut _ as libc::host_info64_t,
            &mut count,
        )
    };
    if kr != 0 {
        return None;
    }
    // SAFETY: sysconf has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    let used = memory_used_pages(
        vm.internal_page_count as u64,
        vm.purgeable_count as u64,
        vm.wire_count as u64,
        vm.compressor_page_count as u64,
    ) * page;
    let _ = (
        vm.free_count,
        vm.active_count,
        vm.inactive_count,
        vm.speculative_count,
        vm.external_page_count,
    );
    Some((used.min(total), total))
}

/// Prefix of `struct if_data` (`net/if_var.h`, 32-bit counters) as returned in
/// `ifa_data` for `AF_LINK` entries.
#[repr(C)]
struct IfDataPrefix {
    ifi_type: u8,
    ifi_typelen: u8,
    ifi_physical: u8,
    ifi_addrlen: u8,
    ifi_hdrlen: u8,
    ifi_recvquota: u8,
    ifi_xmitquota: u8,
    ifi_unused1: u8,
    ifi_mtu: u32,
    ifi_metric: u32,
    ifi_baudrate: u32,
    ifi_ipackets: u32,
    ifi_ierrors: u32,
    ifi_opackets: u32,
    ifi_oerrors: u32,
    ifi_collisions: u32,
    ifi_ibytes: u32,
    ifi_obytes: u32,
}

/// Per-interface `(in_bytes, out_bytes)` 32-bit counters (`getifaddrs`, `AF_LINK`).
pub fn read_net_counters() -> HashMap<String, (u32, u32)> {
    let mut out = HashMap::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: valid out pointer; freed with freeifaddrs below.
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return out;
    }
    let mut cur = ifap;
    while !cur.is_null() {
        // SAFETY: `cur` walks the list returned by getifaddrs.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_data.is_null() || ifa.ifa_name.is_null() {
            continue;
        }
        // SAFETY: non-null sockaddr from getifaddrs.
        if unsafe { (*ifa.ifa_addr).sa_family } as i32 != libc::AF_LINK {
            continue;
        }
        // SAFETY: for AF_LINK entries `ifa_data` points to a `struct if_data`.
        let data = unsafe { &*(ifa.ifa_data as *const IfDataPrefix) };
        // SAFETY: NUL-terminated interface name.
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let _ = (data.ifi_type, data.ifi_mtu, data.ifi_ipackets);
        out.insert(name, (data.ifi_ibytes, data.ifi_obytes));
    }
    // SAFETY: list from getifaddrs.
    unsafe { libc::freeifaddrs(ifap) };
    out
}

/// Summed byte deltas between two counter snapshots for `iface` (all non-loopback
/// interfaces when `None`).
pub fn net_delta(
    prev: &HashMap<String, (u32, u32)>,
    cur: &HashMap<String, (u32, u32)>,
    iface: Option<&str>,
) -> (u64, u64) {
    let mut down = 0;
    let mut up = 0;
    for (name, &(i, o)) in cur {
        let selected = match iface {
            Some(f) => name == f,
            None => !name.starts_with("lo"),
        };
        if !selected {
            continue;
        }
        if let Some(&(pi, po)) = prev.get(name) {
            down += counter_delta(pi, i);
            up += counter_delta(po, o);
        }
    }
    (down, up)
}

/// `(free_bytes, total_bytes)` of the file system at `path` (available-to-user blocks).
pub fn read_disk(path: &str) -> Option<(u64, u64)> {
    let c = CString::new(path).ok()?;
    // SAFETY: zeroed statfs is a valid out buffer.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid path and out buffer.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let bs = st.f_bsize as u64;
    Some((st.f_bavail * bs, st.f_blocks * bs))
}

// ---------------------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------------------

/// Interface counters with the time they were read.
type NetSnapshot = (HashMap<String, (u32, u32)>, Instant);

enum Memo {
    None,
    Cpu(Option<CpuTicks>),
    Net(Option<NetSnapshot>),
}

struct Sub {
    kind: Kind,
    freq: Option<Duration>,
    args: Option<String>,
    next: Instant,
    memo: Memo,
}

#[derive(Default)]
struct Sched {
    subs: HashMap<u64, Sub>,
    shutdown: bool,
}

type Shared = Arc<(Mutex<Sched>, Condvar)>;

/// Scheduler woken by IOPS change notifications (battery provider).
static BATTERY_TARGET: Mutex<Option<WeakShared>> = Mutex::new(None);
type WeakShared = std::sync::Weak<(Mutex<Sched>, Condvar)>;

/// The provider scheduler. Dropping it stops the sampling thread.
pub struct Providers {
    shared: Shared,
    sink: Sink,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn values(pairs: &[(&str, String)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn battery_values() -> Vec<(String, String)> {
    match power::battery() {
        Some(b) => values(&[
            ("percent", b.percent.to_string()),
            ("charging", b.charging.to_string()),
            ("remaining", b.remaining_text()),
        ]),
        None => Vec::new(),
    }
}

fn volume_values(v: Option<f32>) -> Vec<(String, String)> {
    let (read, muted) = volume::read();
    let v = v.unwrap_or(read);
    values(&[
        ("percent", volume::percent(v).to_string()),
        ("muted", muted.to_string()),
    ])
}

fn wifi_values(ssid: Option<String>) -> Vec<(String, String)> {
    let ssid = ssid.unwrap_or_else(wifi::current_ssid);
    let rssi = wifi::current_rssi()
        .map(|r| r.to_string())
        .unwrap_or_default();
    values(&[("ssid", ssid), ("rssi", rssi)])
}

fn front_app_values(name: Option<String>, bundle: Option<String>) -> Vec<(String, String)> {
    values(&[
        ("name", name.unwrap_or_default()),
        ("bundle_id", bundle.unwrap_or_default()),
    ])
}

fn media_values() -> Vec<(String, String)> {
    match media::now_playing() {
        Some(n) => values(&[
            ("title", n.title.clone()),
            ("artist", n.artist.clone()),
            ("album", n.album.clone()),
            ("app", n.app.clone()),
            ("state", n.state().to_string()),
        ]),
        None => values(&[
            ("title", String::new()),
            ("artist", String::new()),
            ("album", String::new()),
            ("app", String::new()),
            ("state", "paused".into()),
        ]),
    }
}

/// Samples one subscription; `None` when the sample needs a second reading (deltas).
fn sample(sub: &mut Sub) -> Option<Vec<(String, String)>> {
    match sub.kind {
        Kind::Cpu => {
            let cur = read_cpu_ticks()?;
            let prev = match &mut sub.memo {
                Memo::Cpu(p) => p.replace(cur),
                _ => {
                    sub.memo = Memo::Cpu(Some(cur));
                    None
                }
            }?;
            let (p, u, s) = cpu_usage(&prev, &cur);
            Some(values(&[
                ("percent", format!("{:.0}", p)),
                ("user", format!("{:.0}", u)),
                ("sys", format!("{:.0}", s)),
            ]))
        }
        Kind::Memory => {
            let (used, total) = read_memory()?;
            Some(values(&[
                ("percent", percent_of(used, total).to_string()),
                ("used_gb", format_gb(used)),
                ("total_gb", format_gb(total)),
            ]))
        }
        Kind::Network => {
            let now = Instant::now();
            let cur = read_net_counters();
            let prev = match &mut sub.memo {
                Memo::Net(p) => p.replace((cur.clone(), now)),
                _ => {
                    sub.memo = Memo::Net(Some((cur.clone(), now)));
                    None
                }
            }?;
            let dt = now.duration_since(prev.1).as_secs_f64().max(0.001);
            let (down, up) =
                net_delta(&prev.0, &cur, sub.args.as_deref().filter(|a| !a.is_empty()));
            let (down, up) = (down as f64 / dt, up as f64 / dt);
            Some(values(&[
                ("down", human_rate(down)),
                ("up", human_rate(up)),
                ("down_bytes", format!("{:.0}", down)),
                ("up_bytes", format!("{:.0}", up)),
            ]))
        }
        Kind::Disk => {
            let path = sub
                .args
                .clone()
                .filter(|a| !a.is_empty())
                .unwrap_or_else(|| "/".into());
            let (free, total) = read_disk(&path)?;
            Some(values(&[
                (
                    "percent",
                    percent_of(total - free.min(total), total).to_string(),
                ),
                ("free_gb", format_gb(free)),
                ("total_gb", format_gb(total)),
            ]))
        }
        Kind::Battery => Some(battery_values()),
        Kind::Volume => Some(volume_values(None)),
        Kind::Wifi => Some(wifi_values(None)),
        Kind::FrontApp => {
            let (n, b, _) = events::front_app().unwrap_or((None, None, 0));
            Some(front_app_values(n, b))
        }
        Kind::Media => Some(media_values()),
    }
}

fn run(shared: Shared, sink: Sink) {
    let (lock, cv) = &*shared;
    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if guard.shutdown {
            return;
        }
        let now = Instant::now();
        let mut posts = Vec::new();
        for (id, sub) in guard.subs.iter_mut() {
            if sub.next > now {
                continue;
            }
            let first = matches!(sub.memo, Memo::None);
            let result = sample(sub);
            let freq = sub.freq.or(sub.kind.default_freq());
            let delta_kind = matches!(sub.kind, Kind::Cpu | Kind::Network);
            sub.next = match (result.is_none() && first && delta_kind, freq) {
                // Delta providers: take the second reading soon after the first.
                (true, _) => now + Duration::from_millis(500),
                (false, Some(f)) => now + f,
                (false, None) => now + Duration::from_secs(365 * 24 * 3600),
            };
            if let Some(v) = result {
                posts.push((*id, sub.kind, v));
            }
        }
        let next = guard.subs.values().map(|s| s.next).min();
        drop(guard);
        for (id, kind, v) in posts {
            sink(SysEvent::ProviderSample {
                id,
                provider: kind.name().to_string(),
                values: v,
            });
        }
        guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if guard.shutdown {
            return;
        }
        let wait = next.map(|t| t.saturating_duration_since(Instant::now()));
        guard = match wait {
            Some(d) if d.is_zero() => guard,
            Some(d) => cv
                .wait_timeout(guard, d)
                .map(|r| r.0)
                .unwrap_or_else(|e| e.into_inner().0),
            None => cv.wait(guard).unwrap_or_else(|e| e.into_inner()),
        };
    }
}

impl Providers {
    /// Starts the (idle) sampling thread.
    pub fn new(sink: Sink) -> Providers {
        let shared: Shared = Arc::new((Mutex::new(Sched::default()), Condvar::new()));
        let (s2, k2) = (shared.clone(), sink.clone());
        let thread = std::thread::Builder::new()
            .name("mbar-providers".into())
            .spawn(move || run(s2, k2))
            .ok();
        Providers {
            shared,
            sink,
            thread,
        }
    }

    /// (Re)configures provider `provider` for subscription `id` (`provider=`,
    /// `provider.freq=`, `provider.args=`). Samples immediately. Errors for unknown names and
    /// `clock` (formatted by the core).
    pub fn subscribe(
        &self,
        id: u64,
        provider: &str,
        freq: Option<f32>,
        args: Option<String>,
    ) -> Result<(), String> {
        let kind = match provider {
            "clock" => return Err("provider 'clock' is handled by the core".into()),
            p => Kind::parse(p).ok_or_else(|| format!("unknown provider '{p}'"))?,
        };
        match kind {
            Kind::Volume => volume::start(self.sink.clone()),
            Kind::Media => media::start(self.sink.clone()),
            Kind::Battery => {
                *BATTERY_TARGET.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(Arc::downgrade(&self.shared));
                static HOOKED: std::sync::Once = std::sync::Once::new();
                HOOKED.call_once(|| {
                    power::add_change_listener(Arc::new(|| {
                        let target = BATTERY_TARGET
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clone();
                        let Some(shared) = target.and_then(|w| w.upgrade()) else {
                            return;
                        };
                        let (lock, cv) = &*shared;
                        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
                        let now = Instant::now();
                        for sub in s.subs.values_mut().filter(|s| s.kind == Kind::Battery) {
                            sub.next = now;
                        }
                        cv.notify_all();
                    }));
                });
            }
            _ => {}
        }
        let freq = if kind.event_driven() && kind != Kind::Battery {
            None
        } else {
            freq.filter(|f| *f > 0.0)
                .map(|f| Duration::from_secs_f32(f.max(0.1)))
        };
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        s.subs.insert(
            id,
            Sub {
                kind,
                freq,
                args,
                next: Instant::now(),
                memo: Memo::None,
            },
        );
        cv.notify_all();
        Ok(())
    }

    /// Removes subscription `id`.
    pub fn unsubscribe(&self, id: u64) {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(|e| e.into_inner())
            .subs
            .remove(&id);
    }

    /// Samples `id` as soon as possible.
    pub fn sample_now(&self, id: u64) {
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sub) = s.subs.get_mut(&id) {
            sub.next = Instant::now();
        }
        cv.notify_all();
    }

    /// Kinds currently subscribed.
    pub fn active_kinds(&self) -> Vec<Kind> {
        let (lock, _) = &*self.shared;
        let s = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut k: Vec<Kind> = Vec::new();
        for sub in s.subs.values() {
            if !k.contains(&sub.kind) {
                k.push(sub.kind);
            }
        }
        k
    }

    /// Feeds a system event to the event-driven providers (call for every event before
    /// handing it to the core). Posts samples synchronously through the sink.
    pub fn on_event(&self, ev: &SysEvent) {
        let kind = match ev {
            SysEvent::VolumeChange(_) => Kind::Volume,
            SysEvent::WifiChange(_) => Kind::Wifi,
            SysEvent::FrontAppSwitched { .. } => Kind::FrontApp,
            SysEvent::MediaChange(_) => Kind::Media,
            SysEvent::PowerSourceChange(_) => Kind::Battery,
            _ => return,
        };
        let ids: Vec<u64> = {
            let (lock, _) = &*self.shared;
            let s = lock.lock().unwrap_or_else(|e| e.into_inner());
            s.subs
                .iter()
                .filter(|(_, s)| s.kind == kind)
                .map(|(id, _)| *id)
                .collect()
        };
        if ids.is_empty() {
            return;
        }
        let v = match ev {
            SysEvent::VolumeChange(v) => volume_values(Some(*v)),
            SysEvent::WifiChange(ssid) => wifi_values(Some(ssid.clone())),
            SysEvent::FrontAppSwitched {
                name, bundle_id, ..
            } => front_app_values(name.clone(), bundle_id.clone()),
            SysEvent::MediaChange(_) => media_values(),
            _ => battery_values(),
        };
        for id in ids {
            (self.sink)(SysEvent::ProviderSample {
                id,
                provider: kind.name().to_string(),
                values: v.clone(),
            });
        }
    }
}

impl Drop for Providers {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.shared;
            lock.lock().unwrap_or_else(|e| e.into_inner()).shutdown = true;
            cv.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(Kind::parse("cpu"), Some(Kind::Cpu));
        assert_eq!(Kind::parse("clock"), None);
        assert!(Kind::Volume.event_driven());
        assert!(!Kind::Cpu.event_driven());
        assert_eq!(Kind::FrontApp.name(), "front_app");
    }

    #[test]
    fn cpu_math() {
        let a = CpuTicks {
            user: 100,
            system: 50,
            idle: 850,
            nice: 0,
        };
        let b = CpuTicks {
            user: 130,
            system: 60,
            idle: 910,
            nice: 0,
        };
        let (p, u, s) = cpu_usage(&a, &b);
        assert!((p - 40.0).abs() < 1e-9);
        assert!((u - 30.0).abs() < 1e-9);
        assert!((s - 10.0).abs() < 1e-9);
        assert_eq!(cpu_usage(&a, &a), (0.0, 0.0, 0.0));
        // wrap-around of 32-bit counters
        let w1 = CpuTicks {
            user: u32::MAX as u64 - 9,
            system: 0,
            idle: 0,
            nice: 0,
        };
        let w2 = CpuTicks {
            user: 10,
            system: 0,
            idle: 20,
            nice: 0,
        };
        let (p, _, _) = cpu_usage(&w1, &w2);
        assert!((p - 50.0).abs() < 1e-9);
    }

    #[test]
    fn net_math() {
        assert_eq!(counter_delta(10, 30), 20);
        assert_eq!(counter_delta(u32::MAX - 4, 5), 10);
        let mut prev = HashMap::new();
        prev.insert("en0".to_string(), (100u32, 50u32));
        prev.insert("lo0".to_string(), (0u32, 0u32));
        let mut cur = HashMap::new();
        cur.insert("en0".to_string(), (1124u32, 150u32));
        cur.insert("lo0".to_string(), (999u32, 999u32));
        cur.insert("en1".to_string(), (5u32, 5u32));
        assert_eq!(net_delta(&prev, &cur, None), (1024, 100));
        assert_eq!(net_delta(&prev, &cur, Some("lo0")), (999, 999));
        assert_eq!(human_rate(512.0), "512 B/s");
        assert_eq!(human_rate(12.0 * 1024.0), "12 KB/s");
        assert_eq!(human_rate(1.5 * 1024.0 * 1024.0), "1.5 MB/s");
        assert_eq!(human_rate(-3.0), "0 B/s");
    }

    #[test]
    fn misc_math() {
        assert_eq!(format_gb(16 * 1024 * 1024 * 1024), "16.0");
        assert_eq!(percent_of(1, 3), 33);
        assert_eq!(percent_of(5, 0), 0);
        assert_eq!(memory_used_pages(100, 10, 20, 5), 115);
        assert_eq!(memory_used_pages(5, 10, 20, 5), 25);
    }
}
