# JankyBorders: private and system API surface (for Rust FFI)

Source: JankyBorders v1.9.0 @ `a7297ca`. All paths are relative to `src/`. The
signatures below are copied **exactly as JankyBorders declares them**
(`misc/extern.h` unless noted). They are reverse-engineered. Use the
"FFI notes" column for the Rust type mapping.

Type mapping used throughout:
- `int` / `CGError` / `OSStatus` → `i32`
- `uint32_t` wid → `u32`
- `uint64_t` sid / tags → `u64`
- `CFTypeRef`, `CFArrayRef`, etc. → `*const c_void` (or core-foundation types)
- `bool` → Rust `bool` (C `_Bool`, 1 byte)
- `CGRect` / `CGPoint` / `CGAffineTransform` → `#[repr(C)]` f64 structs (all of
  these are `CGFloat` = f64 on 64-bit)
- `float` → `f32`, `double` → `f64`

Link with `-framework SkyLight` (in `/System/Library/PrivateFrameworks`) plus
CoreGraphics, CoreFoundation, ApplicationServices (AX/HIServices), CoreVideo and
AppKit.

---

## 1. SkyLight: connections, events, notifications

| Symbol | Declared signature | Used at | FFI notes |
|---|---|---|---|
| `SLSMainConnectionID` | `int SLSMainConnectionID();` | `main.c:153,205`; `windows.c:33,236,241,259,311`; `border.c:281,299`; `misc/connection.h:100` | `fn() -> i32` |
| `SLSNewConnection` | `CGError SLSNewConnection(int zero, int *cid);` | `border.c:287` (one connection per border) | pass 0 |
| `SLSReleaseConnection` | `CGError SLSReleaseConnection(int cid);` | `border.c:300` | |
| `SLSConnectionGetPID` | `CGError SLSConnectionGetPID(int cid, pid_t *pid);` | `events.c:34`; `windows.c:38`; `misc/ax.h:30` | `pid_t` = i32 |
| `SLSGetConnectionIDForPSN` | `CGError SLSGetConnectionIDForPSN(int cid, ProcessSerialNumber *psn, int *psn_cid);` | `misc/window.h:57`; `misc/ax.h:27` | PSN = `{u32 high, u32 low}` |
| `_SLPSGetFrontProcess` | `OSStatus _SLPSGetFrontProcess(ProcessSerialNumber *psn);` | `misc/window.h:55`; `misc/ax.h:25` | exported by SkyLight |
| `SLSRegisterNotifyProc` | `CGError SLSRegisterNotifyProc(void* handler, uint32_t event, void* context);` | `events.c:113-127` (+ `131` debug) | handler is `extern "C" fn(u32 event, *mut c_void data, usize len, *mut c_void ctx)` (JB types the 4th parameter as `int cid`) |
| `SLSRequestNotificationsForWindows` | `CGError SLSRequestNotificationsForWindows(int cid, uint32_t *window_list, int window_count);` | `windows.c:237` | replaces the whole list each call |
| `SLSGetEventPort` | `CGError SLSGetEventPort(int cid, mach_port_t* port_out);` | `main.c:209` | `mach_port_t` = u32 |
| `SLEventCreateNextEvent` | `CGEventRef SLEventCreateNextEvent(int cid);` | `main.c:154,158` | returns +1 retained; `CFRelease` |
| `_CFMachPortSetOptions` | `void _CFMachPortSetOptions(CFMachPortRef mach_port, int options);` | `main.c:217` (options `0x40`) | private CoreFoundation export |
| `SLSServerPort` | `mach_port_t SLSServerPort(void* zero);` | declared, **unused** | |
| `SLSWindowManagementBridgeSetDelegate` | `CGError SLSWindowManagementBridgeSetDelegate(void* delegate);` | declared, **unused** | |
| `SLSCopyConnectionProperty` | `CGError SLSCopyConnectionProperty(int cid, int target_cid, CFStringRef key, CFTypeRef* value);` | declared, **unused** | |

## 2. SkyLight: window queries and iterators

| Symbol | Declared signature | Used at | FFI notes |
|---|---|---|---|
| `SLSCopyWindowsWithOptionsAndTags` | `CFArrayRef SLSCopyWindowsWithOptionsAndTags(int cid, uint32_t owner, CFArrayRef spaces, uint32_t options, uint64_t *set_tags, uint64_t *clear_tags);` | `misc/window.h:66` (owner = front cid); `windows.c:278,349` (owner 0) | options `0x2`, `*set_tags = 1`, `*clear_tags = 0`; `spaces` = CFArray of CFNumber SInt64; returns CFArray of CFNumber wids (+1) |
| `SLSWindowQueryWindows` | `CFTypeRef SLSWindowQueryWindows(int cid, CFArrayRef windows, uint32_t options);` | `misc/window.h:34,75,204`; `windows.c:51,286,358` | options `0x0`; `windows` = CFArray of CFNumber SInt32 (single-window queries) or the array returned by `SLSCopyWindowsWithOptionsAndTags` (`window.h:75`, `windows.c:286,358`); the result is not NULL-checked at `window.h:205` and `windows.c:359` |
| `SLSWindowQueryResultCopyWindows` | `CFTypeRef SLSWindowQueryResultCopyWindows(CFTypeRef window_query);` | same sites | returns an iterator (+1) |
| `SLSWindowIteratorGetCount` | `int SLSWindowIteratorGetCount(CFTypeRef iterator);` | `misc/window.h:38,78`; `windows.c:54` | |
| `SLSWindowIteratorAdvance` | `bool SLSWindowIteratorAdvance(CFTypeRef iterator);` | `misc/window.h:39,79,207`; `windows.c:55,290,361` | must be called before the first read |
| `SLSWindowIteratorGetParentID` | `uint32_t SLSWindowIteratorGetParentID(CFTypeRef iterator);` | `misc/window.h:16` | |
| `SLSWindowIteratorGetWindowID` | `uint32_t SLSWindowIteratorGetWindowID(CFTypeRef iterator);` | `misc/window.h:81`; `windows.c:292,363` | |
| `SLSWindowIteratorGetTags` | `uint64_t SLSWindowIteratorGetTags(CFTypeRef iterator);` | `misc/window.h:14,40` | |
| `SLSWindowIteratorGetAttributes` | `uint64_t SLSWindowIteratorGetAttributes(CFTypeRef iterator);` | `misc/window.h:15` | |
| `SLSWindowIteratorGetLevel` | `int SLSWindowIteratorGetLevel(CFTypeRef iterator);` | `misc/window.h:208` | |
| `SLSWindowIteratorGetCornerRadii` | `CFArrayRef (*)(CFTypeRef)` (function pointer `JBSLSWindowIteratorGetCornerRadii`, `main.c:25`; extern in `windows.c:12`) | resolved `main.c:162-169` (macOS ≥ 26 only, `dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight")` + `dlsym`); used `windows.c:67-74` | returns CFArray of CFNumber (element 0 read as SInt32); +1 (released) |
| `SLSGetWindowBounds` | `CGError SLSGetWindowBounds(int cid, uint32_t wid, CGRect *frame);` | `border.c:36,318`; `misc/yabai.h:67` | global coordinates, top-left origin |
| `SLSGetWindowOwner` | `CGError SLSGetWindowOwner(int cid, uint32_t wid, int* out_cid);` | `events.c:32`; `windows.c:35` | |
| `SLSWindowIsOrderedIn` | `CGError SLSWindowIsOrderedIn(int cid, uint32_t wid, bool* shown);` | `border.c:189` | `*mut bool` |
| `SLSGetWindowTransform` | `CGError SLSGetWindowTransform(int cid, uint32_t wid, CGAffineTransform* transform);` | `misc/yabai.h:39` | |
| `SLSGetWindowLevel` | `CGError SLSGetWindowLevel(int cid, uint32_t wid, int64_t* level_out);` | declared, **unused** | |
| `SLSGetWindowSubLevel` | `int32_t SLSGetWindowSubLevel(int cid, uint32_t wid);` | declared, **unused** (replaced by the raw MIG call, §8) | candidate fallback for mbar |

## 3. SkyLight: displays and spaces

| Symbol | Declared signature | Used at | FFI notes |
|---|---|---|---|
| `SLSCopyManagedDisplays` | `CFArrayRef SLSCopyManagedDisplays(int cid);` | `misc/space.h:33`; `windows.c:260` | CFArray of CFString display UUIDs |
| `SLSCopyManagedDisplaySpaces` | `CFArrayRef SLSCopyManagedDisplaySpaces(int cid);` | `windows.c:315` | array of dicts; keys `"Spaces"` → array of dicts with `"id64"` (CFNumber) |
| `SLSManagedDisplayGetCurrentSpace` | `uint64_t SLSManagedDisplayGetCurrentSpace(int cid, CFStringRef uuid);` | `misc/window.h:126`; `misc/space.h:27,38`; `windows.c:265` | |
| `SLSCopyManagedDisplayForWindow` | `CFStringRef SLSCopyManagedDisplayForWindow(int cid, uint32_t wid);` | `misc/window.h:124` | |
| `SLSCopyActiveMenuBarDisplayIdentifier` | `CFStringRef SLSCopyActiveMenuBarDisplayIdentifier(int cid);` | `misc/space.h:23` | |
| `SLSCopySpacesForWindows` | `CFArrayRef SLSCopySpacesForWindows(int cid, int selector, CFArrayRef window_list);` | `misc/window.h:106` | selector `0x7`; returns CFNumbers |
| `SLSMoveWindowsToManagedSpace` | `CGError SLSMoveWindowsToManagedSpace(int cid, CFArrayRef window_list, uint64_t sid);` | `misc/window.h:224` (called with a u32-truncated sid) | |

## 4. SkyLight: window creation and properties

| Symbol | Declared signature | Used at | FFI notes |
|---|---|---|---|
| `SLSNewWindow` | `CGError SLSNewWindow(int cid, int type, float x, float y, CFTypeRef region, uint32_t *wid);` | `misc/window.h:253` | `type = kCGBackingStoreBuffered (2)`, x = y = `-9999.0` (**f32**) |
| `SLSNewWindowWithOpaqueShapeAndContext` | `CGError SLSNewWindowWithOpaqueShapeAndContext(int cid, int type, CFTypeRef region, CFTypeRef opaque_shape, int options, uint64_t *tags, float x, float y, int tag_size, uint32_t *wid, void *context);` | `misc/window.h:239` (yabai proxies) | options `13 \| (1<<18)` = `0x4000D`; tags `(1<<1)\|(1<<9)`; tag_size 64; `opaque_shape` = empty region; context NULL |
| `SLSReleaseWindow` | `CGError SLSReleaseWindow(int cid, uint32_t wid);` | `border.c:19` | |
| `SLSSetWindowTags` | `CGError SLSSetWindowTags(int cid, uint32_t wid, uint64_t* tags, int tag_size);` | `border.c:254`; `misc/window.h:266` | tag_size `64` (= `0x40`) bits |
| `SLSClearWindowTags` | `CGError SLSClearWindowTags(int cid, uint32_t wid, uint64_t* tags, int tag_size);` | `border.c:255`; `misc/window.h:267` | |
| `SLSSetWindowShape` | `CGError SLSSetWindowShape(int cid, uint32_t wid, float x_offset, float y_offset, CFTypeRef shape);` | `border.c:214` | offsets are f32 (the border origin) |
| `SLSSetWindowResolution` | `CGError SLSSetWindowResolution(int cid, uint32_t wid, double res);` | `misc/window.h:265` | **f64**: 1.0 or 2.0 |
| `SLSSetWindowOpacity` | `CGError SLSSetWindowOpacity(int cid, uint32_t wid, bool isOpaque);` | `misc/window.h:268` | false |
| `SLSSetWindowAlpha` | `CGError SLSSetWindowAlpha(int cid, uint32_t wid, float alpha);` | `misc/window.h:250` | 0.0 for proxies |
| `SLSWindowSetShadowProperties` | `CGError SLSWindowSetShadowProperties(uint32_t wid, CFDictionaryRef properties);` | `misc/window.h:284` | **no cid**; dict `{"com.apple.WindowShadowDensity": CFNumber(CFIndex 0)}` |
| `SLSSetWindowBackgroundBlurRadius` | `CGError SLSSetWindowBackgroundBlurRadius(int cid, uint32_t wid, uint32_t radius);` | declared, **unused** | |
| `SLSSetWindowShadowParameters` | `CGError SLSSetWindowShadowParameters(int cid, uint32_t wid, float std, float density, int x_offset, int y_offset);` | declared, **unused** | |
| `SLSSetWindowTransform` | `CGError SLSSetWindowTransform(int cid, uint32_t wid, CGAffineTransform transform);` | declared, **unused** | |
| `SLWindowContextCreate` | `CGContextRef SLWindowContextCreate(int cid, uint32_t wid, CFDictionaryRef options);` | `border.c:168` | options NULL; release with `CGContextRelease` |
| `SLSFlushWindowContentRegion` | `CGError SLSFlushWindowContentRegion(int cid, uint32_t wid, void* dirty);` | `border.c:157` | dirty NULL |
| `SLSWindowFreezeWithOptions` | `CGError SLSWindowFreezeWithOptions(int cid, uint32_t wid, CFTypeRef options);` | `border.c:213` | options NULL |
| `SLSWindowThaw` | `CGError SLSWindowThaw(int cid, uint32_t wid);` | `border.c:158` | |
| `SLSDisableUpdate` | `CGError SLSDisableUpdate(int cid);` | `border.c:208` | |
| `SLSReenableUpdate` | `CGError SLSReenableUpdate(int cid);` | `border.c:257` | |
| `CGSNewRegionWithRect` | `CGError CGSNewRegionWithRect(CGRect *rect, CFTypeRef *outRegion);` | `border.c:211`; `misc/window.h:234` | rect passed **by pointer**; region +1 |
| `CGRegionCreateEmptyRegion` | `CFTypeRef CGRegionCreateEmptyRegion(void);` | `misc/window.h:238` | private CoreGraphics export |

## 5. SkyLight: transactions

| Symbol | Declared signature | Used at | FFI notes |
|---|---|---|---|
| `SLSTransactionCreate` | `CFTypeRef SLSTransactionCreate(int cid);` | `border.c:223,326,368,392`; `misc/yabai.h:48,93,168` | may return NULL; +1 |
| `SLSTransactionCommit` | `CGError SLSTransactionCommit(CFTypeRef transaction, int synchronous);` | same sites | always `0` (async) |
| `SLSTransactionMoveWindowWithGroup` | `CGError SLSTransactionMoveWindowWithGroup(CFTypeRef transaction, uint32_t wid, CGPoint point);` | `border.c:225,328` | CGPoint **by value** (2×f64) |
| `SLSTransactionOrderWindow` | `CGError SLSTransactionOrderWindow(CFTypeRef transaction, uint32_t wid, int order, uint32_t rel_wid);` | `border.c:239,370,394`; `misc/yabai.h:95` | order `+1` above, `-1` below, `0` out |
| `SLSTransactionSetWindowLevel` | `CGError SLSTransactionSetWindowLevel(CFTypeRef transaction, uint32_t wid, int level);` | `border.c:237` | |
| `SLSTransactionSetWindowSubLevel` | `CGError SLSTransactionSetWindowSubLevel(CFTypeRef transaction, uint32_t wid, int level);` | `border.c:238` | |
| `SLSTransactionSetWindowTransform` | `CGError SLSTransactionSetWindowTransform(CFTypeRef transaction, uint32_t wid, int not, int important, CGAffineTransform transform);` | `border.c:231`; `misc/yabai.h:50,51` | 3rd/4th args always `0, 0`; CGAffineTransform (6×f64 = 48 bytes) **by value**. With `extern "C"` and a `#[repr(C)]` struct, Rust passes it indirectly per AAPCS64 |
| `SLSTransactionSetWindowAlpha` | `CGError SLSTransactionSetWindowAlpha(CFTypeRef transaction, uint32_t wid, float alpha);` | `misc/yabai.h:100,101,170,171` | f32 |
| `SLSTransactionSetWindowShape` | `CGError SLSTransactionSetWindowShape(CFTypeRef transaction, uint32_t wid, float x_offset, float y_offset, CFTypeRef shape);` | declared, **unused** | |
| `SLSTransactionSetWindowSystemAlpha` | `CGError SLSTransactionSetWindowSystemAlpha(CFTypeRef transaction, uint32_t wid, float alpha);` | declared, **unused** | |
| `SLSTransactionCommitUsingMethod` | `CGError SLSTransactionCommitUsingMethod(CFTypeRef transaction, uint32_t method);` | declared, **unused** | |

## 6. Hidden (non-exported) symbols

| Symbol | Signature (as used) | How resolved | Used at |
|---|---|---|---|
| `CGSGetConnectionPortById` | `mach_port_t (*)(int cid)` | `macho_find_symbol("/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight", "_CGSGetConnectionPortById")`: walk `_dyld_image_count`/`_dyld_get_image_name` for an exact path match, find `__LINKEDIT` and `LC_SYMTAB`, then linear-scan `nlist_64` by name; address = `n_value + slide` (`misc/connection.h:5-96`) | `misc/connection.h:98-101` → `main.c:203` (`g_server_port`) |

Rust: reimplement with `_dyld_*` from libSystem plus `mach_header_64` parsing,
or try `dlsym` first, which fails because the symbol is local. Guard against
NULL; the C code doesn't.

## 7. Accessibility (ApplicationServices / HIServices)

| Symbol | Signature | Used at | Notes |
|---|---|---|---|
| `_AXUIElementGetWindow` | `void _AXUIElementGetWindow(CFTypeRef window, uint32_t* wid);` (as declared in `misc/ax.h:5`; Apple's real return type is `AXError`) | `misc/ax.h:38` | private |
| `AXIsProcessTrusted` | `Boolean AXIsProcessTrusted(void)` | `misc/ax.h:9` (via `main.c:186`) | |
| `AXIsProcessTrustedWithOptions` | `Boolean AXIsProcessTrustedWithOptions(CFDictionaryRef)` | `misc/ax.h:12` | `{kAXTrustedCheckOptionPrompt: kCFBooleanTrue}` |
| `AXUIElementCreateApplication` | `AXUIElementRef AXUIElementCreateApplication(pid_t)` | `misc/ax.h:32` | |
| `AXUIElementCopyAttributeValue` | `AXError AXUIElementCopyAttributeValue(AXUIElementRef, CFStringRef, CFTypeRef*)` | `misc/ax.h:34` | attribute `kAXFocusedWindowAttribute` (`"AXFocusedWindow"`) |

## 8. Raw MIG call: window sub-level (`misc/window.h:134-196`)

```c
#pragma pack(push,2)
struct {
  struct { mach_msg_header_t header; /*24*/ NDR_record_t NDR_record; /*8*/ } info;
  struct { int32_t wid; } payload;                      // offset 32
  struct { int32_t sub_level; int64_t padding; } response; // sub_level at offset 36
} msg;                                                  // sizeof = 48
#pragma pack(pop)
```
- Request id: `0x73c3` (29635). On macOS ≥ 26 (runtime): `0x76e3` (30435).
- Expected reply id: `0x7427` (29735). On macOS ≥ 26: `0x7747` (30535).
- `msgh_remote_port = CGSGetConnectionPortById(main_cid)`, `msgh_local_port = mig_get_special_reply_port()`.
- `msgh_bits = MACH_MSGH_BITS_SET(MACH_MSG_TYPE_COPY_SEND(19), MACH_MSG_TYPE_MAKE_SEND_ONCE(21), 0, MACH_MSGH_BITS_REMOTE_MASK)` → `0x00001513` (the "other" argument is masked away).
- `mach_msg(&hdr, MACH_SEND_MSG|MACH_SEND_SYNC_OVERRIDE|MACH_SEND_PROPAGATE_QOS|MACH_RCV_MSG|MACH_RCV_SYNC_WAIT, send_size=36, rcv_size=48, rcv_name=reply_port, MACH_MSG_TIMEOUT_NONE, MACH_PORT_NULL)`.
- `NDR_record` is the libsystem global `NDR_record_t NDR_record` (8 bytes: `{0,0,0,0,1(int_rep LE),0,0,0}`).

| Symbol | Declared signature | Used at |
|---|---|---|
| `mig_get_special_reply_port` | `mach_port_t mig_get_special_reply_port(void);` | `misc/window.h:162` |
| `mig_dealloc_special_reply_port` | `mach_port_t mig_dealloc_special_reply_port(mach_port_t port);` | `misc/window.h:185` |

## 9. Mach / bootstrap / process

| Symbol | Signature | Used at | Notes |
|---|---|---|---|
| `task_get_special_port` | `kern_return_t (task_t, int which, mach_port_t*)` | `mach.c:9,67` | `TASK_BOOTSTRAP_PORT` (4) |
| `bootstrap_look_up` | `kern_return_t (mach_port_t bp, const name_t name, mach_port_t* sp)` | `mach.c:16` | `"git.felix.borders"` |
| `bootstrap_register` | `kern_return_t (mach_port_t bp, name_t name, mach_port_t sp)` (deprecated) | `mach.c:73` | `"git.felix.borders"`, `"git.felix.jbevent"` (yabai.h:242) |
| `mach_port_allocate` / `mach_port_set_attributes` / `mach_port_insert_right` | standard | `mach.c:84-105`; `yabai.h:227-240` | `MACH_PORT_LIMITS_INFO`; qlimit `MACH_PORT_QLIMIT_LARGE` (1024) for borders, `1` for jbevent. `mach_port_insert_right(MAKE_SEND)` is called only for borders (`mach.c:101`); the jbevent port is registered without it |
| `mach_msg` / `mach_msg_destroy` | standard | `mach.c:45,58`; `window.h:171,191`; `yabai.h:220` | |
| `CFMachPortCreateWithPort` / `CFMachPortCreateRunLoopSource` / `CFRunLoopAddSource` | standard CF | `main.c:211-224`; `mach.c:115-125`; `yabai.h:246-256` | |
| `pid_for_task` | `kern_return_t (mach_port_name_t, int*)` | `main.c:200` | own pid |
| `proc_name` | `int proc_name(int pid, void* buffer, uint32_t buffersize)` (libproc) | `windows.c:40` | buffer `PROC_PIDPATHINFO_MAXSIZE` (4096) |
| `dlopen` / `dlsym` | standard | `main.c:164-166` | |
| `_dyld_image_count`, `_dyld_get_image_name`, `_dyld_get_image_header`, `_dyld_get_image_vmaddr_slide` | `<mach-o/dyld.h>` | `misc/connection.h:7-20` | |

IPC message struct (`mach.h:6-10`), 44 bytes, used for both services:
```c
struct mach_message {
  mach_msg_header_t header;               // 24
  mach_msg_size_t msgh_descriptor_count;  // 4
  mach_msg_ool_descriptor_t descriptor;   // 16: {void* address; uint32 deallocate:8, copy:8, pad1:8, type:8; uint32 size}
};
```
Note: in the 64-bit `mach_msg_ool_descriptor_t` layout the fields are `address`
(8), then the bitfields (`deallocate:8, copy:8, pad1:8, type:8`), then
`size` (4). `#[repr(C, packed(4))]` gives 44 bytes total.

yabai payload (`yabai.h:197-202`), 4104 bytes:
```c
struct { uint32_t event; uint32_t count; uint32_t proxy_wid[512]; uint32_t real_wid[512]; };
```
SLS spawn-event payload (`events.c:25-28`, events 1325/1326):
```c
struct window_spawn_data { uint64_t sid; uint32_t wid; };
```
Window-modify payload (events 723, 804-816, 1322): `uint32_t wid` at offset 0.

## 10. CoreVideo (`animation.c`, `misc/yabai.h`)

`CVDisplayLinkCreateWithActiveCGDisplays`, `CVDisplayLinkGetNominalOutputVideoRefreshPeriod`
(→ `CVTime{i64 timeValue, i32 timeScale, i32 flags}`), `CVDisplayLinkSetOutputCallback`,
`CVDisplayLinkStart`, `CVDisplayLinkStop`, `CVDisplayLinkRelease`. Output callback signature:
`CVReturn (CVDisplayLinkRef, const CVTimeStamp* now, const CVTimeStamp* out, CVOptionFlags, CVOptionFlags*, void* ctx)`.

## 11. Public CoreGraphics / CoreFoundation used for drawing and data

- **Geometry:** `CGRectInset`, `CGRectEqualToRect`, `CGRectNull`,
  `CGPointZero`, `CGPointMake`, `CGPointApplyAffineTransform`,
  `CGAffineTransformMakeScale`, `CGAffineTransformConcat`,
  `CGAffineTransformIdentity`, `CGSizeZero`.
- **Paths:** `CGPathCreateMutable`, `CGPathAddRect`, `CGPathAddRoundedRect`,
  `CGPathAddPath`, `CGPathCreateWithRect`, `CGPathCreateWithRoundedRect`.
  `CGPathCreateWithRoundedRect` asserts if `2·radius > width/height`; the
  `too_small` check guards this for radius `r`/`r+1`, but not for the fixed
  radius 9 of `style=uniform` when `r < 7` (spec BQ32).
- **Context:** `CGContextSaveGState`, `CGContextRestoreGState`,
  `CGContextSetLineWidth`, `CGContextClearRect`, `CGContextAddPath`,
  `CGContextEOClip`, `CGContextClip`, `CGContextFillPath`,
  `CGContextStrokePath`, `CGContextReplacePathWithStrokedPath`,
  `CGContextDrawLinearGradient` (options 0), `CGContextSetRGBFillColor`,
  `CGContextSetRGBStrokeColor`, `CGContextSetShadowWithColor`,
  `CGContextSetInterpolationQuality` (`kCGInterpolationNone`),
  `CGContextFlush`, `CGContextRelease`.
- **Color:** `CGColorCreateGenericRGB` (glow shadow), `CGColorCreateSRGB`
  (gradient stops), `CGColorRelease`, `CGGradientCreateWithColors` (NULL
  colorspace, NULL locations), `CGGradientRelease`.
- **Displays:** `CGGetActiveDisplayList`, `CGDisplayCreateUUIDFromDisplayID`,
  `CFUUIDCreateString`.
- **CF:** `CFArrayCreate` (`kCFTypeArrayCallBacks`), `CFArrayGetCount`,
  `CFArrayGetValueAtIndex`, `CFNumberCreate`
  (`kCFNumberSInt32Type`/`SInt64Type`/`CFIndexType`), `CFNumberGetValue`,
  `CFNumberGetType`, `CFDictionaryCreate`, `CFDictionaryGetValue`,
  `CFRelease`, `CFRunLoopGetCurrent`, `CFRunLoopGetMain`, `CFRunLoopRun`,
  `CFSTR`.
- **GCD:** `dispatch_async`, `dispatch_get_main_queue`,
  `dispatch_get_global_queue` (`DISPATCH_QUEUE_PRIORITY_HIGH` for moves,
  `_LOW` for delays).
- **pthread:** recursive mutexes, `pthread_main_np`.

## 12. Constants

| Name | Value | Meaning / use |
|---|---|---|
| SLS event 723 | `EVENT_WINDOW_UPDATE` | focus check (50 ms) |
| 804 | `EVENT_WINDOW_CLOSE` | destroy border |
| 806 | `EVENT_WINDOW_MOVE` | move border |
| 807 | `EVENT_WINDOW_RESIZE` | update |
| 808 | `EVENT_WINDOW_REORDER` | update + focus check (10 ms) |
| 811 | `EVENT_WINDOW_LEVEL` | update |
| 815 | `EVENT_WINDOW_UNHIDE` | unhide |
| 816 | `EVENT_WINDOW_HIDE` | hide |
| 1322 | `EVENT_WINDOW_TITLE` | focus check (50 ms) |
| 1325 | `EVENT_WINDOW_CREATE` | create (also the yabai proxy-begin code) |
| 1326 | `EVENT_WINDOW_DESTROY` | destroy (also the yabai proxy-end code) |
| 1401 | `EVENT_SPACE_CHANGE` | redraw current spaces (20 ms) |
| 1508 | `EVENT_FRONT_CHANGE` | focus check (50 ms) |
| `WINDOW_TAG_DOCUMENT` | `1<<0` | suitability |
| `WINDOW_TAG_FLOATING` | `1<<1` | suitability; also set on border windows |
| `WINDOW_TAG_ATTACHED` | `1<<7` | excluded |
| tag `1<<9` | — | set on border windows (meaning not named in source) |
| `WINDOW_TAG_STICKY` | `1<<11` | sticky detection; set on sticky borders |
| `WINDOW_TAG_IGNORES_CYCLE` | `1<<18` | excluded |
| `WINDOW_TAG_MODAL` | `1<<31` | suitability |
| tag `1<<45` | — | cleared on sticky borders |
| tag `1<<58` (`0x400000000000000`) | — | alternative to attribute `0x2` in suitability |
| attribute `0x2` | — | suitability |
| `SLSCopyWindowsWithOptionsAndTags` options | `0x2` | |
| `SLSCopySpacesForWindows` selector | `0x7` | all spaces |
| `_CFMachPortSetOptions` | `0x40` | on the SLS event port |
| `kCGBackingStoreBuffered` | `2` | window type |
| proxy window options | `13 \| (1<<18)` | `SLSNewWindowWithOpaqueShapeAndContext` |
| tag_size | `64` / `0x40` | Set/ClearWindowTags |
| window origin at creation | `-9999, -9999` | off-screen |
| shadow dict key | `"com.apple.WindowShadowDensity"` = 0 | no shadow |
| bootstrap names | `"git.felix.borders"`, `"git.felix.jbevent"` | |
