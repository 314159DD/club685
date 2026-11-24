//! Deck-state exporter. Polls the rekordbox engine through its own per-deck getters and the
//! deck's beat grid, and sends one JSON datagram per tick to a local UDP port.
//!
//! Addresses and the access recipe: re/docs/objects/deck-state-access.md
//! ("Beat, bar and phase per deck (v2 recipe)"). Read-only: no rekordbox state is written.
//! The only side effect is taking and releasing one reference on the deck's beat grid per read.

use crate::diag;
use crate::guard::rva;
use crate::log;
use std::ffi::c_void;
use std::net::UdpSocket;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// `DAT_14442c0a8`: the engine manager singleton returned by `FUN_1402807e0`.
const VA_ENGINE_SINGLETON: usize = 0x1_4442_c0a8;
// FUN_140281590 is `mov eax,[rcx+0x10]; ret` (a `this` getter, not a no-arg function). Never call it bare.
/// `FUN_1411c9750(mgr, deck) -> i32`: play position in ms.
const VA_GET_POSITION_MS: usize = 0x1_411c_9750;
// FUN_1411c7b70 / FUN_1411c9c10 are no longer called (live run 1: master always false, flags always 0).
// Both are `(rcx = mgr, edx = deck) -> al/eax`, no hidden return pointer. See deck-state-access.md "Resolved (live run 1)".
/// `FUN_1402dd1b0(&grid_ref)`: releases a FormattedBeatGrid reference.
const VA_GRID_RELEASE: usize = 0x1_402d_d1b0;
/// `FormattedBeatGrid` vtable: entries are a plain array only for this class.
const VA_FORMATTED_BEAT_GRID_VTBL: usize = 0x1_43be_8d40;
/// `DAT_1443c6b10`: LoadedContentsManager root (browse side), rows per deck at +0x38 + 8*deck.
const VA_LOADED_CONTENTS: usize = 0x1_443c_6b10;

const OFF_MGR_SYSTEM: usize = 0xf0;
const OFF_SYS_DECKS: usize = 0x3f8;
const OFF_SYS_DECK_COUNT: usize = 0x408;
/// Deck wrapper vtable slot returning the `DjPlayerUnitState*`.
const OFF_WRAPPER_VF_STATE: usize = 0x2a8;
/// `DjPlayerUnitState` vtable: slot 4 = get beat grid (ref-counted out-pointer), slot 41 = tempo offset (f32).
const OFF_STATE_VF_GRID: usize = 4 * 8;
const OFF_STATE_VF_TEMPO: usize = 41 * 8;
/// `DjPlayerUnitState+0x10` -> DjPlayerState: +0x10 playing (u8), +0x20 loaded track (non-null).
const OFF_STATE_DPS: usize = 0x10;
const OFF_DPS_PLAYING: usize = 0x10;
const OFF_DPS_TRACK: usize = 0x20;
/// `DjPlayerState+0x68` = CueBehaveDispatcher: +0x350 i32 index, +0x372 byte (inputs of `DjPlayerUnitState::vf12`).
const OFF_DPS_CUE: usize = 0x68;
const OFF_CUE_IDX: usize = 0x350;
const OFF_CUE_FLAG: usize = 0x372;
/// `DjPlayerUnitState+0x20` -> DjPlayerState with a weak ref at +0x90 (load pending, vf12 flag value 1).
const OFF_STATE_PENDING: usize = 0x20;
const OFF_PENDING_REF: usize = 0x90;
/// The wrapper in the deck vector is the `IDjPlayerControl` subobject at `DjPlayerUnit+0xe0`.
const OFF_WRAPPER_TO_UNIT: usize = 0xe0;
const OFF_UNIT_STATE: usize = 0x1d8;
/// `DjPlayerUnit+0x238` = DjSyncMasterBehavior (the deck's ISyncMaster).
const OFF_UNIT_SYNC_MASTER: usize = 0x238;
/// ISyncMaster participant: +0x18 isMaster byte, +0x20 back-pointer to the SyncManager. SyncManager+0xf0 = current master.
const OFF_PART_IS_MASTER: usize = 0x18;
const OFF_PART_MANAGER: usize = 0x20;
const OFF_MANAGER_MASTER: usize = 0xf0;
/// FormattedBeatGrid: refcount +8, magic +0x10, entries begin +0x20, end +0x28. Entry is 16 bytes.
const GRID_MAGIC: u32 = 0x5243_4f58;
const OFF_GRID_MAGIC: usize = 0x10;
const OFF_GRID_BEGIN: usize = 0x20;
const OFF_GRID_END: usize = 0x28;
const GRID_ENTRY: usize = 16;
const OFF_ENTRY_BPM: usize = 0;
const OFF_ENTRY_BEAT_IN_BAR: usize = 6;
const OFF_ENTRY_TIME_MS: usize = 8;
/// Upper bound on entries we accept (a 2 h track at 300 BPM is 36 000 beats).
const MAX_GRID_ENTRIES: usize = 100_000;
const OFF_ROW_CONTENT_ID: usize = 0x4d4;
/// `DjPlayerUnitState+0x28` -> sync slave dispatcher; its +0x248 points at the current master's
/// play-head info and is NULL on the master deck (observed live, diag run 1).
const OFF_STATE_SYNC_DISPATCH: usize = 0x28;
const OFF_DISPATCH_MASTER_HEAD: usize = 0x248;

type DeckGetterI32 = unsafe extern "system" fn(*mut c_void, u32) -> i32;
type ObjFn = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type ObjOut = unsafe extern "system" fn(*mut c_void, *mut usize) -> *mut c_void;
type ObjFloat = unsafe extern "system" fn(*mut c_void) -> f32;
type RefRelease = unsafe extern "system" fn(*mut usize);

/// True when `len` bytes at `p` are committed and readable (single region).
fn readable(p: usize, len: usize) -> bool {
    if p < 0x10000 {
        return false;
    }
    unsafe {
        let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        if VirtualQuery(p as *const c_void, &mut mbi, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) == 0 {
            return false;
        }
        let end = mbi.BaseAddress as usize + mbi.RegionSize;
        mbi.State == MEM_COMMIT && mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 && p + len <= end
    }
}

fn rd<T: Copy>(p: usize) -> Option<T> {
    readable(p, std::mem::size_of::<T>()).then(|| unsafe { std::ptr::read_unaligned(p as *const T) })
}

/// Virtual function pointer at `slot_off` of the object at `obj`.
fn vfn(obj: usize, slot_off: usize) -> Option<usize> {
    rd::<usize>(rd::<usize>(obj)? + slot_off)
}

#[derive(Default)]
struct Beat {
    index: i32,
    bar: i32,
    beat_in_bar: u8,
    phase_beat: f64,
    phase_bar: f64,
    grid_bpm: f64,
    count: usize,
}

#[derive(Default)]
struct Deck {
    index: u32,
    position_ms: i32,
    master: bool,
    master_source: &'static str,
    sync_obj: usize,
    sync_mgr: usize,
    /// `*(*(state+0x28)+0x248)`: master's play-head pointer, 0 on the master deck itself.
    follow_master: Option<usize>,
    p18: Option<u8>,
    flags: u32,
    loaded: bool,
    playing: Option<bool>,
    tempo: Option<f32>,
    content_id: Option<i32>,
    beat: Option<Beat>,
}

fn num(v: Option<String>) -> String {
    v.unwrap_or_else(|| "null".into())
}

impl Deck {
    fn json(&self) -> String {
        let b = self.beat.as_ref();
        let bpm = match (b, self.tempo) {
            (Some(b), Some(t)) => Some(format!("{:.4}", b.grid_bpm * (1.0 + t as f64))),
            _ => None,
        };
        format!(
            concat!(
                "{{\"deck\":{},\"pos_ms\":{},\"master\":{},\"master_source\":\"{}\",\"flags\":{},\"loaded\":{},\"playing\":{},",
                "\"grid_bpm\":{},\"tempo\":{},\"bpm\":{},\"content_id\":{},",
                "\"beat\":{},\"bar\":{},\"beat_in_bar\":{},\"phase_beat\":{},\"phase_bar\":{},\"grid_beats\":{}}}"
            ),
            self.index,
            self.position_ms,
            self.master,
            self.master_source,
            self.flags,
            self.loaded,
            num(self.playing.map(|p| p.to_string())),
            num(b.map(|b| format!("{:.4}", b.grid_bpm))),
            num(self.tempo.map(|t| format!("{t:.5}"))),
            num(bpm),
            num(self.content_id.map(|c| c.to_string())),
            num(b.map(|b| b.index.to_string())),
            num(b.map(|b| b.bar.to_string())),
            num(b.map(|b| b.beat_in_bar.to_string())),
            num(b.map(|b| format!("{:.5}", b.phase_beat))),
            num(b.map(|b| format!("{:.5}", b.phase_bar))),
            num(b.map(|b| b.count.to_string())),
        )
    }
}

struct Engine {
    base: usize,
}

impl Engine {
    fn mgr(&self) -> Option<usize> {
        rd::<usize>(rva(self.base, VA_ENGINE_SINGLETON)).filter(|&m| m != 0)
    }

    fn deck_count(&self, mgr: usize) -> Option<u32> {
        let sys = rd::<usize>(mgr + OFF_MGR_SYSTEM).filter(|&s| s != 0)?;
        let n: u32 = rd(sys + OFF_SYS_DECK_COUNT)?;
        Some(n.min(4))
    }

    fn wrapper_of(&self, mgr: usize, deck: u32) -> Option<usize> {
        let sys: usize = rd(mgr + OFF_MGR_SYSTEM)?;
        let data: usize = rd(sys + OFF_SYS_DECKS)?;
        rd::<usize>(data + 8 * deck as usize).filter(|&w| w != 0)
    }

    fn state_of(&self, wrapper: usize) -> Option<usize> {
        let f = vfn(wrapper, OFF_WRAPPER_VF_STATE)?;
        let state = unsafe { std::mem::transmute::<usize, ObjFn>(f)(wrapper as *mut c_void) } as usize;
        readable(state, 0x40).then_some(state)
    }

    /// Takes a grid reference, reads it, releases it. `pos_ms` is the single position sample for this tick.
    fn beat_of(&self, state: usize, pos_ms: f64) -> Option<Beat> {
        let f = vfn(state, OFF_STATE_VF_GRID)?;
        let mut grid: usize = 0;
        unsafe { std::mem::transmute::<usize, ObjOut>(f)(state as *mut c_void, &mut grid) };
        if grid == 0 {
            return None;
        }
        let result = self.read_grid(grid, pos_ms);
        unsafe { std::mem::transmute::<usize, RefRelease>(rva(self.base, VA_GRID_RELEASE))(&mut grid) };
        result
    }

    fn read_grid(&self, grid: usize, pos_ms: f64) -> Option<Beat> {
        if rd::<usize>(grid)? != rva(self.base, VA_FORMATTED_BEAT_GRID_VTBL) || rd::<u32>(grid + OFF_GRID_MAGIC)? != GRID_MAGIC {
            return None;
        }
        let begin: usize = rd(grid + OFF_GRID_BEGIN)?;
        let end: usize = rd(grid + OFF_GRID_END)?;
        if end <= begin || (end - begin) % GRID_ENTRY != 0 {
            return None;
        }
        let count = (end - begin) / GRID_ENTRY;
        if count == 0 || count > MAX_GRID_ENTRIES || !readable(begin, end - begin) {
            return None;
        }
        let e = |i: usize| begin + GRID_ENTRY * i;
        let t = |i: usize| unsafe { std::ptr::read_unaligned((e(i) + OFF_ENTRY_TIME_MS) as *const f64) };
        let bib = |i: usize| unsafe { *((e(i) + OFF_ENTRY_BEAT_IN_BAR) as *const u8) };
        // lower_bound: first i with t[i] >= pos, then the engine's floor-beat rule.
        let (mut lo, mut hi) = (0usize, count);
        while lo < hi {
            let m = (lo + hi) / 2;
            if t(m) < pos_ms {
                lo = m + 1
            } else {
                hi = m
            }
        }
        let idx = if lo == 0 {
            0
        } else if lo == count {
            count - 1
        } else if t(lo) == pos_ms {
            lo
        } else {
            lo - 1
        };
        let phase_beat = if count < 2 {
            0.0
        } else {
            let (a, b) = if idx + 1 < count { (idx, idx + 1) } else { (idx - 1, idx) };
            let len = t(b) - t(a);
            if len > 0.0 {
                ((pos_ms - t(idx)) / len).clamp(0.0, 0.999_999)
            } else {
                0.0
            }
        };
        // Live grids store beat-in-bar 0..3 (0 = downbeat); older/other grids may use 1..4.
        // Detect the base per grid and always export 1..4.
        let zero_based = (0..count).any(|i| bib(i) == 0);
        let downbeat = if zero_based { 0 } else { 1 };
        let beat_in_bar = (bib(idx) as i32 - downbeat as i32 + 1).clamp(1, 8) as u8;
        let bar = (0..=idx).filter(|&i| bib(i) == downbeat).count() as i32;
        let grid_bpm = unsafe { std::ptr::read_unaligned((e(idx) + OFF_ENTRY_BPM) as *const f32) } as f64;
        let phase_bar = ((beat_in_bar as f64 - 1.0) + phase_beat) / 4.0;
        Some(Beat { index: idx as i32, bar, beat_in_bar, phase_beat, phase_bar, grid_bpm, count })
    }

    fn content_id(&self, deck: u32) -> Option<i32> {
        let root = rd::<usize>(rva(self.base, VA_LOADED_CONTENTS)).filter(|&r| r != 0)?;
        if deck > 3 {
            return None;
        }
        let row = rd::<usize>(root + 0x38 + 8 * deck as usize).filter(|&r| r != 0)?;
        rd::<i32>(row + OFF_ROW_CONTENT_ID)
    }

    fn read_deck(&self, mgr: usize, deck: u32) -> Deck {
        let m = mgr as *mut c_void;
        let mut d = Deck { index: deck, ..Default::default() };
        unsafe {
            d.position_ms = std::mem::transmute::<usize, DeckGetterI32>(rva(self.base, VA_GET_POSITION_MS))(m, deck);
        }
        d.content_id = self.content_id(deck);
        let wrapper = self.wrapper_of(mgr, deck);
        if let Some(w) = wrapper {
            let unit = w - OFF_WRAPPER_TO_UNIT;
            let ok = self.state_of(w).is_some_and(|st| rd::<usize>(unit + OFF_UNIT_STATE) == Some(st));
            if ok {
                d.sync_obj = rd::<usize>(unit + OFF_UNIT_SYNC_MASTER).filter(|&o| o != 0 && o & 7 == 0).unwrap_or(0);
                if d.sync_obj != 0 {
                    d.sync_mgr = rd::<usize>(d.sync_obj + OFF_PART_MANAGER).filter(|&o| o != 0 && o & 7 == 0).unwrap_or(0);
                    d.p18 = rd::<u8>(d.sync_obj + OFF_PART_IS_MASTER);
                }
            }
        }
        if let Some(state) = wrapper.and_then(|w| self.state_of(w)) {
            // Live diag run 1 (SYNC off, MASTER clicked 13x): the object at *(state+0x28) (sync slave
            // dispatcher) holds at +0x248 a pointer to the current master's play-head info. It is NULL on
            // the master deck and non-null on every other loaded deck (balanced accuracy 0.982, both decks).
            d.follow_master = rd::<usize>(state + OFF_STATE_SYNC_DISPATCH)
                .filter(|&p| p != 0 && p & 7 == 0)
                .and_then(|p| rd::<usize>(p + OFF_DISPATCH_MASTER_HEAD));
            let pending = rd::<usize>(state + OFF_STATE_PENDING)
                .filter(|&p| p != 0)
                .and_then(|p| rd::<usize>(p + OFF_PENDING_REF))
                .unwrap_or(0)
                != 0;
            if let Some(dps) = rd::<usize>(state + OFF_STATE_DPS).filter(|&p| p != 0 && p & 3 == 0) {
                d.playing = rd::<u8>(dps + OFF_DPS_PLAYING).map(|b| b != 0);
                let track = rd::<usize>(dps + OFF_DPS_TRACK).unwrap_or(0) != 0;
                let (mut cue_flag, mut cue_idx) = (false, -1i32);
                if let Some(c) = rd::<usize>(dps + OFF_DPS_CUE).filter(|&c| c != 0) {
                    cue_flag = rd::<u8>(c + OFF_CUE_FLAG).unwrap_or(0) != 0;
                    cue_idx = rd::<i32>(c + OFF_CUE_IDX).unwrap_or(-1);
                }
                d.loaded = track;
                d.flags = deck_flags(track, pending, d.playing.unwrap_or(false), cue_flag, cue_idx);
                if track {
                    d.tempo = vfn(state, OFF_STATE_VF_TEMPO)
                        .map(|f| unsafe { std::mem::transmute::<usize, ObjFloat>(f)(state as *mut c_void) });
                    d.beat = self.beat_of(state, d.position_ms as f64);
                }
            }
        }
        d
    }
}

/// Reimplementation of the value `DjPlayerUnitState::vf12` @0x141ad97e0 stores (its body, minus the weak-ref
/// validity guards): 0x10 loaded, 0x100 playing, 0x1000 cue flag, 0x10000 cue index >= 0.
fn deck_flags(track: bool, pending: bool, playing: bool, cue_flag: bool, cue_idx: i32) -> u32 {
    if !track {
        return pending as u32;
    }
    if cue_flag {
        return if playing { 0x1110 } else { 0x1010 };
    }
    if !playing {
        return 0x10;
    }
    0x10110u32.wrapping_add(if cue_idx < 0 { 0xffff_0000 } else { 0 })
}

/// UI "MASTER" state: the deck's ISyncMaster is the one stored at `SyncManager+0xf0`.
/// `mgr_hint` is a SyncManager found through another deck, used when this deck has no back-pointer.
fn sync_master(d: &Deck, mgr_hint: usize) -> (bool, &'static str) {
    let mgr = if d.sync_mgr != 0 { d.sync_mgr } else { mgr_hint };
    if d.sync_obj != 0 && mgr != 0 {
        if let Some(cur) = rd::<usize>(mgr + OFF_MANAGER_MASTER) {
            return (cur == d.sync_obj, "syncmgr+0xf0");
        }
    }
    match d.p18 {
        Some(b) => (b != 0, "participant+0x18"),
        None => (false, "none"),
    }
}

/// QPC time in 100 ns ticks: the same time base as Windows.Graphics.Capture `SystemRelativeTime`.
fn qpc_100ns() -> i64 {
    let (mut c, mut f) = (0i64, 0i64);
    unsafe {
        QueryPerformanceCounter(&mut c);
        QueryPerformanceFrequency(&mut f);
    }
    if f == 0 {
        return 0;
    }
    ((c as i128 * 10_000_000) / f as i128) as i64
}

extern "C" {
    fn rbmod_seh_call(cb: unsafe extern "C" fn(*mut c_void), ctx: *mut c_void, fault: *mut u64) -> u32;
}

struct TickCtx<'a> {
    engine: &'a Engine,
    mgr: usize,
    n: u32,
    out: Vec<String>,
    want_diag: bool,
    first_diag: bool,
    regions: Vec<diag::Region>,
    ptr_log: String,
}

fn grab(regions: &mut Vec<diag::Region>, deck: u32, id: u32, addr: usize, len: usize) {
    let bytes = if readable(addr, len) {
        let mut v = vec![0u8; len];
        unsafe { std::ptr::copy_nonoverlapping(addr as *const u8, v.as_mut_ptr(), len) };
        v
    } else {
        Vec::new()
    };
    regions.push(diag::Region { deck, id, addr: addr as u64, bytes });
}

fn ptr_ok(p: Option<usize>) -> usize {
    p.filter(|&p| p >= 0x10000).unwrap_or(0)
}

/// One-level pointer sweep: every 8-byte aligned value inside [obj, obj+len) that points to readable
/// heap memory (not into the rekordbox image, which holds vtables and code) gets a 0x100-byte snapshot.
/// Region id = id_base + offset of the pointer field, so the diff can name the field that leads to it.
fn sweep(regions: &mut Vec<diag::Region>, image: (usize, usize), deck: u32, id_base: u32, obj: usize, len: usize) {
    if obj == 0 || !readable(obj, len) {
        return;
    }
    for off in (0..len).step_by(8) {
        let Some(p) = rd::<usize>(obj + off) else { continue };
        if p < 0x10000 || p & 7 != 0 || (p >= image.0 && p < image.1) || !readable(p, 0x100) {
            continue;
        }
        grab(regions, deck, id_base + off as u32, p, 0x100);
    }
}

/// Read-only snapshot of the regions listed in the diag spec. `log` receives the pointer chain once.
fn collect_diag(e: &Engine, mgr: usize, n: u32, regions: &mut Vec<diag::Region>, log: Option<&mut String>) {
    use std::fmt::Write;
    let mut log = log;
    for deck in 0..n {
        let Some(w) = e.wrapper_of(mgr, deck) else { continue };
        let unit = w - OFF_WRAPPER_TO_UNIT;
        let state = e.state_of(w).unwrap_or(0);
        let dps = if state != 0 { ptr_ok(rd(state + OFF_STATE_DPS)) } else { 0 };
        let r5 = if state != 0 { ptr_ok(rd(state + 0x28)) } else { 0 };
        let unit_state: Option<usize> = rd(unit + OFF_UNIT_STATE);
        let obj = ptr_ok(rd(unit + OFF_UNIT_SYNC_MASTER));
        let obj_mgr = if obj != 0 { ptr_ok(rd(obj + OFF_PART_MANAGER)) } else { 0 };
        let cue = if dps != 0 { ptr_ok(rd(dps + OFF_DPS_CUE)) } else { 0 };
        if let Some(l) = log.as_deref_mut() {
            let _ = write!(
                l,
                "deck{deck}: wrapper={w:#x} unit={unit:#x} state={state:#x} *(unit+0x1d8)={:#x} match={} dps={dps:#x} *(state+0x28)={r5:#x} obj=*(unit+0x238)={obj:#x} *(obj+0x20)={obj_mgr:#x} cue=*(dps+0x68)={cue:#x}; ",
                unit_state.unwrap_or(0),
                unit_state == Some(state) && state != 0
            );
        }
        grab(regions, deck, 1, w, 0x300);
        grab(regions, deck, 2, unit, 0x2c8);
        grab(regions, deck, 3, state, 0x200);
        grab(regions, deck, 4, dps, 0x200);
        grab(regions, deck, 5, r5, 0x300);
        if obj != 0 {
            grab(regions, deck, 6, obj, 0x200);
        }
        if obj_mgr != 0 {
            grab(regions, deck, 7, obj_mgr, 0x200);
        }
        grab(regions, deck, 8, cue, 0x400);
        // Pass-5 master candidates (sync participant at unit+0x230, slave dispatcher, LocalSyncManager)
        // are reached through pointers in these objects; sweep them one level deep.
        let image = (e.base, e.base + crate::guard::EXPECTED_SIZE_OF_IMAGE as usize);
        sweep(regions, image, deck, 0x1_0000, unit, 0x2c8);
        sweep(regions, image, deck, 0x2_0000, state, 0x200);
        sweep(regions, image, deck, 0x3_0000, dps, 0x200);
    }
    let sys = ptr_ok(rd(mgr + OFF_MGR_SYSTEM));
    if let Some(l) = log.as_deref_mut() {
        let _ = write!(l, "mgr={mgr:#x} sys=*(mgr+0xf0)={sys:#x}");
    }
    grab(regions, diag::GLOBAL_DECK, 100, mgr, 0x200);
    grab(regions, diag::GLOBAL_DECK, 101, sys, 0x500);
    let image = (e.base, e.base + crate::guard::EXPECTED_SIZE_OF_IMAGE as usize);
    sweep(regions, image, diag::GLOBAL_DECK, 0x4_0000, mgr, 0x200);
    sweep(regions, image, diag::GLOBAL_DECK, 0x5_0000, sys, 0x500);
}

/// The dispatcher pointer is NULL on every deck until a MASTER is set (live: both decks null before
/// the first MASTER click, then exactly one null). Enforce at most one master:
/// - exactly one loaded deck null, the others non-null: that deck is master
/// - several (or all) loaded decks null: no master set yet -> all false, "no_master_set"
/// - a single loaded deck: null cannot tell "master" from "unset" -> false, "single_deck_unknown"
fn resolve_master(decks: &mut [Deck]) {
    let via: Vec<usize> = (0..decks.len())
        .filter(|&i| decks[i].loaded && decks[i].master_source == "dispatch+0x248")
        .collect();
    if via.is_empty() {
        return;
    }
    let nulls: Vec<usize> = via.iter().copied().filter(|&i| decks[i].follow_master == Some(0)).collect();
    let (verdict, src): (Option<usize>, &'static str) = match (via.len(), nulls.len()) {
        (1, _) => (None, "single_deck_unknown"),
        (_, 1) => (Some(nulls[0]), "dispatch+0x248"),
        _ => (None, "no_master_set"),
    };
    for &i in &via {
        decks[i].master = verdict == Some(i);
        decks[i].master_source = src;
    }
}

unsafe extern "C" fn tick_cb(p: *mut c_void) {
    let c = &mut *(p as *mut TickCtx);
    let mut decks: Vec<Deck> = (0..c.n).map(|i| c.engine.read_deck(c.mgr, i)).collect();
    let hint = decks.iter().map(|d| d.sync_mgr).find(|&m| m != 0).unwrap_or(0);
    for d in decks.iter_mut() {
        let (m, src) = match (d.loaded, d.follow_master) {
            (true, Some(p)) => (p == 0, "dispatch+0x248"),
            _ => sync_master(d, hint),
        };
        d.master = m;
        d.master_source = src;
    }
    resolve_master(&mut decks);
    c.out = decks.iter().map(|d| d.json()).collect();
    if c.want_diag {
        let log = if c.first_diag { Some(&mut c.ptr_log) } else { None };
        collect_diag(c.engine, c.mgr, c.n, &mut c.regions, log);
    }
}

/// Runs the read under SEH. Err carries (exception code, faulting address).
type TickOut = (Vec<String>, Vec<diag::Region>, String);

fn guarded_read(engine: &Engine, mgr: usize, n: u32, want_diag: bool, first_diag: bool) -> Result<TickOut, (u32, u64)> {
    let mut ctx = TickCtx { engine, mgr, n, out: Vec::new(), want_diag, first_diag, regions: Vec::new(), ptr_log: String::new() };
    let mut fault = 0u64;
    let code = unsafe { rbmod_seh_call(tick_cb, &mut ctx as *mut _ as *mut c_void, &mut fault) };
    if code == 0 { Ok((ctx.out, ctx.regions, ctx.ptr_log)) } else { Err((code, fault)) }
}

/// Runs on the mod thread until an exception disables the exporter.
pub fn run(base: usize) {
    let target = std::env::var("RBMOD_UDP").unwrap_or_else(|_| "127.0.0.1:47990".into());
    let hz: u64 = std::env::var("RBMOD_HZ").ok().and_then(|v| v.parse().ok()).unwrap_or(60).clamp(1, 500);
    let sock = match UdpSocket::bind("127.0.0.1:0") {
        Ok(s) => s,
        Err(e) => {
            log::line(&format!("udp bind failed: {e}"));
            return;
        }
    };
    let engine = Engine { base };
    log::line(&format!("deck exporter: waiting for engine, sending to {target} at {hz} Hz"));
    let period = Duration::from_micros(1_000_000 / hz);
    let started = Instant::now();
    let mut announced = false;
    let mut stable: Option<(usize, Instant)> = None;
    let diag_on = std::env::var("RBMOD_DIAG").map(|v| v == "1").unwrap_or(false);
    let mut diag_writer = diag_on.then(diag::Writer::open);
    let mut last_diag: Option<Instant> = None;
    let mut diag_first = true;
    loop {
        let tick = Instant::now();
        // Start-up delay: the root pointer must be non-null and unchanged for 2 s before any call.
        let mgr = match engine.mgr() {
            Some(m) => {
                match stable {
                    Some((s, _)) if s == m => {}
                    _ => stable = Some((m, Instant::now())),
                }
                m
            }
            None => {
                stable = None;
                0
            }
        };
        let ready = matches!(stable, Some((_, t)) if t.elapsed() >= Duration::from_secs(2));
        if ready {
            if let Some(n) = engine.deck_count(mgr).filter(|&n| n > 0) {
                if !announced {
                    log::line(&format!("engine up: mgr {mgr:#x}, {n} decks"));
                    announced = true;
                }
                let qpc_start = qpc_100ns();
                let want_diag = diag_on && last_diag.map_or(true, |t| t.elapsed() >= Duration::from_millis(200));
                let (decks, regions, ptr_log) = match guarded_read(&engine, mgr, n, want_diag, diag_first) {
                    Ok(d) => d,
                    Err((code, addr)) => {
                        let off = addr.wrapping_sub(base as u64);
                        log::line(&format!("exception {code:#x} at {addr:#x} (module+{off:#x}) during read, exporter disabled, rekordbox keeps running"));
                        return;
                    }
                };
                let qpc_end = qpc_100ns();
                if want_diag {
                    last_diag = Some(Instant::now());
                    if diag_first {
                        log::line(&format!("diag pointers: {ptr_log}"));
                        diag_first = false;
                    }
                    if let Some(w) = diag_writer.as_mut() {
                        w.append(&diag::encode(qpc_start, n, &regions));
                    }
                }
                let msg = format!(
                    "{{\"v\":1,\"t_ms\":{},\"qpc_100ns\":{},\"read_100ns\":{},\"decks\":[{}]}}",
                    started.elapsed().as_millis(),
                    qpc_start,
                    qpc_end - qpc_start,
                    decks.join(",")
                );
                let _ = sock.send_to(msg.as_bytes(), &target);
            }
        }
        if let Some(rest) = period.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a fake FormattedBeatGrid at 120 BPM (500 ms per beat), 4/4, first downbeat at 100 ms.
    fn fake_grid(beats: usize) -> (Vec<u8>, Vec<u8>, usize) {
        fake_grid_based(beats, 1)
    }

    fn fake_grid_based(beats: usize, first: u8) -> (Vec<u8>, Vec<u8>, usize) {
        let mut entries = vec![0u8; beats * GRID_ENTRY];
        for i in 0..beats {
            let o = i * GRID_ENTRY;
            entries[o..o + 4].copy_from_slice(&120.0f32.to_le_bytes());
            entries[o + OFF_ENTRY_BEAT_IN_BAR] = (i % 4) as u8 + first;
            entries[o + 8..o + 16].copy_from_slice(&(100.0 + 500.0 * i as f64).to_le_bytes());
        }
        let fake_vtbl = 0x7_0000_0000usize;
        let base = fake_vtbl - (VA_FORMATTED_BEAT_GRID_VTBL - crate::guard::PREFERRED_BASE);
        let mut obj = vec![0u8; 0x30];
        obj[0..8].copy_from_slice(&fake_vtbl.to_le_bytes());
        obj[OFF_GRID_MAGIC..OFF_GRID_MAGIC + 4].copy_from_slice(&GRID_MAGIC.to_le_bytes());
        let b = entries.as_ptr() as usize;
        obj[OFF_GRID_BEGIN..OFF_GRID_BEGIN + 8].copy_from_slice(&b.to_le_bytes());
        obj[OFF_GRID_END..OFF_GRID_END + 8].copy_from_slice(&(b + entries.len()).to_le_bytes());
        (obj, entries, base)
    }

    #[test]
    fn beat_bar_phase_from_grid() {
        let (obj, _entries, base) = fake_grid(64);
        let e = Engine { base };
        let g = obj.as_ptr() as usize;
        let b = e.read_grid(g, 100.0).unwrap();
        assert_eq!((b.index, b.bar, b.beat_in_bar), (0, 1, 1));
        assert!(b.phase_beat.abs() < 1e-9);
        let b = e.read_grid(g, 100.0 + 500.0 * 5.0 + 250.0).unwrap();
        assert_eq!((b.index, b.bar, b.beat_in_bar), (5, 2, 2));
        assert!((b.phase_beat - 0.5).abs() < 1e-9);
        assert!((b.phase_bar - (1.0 + 0.5) / 4.0).abs() < 1e-9);
        assert!((b.grid_bpm - 120.0).abs() < 1e-6);
        let b = e.read_grid(g, 50.0).unwrap();
        assert_eq!(b.index, 0);
        assert_eq!(b.phase_beat, 0.0);
        let b = e.read_grid(g, 100.0 + 500.0 * 63.0 + 125.0).unwrap();
        assert_eq!(b.index, 63);
        assert!((b.phase_beat - 0.25).abs() < 1e-9);
    }

    #[test]
    fn seh_contains_access_violation() {
        unsafe extern "C" fn bad(_: *mut c_void) {
            std::ptr::read_volatile(0x2815a0 as *const u32);
        }
        let mut f = 0u64;
        let code = unsafe { rbmod_seh_call(bad, std::ptr::null_mut(), &mut f) };
        assert_eq!(code, 0xC000_0005);
    }

    #[test]
    fn zero_based_grid_as_seen_live() {
        let (obj, _entries, base) = fake_grid_based(64, 0);
        let e = Engine { base };
        let g = obj.as_ptr() as usize;
        let b = e.read_grid(g, 100.0).unwrap();
        assert_eq!((b.index, b.bar, b.beat_in_bar), (0, 1, 1));
        let b = e.read_grid(g, 100.0 + 500.0 * 7.0 + 400.0).unwrap();
        assert_eq!((b.index, b.bar, b.beat_in_bar), (7, 2, 4));
        assert!((b.phase_bar - (3.0 + 0.8) / 4.0).abs() < 1e-9);
        assert!(b.phase_bar >= 0.0 && b.phase_bar < 1.0);
    }

    #[test]
    fn flags_follow_vf12() {
        assert_eq!(deck_flags(false, false, false, false, -1), 0);
        assert_eq!(deck_flags(false, true, false, false, -1), 1);
        assert_eq!(deck_flags(true, false, false, false, -1), 0x10);
        assert_eq!(deck_flags(true, false, true, true, -1), 0x1110);
        assert_eq!(deck_flags(true, false, false, true, 3), 0x1010);
        assert_eq!(deck_flags(true, false, true, false, 3), 0x10110);
        assert_eq!(deck_flags(true, false, true, false, -1), 0x0000_0110);
    }

    #[test]
    fn master_from_sync_manager() {
        let mut mgr = vec![0u64; 0x40];
        let mut a = vec![0u64; 0x10];
        let mut b = vec![0u64; 0x10];
        let (pa, pb, pm) = (a.as_mut_ptr() as usize, b.as_mut_ptr() as usize, mgr.as_mut_ptr() as usize);
        mgr[OFF_MANAGER_MASTER / 8] = pb as u64;
        a[OFF_PART_MANAGER / 8] = pm as u64;
        let da = Deck { sync_obj: pa, sync_mgr: pm, ..Default::default() };
        let db = Deck { sync_obj: pb, sync_mgr: 0, ..Default::default() };
        assert_eq!(sync_master(&da, 0), (false, "syncmgr+0xf0"));
        assert_eq!(sync_master(&db, pm), (true, "syncmgr+0xf0"));
        let dn = Deck { p18: Some(1), ..Default::default() };
        assert_eq!(sync_master(&dn, 0), (true, "participant+0x18"));
    }

    fn dispatch_deck(follow: usize) -> Deck {
        Deck { loaded: true, follow_master: Some(follow), master: follow == 0, master_source: "dispatch+0x248", ..Default::default() }
    }

    #[test]
    fn master_unique_rule() {
        let mut d = vec![dispatch_deck(0), dispatch_deck(0x1234)];
        resolve_master(&mut d);
        assert!(d[0].master && !d[1].master);
        let mut d = vec![dispatch_deck(0), dispatch_deck(0)];
        resolve_master(&mut d);
        assert!(!d[0].master && !d[1].master);
        assert_eq!(d[0].master_source, "no_master_set");
        let mut d = vec![dispatch_deck(0), Deck::default()];
        resolve_master(&mut d);
        assert!(!d[0].master);
        assert_eq!(d[0].master_source, "single_deck_unknown");
    }

    #[test]
    fn rejects_foreign_grid() {
        let (mut obj, _entries, base) = fake_grid(8);
        obj[OFF_GRID_MAGIC] ^= 0xff;
        assert!(Engine { base }.read_grid(obj.as_ptr() as usize, 300.0).is_none());
    }
}
