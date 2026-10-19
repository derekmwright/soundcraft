//! `Vst3Plugin`: a loaded VST3 instance behind the safe `soundcraft_dsp::Plugin` trait.

use crate::ffi::{AFTERTOUCH, BusBuffers, EVENT_CAP, EditorLink, HostContext, Instance, NoteEvent, PARAM_QUEUE_CAP, PITCH_BEND};
use crate::params::{ParamCurves, PendingParams};
use soundcraft_dsp::{Plugin, PluginEditor, PluginInfo, ProcessStatus, Transport};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use vst3::ComWrapper;

/// Largest block we set a plugin up for.
const MAX_BLOCK: usize = 1 << 16;
/// Most channels a host strip may ask for.
const MAX_CHANNELS: usize = 64;
/// An `unsynced` slot with no value waiting (a NaN pattern, which `set_param` never stores).
const SYNCED: u64 = u64::MAX;
/// Most curves one editor `idle` samples, and the time after which it stops (it always samples
/// one when any is due).
const CURVE_BUDGET_TABLES: usize = 64;
const CURVE_BUDGET_TIME: Duration = Duration::from_millis(2);

/// How a SoundCraft parameter value maps to the VST3 normalized value.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mapping {
    /// Any stepped parameter (`stepCount > 0`, however many steps): step index `0..=steps` ↔
    /// `index / steps`, exactly.
    Stepped(f64),
    /// Plain value through the controller's curve, sampled on the UI thread ([`ParamCurves`]).
    Plain,
    /// Linear over the info range (the controller gave no usable plain range).
    Linear,
}

/// The status a VST3 plugin implies through its tail: an infinite tail (`kInfiniteTail`) must
/// keep processing; a finite one (including none) may stop that many samples after the input
/// goes silent.
fn status_from_tail(tail: u32) -> ProcessStatus {
    if tail == u32::MAX { ProcessStatus::Continue } else { ProcessStatus::Tail }
}

/// A hosted VST3 plugin. Parameter ids are VST3 `ParamID`s as decimal strings ("0", "17", …);
/// values are plain (display) values with their names and ranges in `info().params`.
pub struct Vst3Plugin {
    inst: Instance,
    info: &'static PluginInfo,
    /// VST3 id of each `info.params` entry (same order).
    ids: Vec<u32>,
    maps: Vec<Mapping>,
    values: Vec<f32>,
    /// Which values the host set since creation or the last state load (same order as `ids`):
    /// only these are re-sent on activation; values read from the plugin never are. `unsynced`
    /// cannot tell, since the state reader clears it once the controller has a value.
    host_set: Vec<bool>,
    /// The controller's curves of the `Mapping::Plain` parameters, shared with the editor handle,
    /// which re-samples them on the UI thread when the plugin asks for a restart.
    curves: Arc<ParamCurves>,
    /// The curves' generation the queued values were converted with.
    curves_seen: u64,
    /// Normalized values waiting for the next block (latest per parameter; never drops).
    pending: PendingParams,
    /// One block's share of `pending`, handed to the processor (capacity `PARAM_QUEUE_CAP`).
    batch: Vec<(u32, f64)>,
    /// Host-set plain values the controller has not seen yet (same order as `ids`; the bits
    /// of an `f64`, or [`SYNCED`]). `set_param` may run on the audio thread, so it never calls the
    /// controller; the state reader, which already does, hands them over first.
    unsynced: Arc<[AtomicU64]>,
    /// Pending notes, kept sorted by time. Never grows past `EVENT_CAP`.
    notes: Vec<NoteEvent>,
    /// The parameter each MIDI controller drives (see `Instance::midi_mapping`).
    midi_map: Vec<Option<u32>>,
    /// Controller points for the next block: (time, param id, normalized value). Bounded.
    controls: Vec<(u32, u32, f64)>,
    inputs: BusBuffers,
    outputs: BusBuffers,
    has_input: bool,
    has_output: bool,
    event_input: bool,
    held: [bool; 128],
    channels: usize,
    max_block: usize,
    latency: usize,
    tail: usize,
    position: i64,
    transport: Option<Transport>,
}

impl Vst3Plugin {
    pub(crate) fn new(inst: Instance, info: &'static PluginInfo) -> Vst3Plugin {
        let ids: Vec<u32> = info.params.iter().map(|p| p.id.parse::<u32>().unwrap_or(u32::MAX)).collect();
        // Any stepped parameter (stepCount > 0) converts exactly, however many steps it has; the
        // registry's MAX_CHOICES only decides whether the UI gets a choice list.
        let steps: std::collections::HashMap<u32, i32> =
            inst.params().into_iter().filter(|r| r.step_count > 0).map(|r| (r.id, r.step_count)).collect();
        let maps: Vec<Mapping> = info
            .params
            .iter()
            .zip(&ids)
            .map(|(p, &id)| {
                if let Some(&n) = steps.get(&id) {
                    return Mapping::Stepped(f64::from(n));
                }
                match (inst.to_plain(id, 0.0), inst.to_plain(id, 1.0)) {
                    (Some(lo), Some(hi))
                        if (lo as f32 - p.min).abs() <= 1e-6 * p.min.abs().max(1.0) && (hi as f32 - p.max).abs() <= 1e-6 * p.max.abs().max(1.0) =>
                    {
                        Mapping::Plain
                    }
                    _ => Mapping::Linear,
                }
            })
            .collect();
        // Creation runs on the UI (control) thread. No curve is sampled here (a plug-in may have
        // thousands of parameters): the editor handle's idle samples them lazily and on demand.
        let plain: Vec<bool> = maps.iter().map(|m| *m == Mapping::Plain).collect();
        let curves = Arc::new(ParamCurves::new(&ids, &plain));
        let mut p = Vst3Plugin {
            info,
            values: Vec::with_capacity(ids.len()),
            host_set: vec![false; ids.len()],
            curves_seen: curves.generation(),
            curves,
            pending: PendingParams::new(ids.len()),
            batch: Vec::with_capacity(PARAM_QUEUE_CAP),
            unsynced: ids.iter().map(|_| AtomicU64::new(SYNCED)).collect(),
            notes: Vec::with_capacity(EVENT_CAP),
            midi_map: Vec::new(),
            controls: Vec::with_capacity(EVENT_CAP),
            inputs: BusBuffers::new(&[], 0),
            outputs: BusBuffers::new(&[], 0),
            has_input: false,
            has_output: false,
            event_input: false,
            held: [false; 128],
            channels: 2,
            max_block: 0,
            latency: 0,
            tail: 0,
            position: 0,
            transport: None,
            ids,
            maps,
            inst,
        };
        let values: Vec<f32> = (0..p.ids.len())
            .map(|i| {
                let (Some(pi), Some(&id)) = (p.info.params.get(i), p.ids.get(i)) else { return 0.0 };
                p.inst.normalized_value(id).and_then(|n| p.plain_via_controller(i, n)).map_or(pi.default, |v| pi.clamp(v))
            })
            .collect();
        p.values = values;
        p
    }

    /// Normalized → plain through the controller (UI thread only: creation and state loads; the
    /// audio thread never converts this way, it only needs plain → normalized),
    /// so a value read from the plug-in is exact before its curve is sampled.
    fn plain_via_controller(&self, i: usize, n: f64) -> Option<f32> {
        let pi = self.info.params.get(i)?;
        let v = match self.maps.get(i)? {
            Mapping::Stepped(steps) => (n * steps).round(),
            Mapping::Plain => self.inst.to_plain(*self.ids.get(i)?, n).unwrap_or(f64::from(pi.min) + n * f64::from(pi.max - pi.min)),
            Mapping::Linear => f64::from(pi.min) + n * f64::from(pi.max - pi.min),
        };
        v.is_finite().then_some(v as f32)
    }

    /// How many plain-mapped parameters have their curve sampled so far (see [`ParamCurves`]).
    pub fn curves_sampled(&self) -> usize {
        self.curves.curves_sampled()
    }

    /// How many plain-mapped parameters there are to sample.
    pub fn curves_total(&self) -> usize {
        self.curves.curves_total()
    }

    /// Samples every unsampled or invalidated curve now, on the calling thread (no budget).
    ///
    /// For renders that never run an editor handle's `idle` (bounces, freezes, CLI renders): without
    /// it plain-mapped parameters convert linearly until `idle` gets to them. Calls the edit
    /// controller (257 times per plain-mapped parameter), so it must run on the thread that may
    /// use the controller (the UI/main thread, or the thread creating the instance), never on a
    /// thread that is rendering this instance. Not real-time. Hosts that render offline get it for
    /// free on every instance created inside an [`crate::offline_scope`].
    pub fn sample_all_curves_now(&self) {
        self.curves.step(usize::MAX, Duration::MAX, |id, n| self.inst.to_plain(id, n));
    }

    /// Plain → normalized without calling the controller (safe on the audio thread).
    fn normalized_from_plain(&self, i: usize, v: f32) -> Option<f64> {
        let pi = self.info.params.get(i)?;
        let span = f64::from(pi.max - pi.min);
        let linear = if span > 0.0 { (f64::from(v) - f64::from(pi.min)) / span } else { 0.0 };
        let n = match self.maps.get(i)? {
            Mapping::Stepped(steps) if *steps > 0.0 => f64::from(v) / steps,
            Mapping::Stepped(_) => 0.0,
            Mapping::Plain => self.curves.to_normalized(i, f64::from(v)).unwrap_or(linear),
            Mapping::Linear => linear,
        };
        n.is_finite().then(|| n.clamp(0.0, 1.0))
    }

    fn converter(&self, link: Arc<crate::ffi::EditorLink>) -> Converter {
        Converter { link, info: self.info, ids: self.ids.clone(), maps: self.maps.clone() }
    }

    /// Converts the host-set plain value of parameter `i` and queues it for the next block (and
    /// for the controller before the next state save).
    fn queue(&mut self, i: usize) {
        let Some(&v) = self.values.get(i) else { return };
        let Some(n) = self.normalized_from_plain(i, v) else { return };
        if let Some(u) = self.unsynced.get(i) {
            u.store(f64::from(v).to_bits(), Ordering::Relaxed);
        }
        self.pending.set(i, n);
    }

    /// The editor's size in pixels without opening it (creates and releases a view). For
    /// diagnostics and tests; real plugins expect this on the main thread.
    pub fn editor_size(&self) -> Result<(u32, u32), String> {
        self.inst.editor().ok_or_else(|| "this plugin has no edit controller".to_string())?.probe()
    }

    /// Re-reads every parameter value from the controller (after a state load) and drops
    /// changes still queued from before it.
    fn refresh_values(&mut self) {
        for i in 0..self.ids.len() {
            let (Some(pi), Some(&id)) = (self.info.params.get(i), self.ids.get(i)) else { continue };
            if let Some(v) = self.inst.normalized_value(id).and_then(|n| self.plain_via_controller(i, n))
                && let Some(slot) = self.values.get_mut(i)
            {
                *slot = pi.clamp(v);
            }
        }
        self.pending.clear();
        for u in self.unsynced.iter() {
            u.store(SYNCED, Ordering::Relaxed);
        }
        self.host_set.fill(false);
    }

    /// Inserts keeping the queue time-sorted; drops the event when the queue is full (never
    /// allocates on the audio thread).
    fn push_note(&mut self, e: NoteEvent) {
        if self.notes.len() >= self.notes.capacity() {
            return;
        }
        let pos = self.notes.iter().rposition(|q| q.time() <= e.time()).map_or(0, |i| i + 1);
        self.notes.insert(pos, e);
    }
}

impl Plugin for Vst3Plugin {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        let mb = max_block.clamp(1, MAX_BLOCK);
        self.channels = channels.clamp(1, MAX_CHANNELS);
        self.max_block = 0;
        match self.inst.configure(sr, mb, self.channels == 1) {
            Ok(layout) => {
                self.inputs = BusBuffers::new(&layout.inputs, mb);
                self.outputs = BusBuffers::new(&layout.outputs, mb);
                self.has_input = layout.inputs.first().is_some_and(|&c| c > 0);
                self.has_output = layout.outputs.first().is_some_and(|&c| c > 0);
                self.event_input = layout.event_input;
                self.max_block = mb;
                self.latency = self.inst.latency() as usize;
                self.tail = self.inst.tail() as usize;
                self.midi_map = self.inst.midi_mapping();
            }
            Err(e) => {
                log::warn!("{}: {e}", self.info.id);
                self.latency = 0;
            }
        }
        // Re-send the values the host set, so a re-activated plugin matches the session. Values only
        // read from the plugin are left alone: some plugins report 0 for everything until they load
        // their own preset (Rhapsody), and sending those readings back set its volume and pan to
        // the minimum, silencing it.
        self.pending.clear();
        for i in resend_indices(&self.host_set, self.ids.len()).collect::<Vec<_>>() {
            self.queue(i);
        }
    }

    fn reset(&mut self) {
        keep_releases_on_reset(&mut self.notes, &mut self.held);
        self.controls.clear();
    }

    /// Runs on the audio thread (automation): converts through the sampled curves, never the
    /// controller, and queues without allocating.
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = self.info.params.iter().position(|p| p.id == id) else { return false };
        let Some(pi) = self.info.params.get(i) else { return false };
        let v = pi.clamp(value);
        if let Some(slot) = self.values.get_mut(i) {
            *slot = v;
        }
        if let Some(h) = self.host_set.get_mut(i) {
            *h = true;
        }
        self.queue(i);
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        let i = self.info.params.iter().position(|p| p.id == id)?;
        self.values.get(i).copied()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if !self.inst.is_active() || self.max_block == 0 {
            return;
        }
        let ch = self.channels.min(io.len());
        let frames = io.iter().take(ch).map(Vec::len).fold(frames, usize::min);
        if frames == 0 || ch == 0 {
            return;
        }
        if self.inst.host().latency_changed.swap(false, Ordering::Relaxed) {
            self.latency = self.inst.latency() as usize;
        }
        // The UI thread re-sampled the curves: the host's plain values may now map elsewhere.
        let generation = self.curves.generation();
        if generation != self.curves_seen {
            self.curves_seen = generation;
            for i in 0..self.ids.len() {
                if self.host_set.get(i).copied().unwrap_or(false) {
                    self.queue(i);
                }
            }
        }
        let last = u32::try_from(frames - 1).unwrap_or(u32::MAX);
        for e in &mut self.notes {
            if e.time() > last {
                e.set_time(last);
            }
        }
        for c in &mut self.controls {
            c.0 = c.0.min(last);
        }
        let mut start = 0usize;
        while start < frames {
            let n = (frames - start).min(self.max_block);
            let end = u32::try_from(start + n).unwrap_or(u32::MAX);
            // Feed the main input bus (mono sources fill every bus channel).
            if self.has_input {
                for c in 0..self.inputs.channels(0) {
                    let src = io.get(c.min(ch - 1)).and_then(|s| s.get(start..start + n));
                    if let (Some(src), Some(dst)) = (src, self.inputs.channel_mut(0, c).and_then(|d| d.get_mut(..n))) {
                        dst.copy_from_slice(src);
                    }
                }
            }
            let k = self.notes.iter().position(|e| e.time() >= end).unwrap_or(self.notes.len());
            let offset = u32::try_from(start).unwrap_or(u32::MAX);
            let notes = self.notes.get(..k).unwrap_or(&[]);
            // Leave room in the pool for this block's controller points; the rest waits.
            self.batch.clear();
            self.pending.take_into(&self.ids, &mut self.batch, PARAM_QUEUE_CAP.saturating_sub(self.controls.len()));
            let r = self.inst.process(
                n,
                &mut self.inputs,
                &mut self.outputs,
                &self.batch,
                &self.controls,
                notes,
                offset,
                self.position,
                self.transport.as_ref(),
            );
            self.batch.clear();
            self.notes.drain(..k);
            self.position = self.position.saturating_add(i64::try_from(n).unwrap_or(0));
            match r {
                Ok(()) if self.has_output => {
                    let oc = self.outputs.channels(0);
                    for (c, dst) in io.iter_mut().take(ch).enumerate() {
                        let Some(dst) = dst.get_mut(start..start + n) else { continue };
                        if ch == 1 && oc >= 2 {
                            if let (Some(l), Some(r)) = (self.outputs.channel(0, 0), self.outputs.channel(0, 1)) {
                                for ((d, a), b) in dst.iter_mut().zip(l).zip(r) {
                                    *d = 0.5 * (a + b);
                                }
                            }
                        } else if let Some(src) = self.outputs.channel(0, c.min(oc.saturating_sub(1))).and_then(|s| s.get(..n)) {
                            dst.copy_from_slice(src);
                        }
                    }
                }
                Err(e) => log::debug!("{}: {e}", self.info.id),
                _ => {}
            }
            start += n;
        }
        self.controls.clear();
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.transport = Some(*transport);
    }

    fn tail_samples(&self) -> usize {
        self.tail
    }

    fn process_status(&self) -> ProcessStatus {
        status_from_tail(u32::try_from(self.tail).unwrap_or(u32::MAX))
    }

    fn set_input_silent(&mut self, silent: bool) {
        self.inputs.set_silent(silent);
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        if note > 127 || !self.event_input {
            return;
        }
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        self.push_note(NoteEvent::On { time, key: note, velocity: f32::from(velocity.min(127)) / 127.0 });
        if let Some(h) = self.held.get_mut(note as usize) {
            *h = true;
        }
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        if note > 127 || !self.event_input {
            return;
        }
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        self.push_note(NoteEvent::Off { time, key: note });
        if let Some(h) = self.held.get_mut(note as usize) {
            *h = false;
        }
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        let time = u32::try_from(offset).unwrap_or(u32::MAX);
        let [_, d1, d2] = msg.map(|b| b & 0x7F);
        let (ctrl, value) = match msg[0] & 0xF0 {
            0xB0 => (usize::from(d1), f64::from(d2) / 127.0),
            0xD0 => (AFTERTOUCH, f64::from(d1) / 127.0),
            0xE0 => (PITCH_BEND, f64::from(u16::from(d2) << 7 | u16::from(d1)) / 16383.0),
            0xA0 if self.event_input => {
                self.push_note(NoteEvent::Pressure { time, key: d1, pressure: f32::from(d2) / 127.0 });
                return;
            }
            _ => return,
        };
        if let Some(&Some(id)) = self.midi_map.get(ctrl)
            && self.controls.len() < self.controls.capacity()
        {
            self.controls.push((time, id, value));
        }
    }

    fn all_notes_off(&mut self) {
        for k in 0..128u8 {
            if self.held.get(k as usize).copied().unwrap_or(false) {
                self.note_off(0, k);
            }
        }
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        let ok = self.inst.load_state(data);
        if ok {
            self.refresh_values();
        }
        ok
    }

    fn open_editor(&mut self) -> Result<(), String> {
        self.inst.editor().ok_or_else(|| "this plugin has no editor".to_string())?.open(self.info.name)
    }

    fn close_editor(&mut self) {
        if let Some(e) = self.inst.editor() {
            e.close();
        }
    }

    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        let link = self.inst.editor()?;
        Some(Box::new(Vst3Editor { conv: self.converter(link), host: self.inst.host_ref(), curves: Arc::clone(&self.curves) }))
    }

    fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
        let conv = self.converter(self.inst.state_link());
        let unsynced = Arc::clone(&self.unsynced);
        Some(Box::new(move || {
            // A parameter only the controller stores (no processor state) would otherwise be
            // saved with the controller's stale value. The host's plain value converts through
            // the controller itself here (this thread may call it), so it is exact even for a
            // curve nobody sampled yet.
            for (i, u) in unsynced.iter().enumerate() {
                let bits = u.swap(SYNCED, Ordering::Relaxed);
                if bits != SYNCED
                    && let (Some(&id), Some(n)) = (conv.ids.get(i), conv.normalized(i, f64::from_bits(bits) as f32))
                {
                    conv.link.sync_param(id, n);
                }
            }
            conv.link.save_state()
        }))
    }
}

/// Plain ↔ normalized through the edit controller: UI thread only (the editor handle and the
/// state reader).
#[derive(Clone)]
struct Converter {
    link: Arc<EditorLink>,
    info: &'static PluginInfo,
    ids: Vec<u32>,
    maps: Vec<Mapping>,
}

impl Converter {
    fn plain(&self, i: usize, n: f64) -> Option<f32> {
        let pi = self.info.params.get(i)?;
        let id = *self.ids.get(i)?;
        let v = match self.maps.get(i)? {
            Mapping::Stepped(steps) => (n * steps).round(),
            Mapping::Plain => self.link.to_plain(id, n)?,
            Mapping::Linear => f64::from(pi.min) + n * f64::from(pi.max - pi.min),
        };
        v.is_finite().then(|| pi.clamp(v as f32))
    }

    fn normalized(&self, i: usize, v: f32) -> Option<f64> {
        let pi = self.info.params.get(i)?;
        let id = *self.ids.get(i)?;
        let span = f64::from(pi.max - pi.min);
        let linear = if span > 0.0 { (f64::from(v) - f64::from(pi.min)) / span } else { 0.0 };
        let n = match self.maps.get(i)? {
            Mapping::Stepped(steps) if *steps > 0.0 => f64::from(v) / steps,
            Mapping::Stepped(_) => 0.0,
            Mapping::Plain => self.link.to_normalized(id, f64::from(v)).unwrap_or(linear),
            Mapping::Linear => linear,
        };
        n.is_finite().then(|| n.clamp(0.0, 1.0))
    }
}

/// A VST3 plugin's editor (`IPlugView` in a host window), driven from the main thread.
struct Vst3Editor {
    conv: Converter,
    host: ComWrapper<HostContext>,
    /// The instance's sampled curves, sampled lazily here (UI thread) and re-sampled on a title
    /// restart.
    curves: Arc<ParamCurves>,
}

impl PluginEditor for Vst3Editor {
    fn open(&mut self) -> Result<(), String> {
        // Edits reported before the editor opened (e.g. during setup) are stale.
        let _ = self.host.take_edits();
        self.conv.link.open(self.conv.info.name)
    }

    fn close(&mut self) {
        self.conv.link.close();
    }

    fn is_open(&self) -> bool {
        self.conv.link.is_open()
    }

    /// Also the UI-thread hook for the lazy curve sampling (at most `CURVE_BUDGET_TABLES` curves or
    /// `CURVE_BUDGET_TIME` per call) and for `restartComponent(kParamTitlesChanged)`:
    /// the host polls every editor handle each UI frame, open or not.
    fn idle(&mut self) -> Vec<(String, f32)> {
        self.conv.link.idle();
        // Only a title change can reshape a curve; a values change leaves them (many plug-ins send
        // it on every preset load) and is recorded for re-reading the values (audit F08).
        if self.host.titles_changed.swap(false, Ordering::Relaxed) {
            self.curves.invalidate();
        }
        let _ = self.host.values_changed.swap(false, Ordering::Relaxed);
        self.curves.step(CURVE_BUDGET_TABLES, CURVE_BUDGET_TIME, |id, n| self.conv.link.to_plain(id, n));
        let mut out = Vec::new();
        for (vid, n) in self.host.take_edits() {
            let Some(i) = self.conv.ids.iter().position(|&x| x == vid) else { continue };
            if let (Some(p), Some(v)) = (self.conv.info.params.get(i), self.conv.plain(i, n)) {
                out.push((p.id.to_string(), v));
            }
        }
        out
    }

    fn set_param(&mut self, id: &str, value: f32) {
        let Some(i) = self.conv.info.params.iter().position(|p| p.id == id) else { return };
        if let (Some(&vid), Some(n)) = (self.conv.ids.get(i), self.conv.normalized(i, value)) {
            self.conv.link.set_normalized(vid, n);
        }
    }
}

/// Indices of the values to re-send when the plugin is (re)activated: those the host set, at most
/// `cap` of them.
fn resend_indices(host_set: &[bool], cap: usize) -> impl Iterator<Item = usize> + '_ {
    host_set.iter().enumerate().filter(|(_, set)| **set).map(|(i, _)| i).take(cap)
}

/// Reset's share of the note queue: drop everything pending except the releases the plug-in is
/// owed. Each held key gets an off at time 0 and any off already queued survives, so a note that
/// was sounding when the transport stopped is still released on the next block.
fn keep_releases_on_reset(notes: &mut Vec<NoteEvent>, held: &mut [bool; 128]) {
    notes.retain(|e| matches!(e, NoteEvent::Off { .. }));
    for (key, h) in (0u8..).zip(held.iter_mut()) {
        if std::mem::take(h) && notes.len() < notes.capacity() {
            notes.insert(0, NoteEvent::Off { time: 0, key });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_values_the_host_set_are_re_sent() {
        assert_eq!(resend_indices(&[false, false, false], 8).count(), 0, "readings from the plugin stay put");
        assert_eq!(resend_indices(&[false, true, false, true], 8).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(resend_indices(&[true, true, true], 2).collect::<Vec<_>>(), vec![0, 1], "bounded by the queue");
    }

    #[test]
    fn process_status_follows_the_tail() {
        assert_eq!(status_from_tail(u32::MAX), ProcessStatus::Continue);
        assert_eq!(status_from_tail(0), ProcessStatus::Tail);
        assert_eq!(status_from_tail(48_000), ProcessStatus::Tail);
    }

    #[test]
    fn reset_keeps_the_releases_it_owes() {
        let mut notes = Vec::with_capacity(EVENT_CAP);
        notes.push(NoteEvent::On { time: 3, key: 72, velocity: 1.0 });
        notes.push(NoteEvent::Off { time: 5, key: 67 });
        let mut held = [false; 128];
        held[60] = true;
        held[64] = true;
        keep_releases_on_reset(&mut notes, &mut held);
        let mut keys: Vec<u8> = notes.iter().filter_map(|e| if let NoteEvent::Off { key, .. } = e { Some(*key) } else { None }).collect();
        assert_eq!(keys.len(), notes.len(), "reset left a non-release queued");
        keys.sort_unstable();
        assert_eq!(keys, [60, 64, 67], "reset must queue an off for every held or already-released key");
        assert!(held.iter().all(|h| !h), "reset must clear held");
    }
}
