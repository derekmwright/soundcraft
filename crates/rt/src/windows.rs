//! Windows: MMCSS "Pro Audio" at critical priority, else `THREAD_PRIORITY_TIME_CRITICAL`.
//!
//! Tested on Windows 11 (the crate's tests run here).

use crate::{Level, Promotion};
use std::cell::Cell;
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::{
    AVRT_PRIORITY_CRITICAL, AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, AvSetMmThreadPriority, GetCurrentThread,
    GetThreadPriority, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};

/// The MMCSS task name "Pro Audio", NUL-terminated UTF-16 (the task is defined in the registry
/// under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks`).
const PRO_AUDIO: [u16; 10] = [0x50, 0x72, 0x6f, 0x20, 0x41, 0x75, 0x64, 0x69, 0x6f, 0];

/// `GetThreadPriority`'s failure value (`MAXLONG`); `windows-sys` does not export it.
const THREAD_PRIORITY_ERROR_RETURN: i32 = 0x7fff_ffff;

/// What this crate changed on the calling thread, so `demote` can undo exactly that.
#[derive(Clone, Copy)]
struct Saved {
    /// The successful promotion, returned again by a repeated `promote`.
    promotion: Option<Promotion>,
    /// The MMCSS handle from `AvSetMmThreadCharacteristicsW`, null when not registered.
    mmcss: HANDLE,
    /// The thread priority before `SetThreadPriority`, when that path was taken.
    previous_priority: Option<i32>,
}

const NOTHING: Saved = Saved { promotion: None, mmcss: std::ptr::null_mut(), previous_priority: None };

std::thread_local! {
    static SAVED: Cell<Saved> = const { Cell::new(NOTHING) };
}

fn last_error() -> i32 {
    // SAFETY: `GetLastError` reads the calling thread's last-error value; no arguments, no
    // preconditions.
    let e = unsafe { GetLastError() };
    i32::try_from(e).unwrap_or(i32::MAX)
}

pub(crate) fn promote() -> Promotion {
    let saved = SAVED.get();
    if let Some(p) = saved.promotion {
        return p;
    }
    let mut task_index = 0u32;
    // SAFETY: `PRO_AUDIO` is a NUL-terminated UTF-16 string in static memory, valid for the call;
    // `task_index` is a live, writable `u32` the call fills in. The function has no other
    // preconditions; it returns null on failure, which is checked below.
    let handle = unsafe { AvSetMmThreadCharacteristicsW(PRO_AUDIO.as_ptr(), &raw mut task_index) };
    if !handle.is_null() {
        // Critical is best effort: the thread is in the Pro Audio task either way, at the task's
        // default priority when this fails.
        // SAFETY: `handle` is the non-null MMCSS handle just returned for this thread, not yet
        // reverted.
        let critical = unsafe { AvSetMmThreadPriority(handle, AVRT_PRIORITY_CRITICAL) } != 0;
        let p = Promotion::Mmcss { task_index, critical };
        SAVED.set(Saved { promotion: Some(p), mmcss: handle, previous_priority: None });
        return p;
    }
    // MMCSS unavailable (service stopped, or a session without it): raise the plain priority.
    // SAFETY: `GetCurrentThread` returns a pseudo-handle for the calling thread; it cannot fail
    // and needs no closing.
    let me = unsafe { GetCurrentThread() };
    // SAFETY: `me` is the calling thread's pseudo-handle, which has query rights.
    let previous = unsafe { GetThreadPriority(me) };
    if previous == THREAD_PRIORITY_ERROR_RETURN {
        return Promotion::Denied(last_error());
    }
    // SAFETY: `me` is the calling thread's pseudo-handle, which has set rights; the level is a
    // documented `THREAD_PRIORITY_*` constant.
    if unsafe { SetThreadPriority(me, THREAD_PRIORITY_TIME_CRITICAL) } == 0 {
        return Promotion::Denied(last_error());
    }
    let p = Promotion::Priority(THREAD_PRIORITY_TIME_CRITICAL);
    SAVED.set(Saved { promotion: Some(p), mmcss: std::ptr::null_mut(), previous_priority: Some(previous) });
    p
}

pub(crate) fn demote() {
    let saved = SAVED.replace(NOTHING);
    if !saved.mmcss.is_null() {
        // SAFETY: `saved.mmcss` is the handle `AvSetMmThreadCharacteristicsW` returned on this
        // thread (thread-local state), and it is reverted exactly once: the state was cleared
        // above.
        let _ = unsafe { AvRevertMmThreadCharacteristics(saved.mmcss) };
    }
    if let Some(previous) = saved.previous_priority {
        // SAFETY: as in `promote`: the calling thread's pseudo-handle and a priority value
        // `GetThreadPriority` returned for this thread.
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), previous) };
    }
}

pub(crate) fn current_level() -> Option<Level> {
    // SAFETY: the calling thread's pseudo-handle, as in `promote`.
    let priority = unsafe { GetThreadPriority(GetCurrentThread()) };
    if priority == THREAD_PRIORITY_ERROR_RETURN {
        return None;
    }
    let mmcss = !SAVED.get().mmcss.is_null();
    Some(Level { priority, realtime: mmcss || priority >= THREAD_PRIORITY_TIME_CRITICAL })
}
