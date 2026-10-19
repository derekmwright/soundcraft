//! Parameter plumbing between the host's plain values and VST3's normalized ones, safe to use on
//! the audio thread.
//!
//! VST3 allows the edit controller's conversions (`normalizedParamToPlain`,
//! `plainParamToNormalized`, `getParamStringByValue`) on the UI thread only, while
//! `Plugin::set_param` runs on the audio thread (automation is evaluated per block). So the
//! controller's curve of every plain-mapped parameter is sampled on the UI thread into
//! [`ParamCurves`] (at creation, and again when the plugin signals `restartComponent` with
//! `kParamValuesChanged` or `kParamTitlesChanged`), and the audio thread converts by interpolating
//! those samples: no controller call, no lock, no allocation. Converted values wait in
//! [`PendingParams`] for the next `process`, which hands them over as `IParameterChanges`.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Samples per plain-mapped curve (normalized steps of 1/256). The inverse is piecewise linear, so
/// its error in normalized units is at most `h² / 8 · max|f''/f'|` (h = 1/256): about 1.3e-5 for
/// a 20 Hz–20 kHz exponential curve. 1 KB per plain-mapped parameter.
pub(crate) const CURVE_POINTS: usize = 257;

/// The sampled normalized → plain curves of the plain-mapped parameters. Written on the UI thread
/// by [`ParamCurves::refresh`], read on the audio thread; every sample is an atomic, so a reader
/// racing a refresh sees a mix of old and new samples for at most that one conversion, and the
/// [`generation`](ParamCurves::generation) bump that ends the refresh tells it to convert again.
pub(crate) struct ParamCurves {
    /// Table index of each parameter (same order as the plugin's ids); `None` when the parameter
    /// is not plain-mapped.
    slot: Vec<Option<usize>>,
    /// VST3 id of each table.
    ids: Vec<u32>,
    /// Whether each table holds a finite curve (else conversions fall back to linear).
    valid: Vec<AtomicBool>,
    /// `CURVE_POINTS` plain values (f32 bits) per table: sample `k` is the plain value at
    /// normalized `k / (CURVE_POINTS - 1)`.
    plain: Vec<AtomicU32>,
    generation: AtomicU64,
}

impl ParamCurves {
    /// Empty tables for the parameters `plain` marks (same order as `ids`). Call
    /// [`ParamCurves::refresh`] to fill them.
    pub fn new(ids: &[u32], plain: &[bool]) -> ParamCurves {
        let mut slot = Vec::with_capacity(ids.len());
        let mut table_ids = Vec::new();
        for (&id, &p) in ids.iter().zip(plain) {
            slot.push(p.then(|| {
                table_ids.push(id);
                table_ids.len() - 1
            }));
        }
        slot.resize(ids.len(), None);
        let tables = table_ids.len();
        ParamCurves {
            slot,
            ids: table_ids,
            valid: (0..tables).map(|_| AtomicBool::new(false)).collect(),
            plain: (0..tables * CURVE_POINTS).map(|_| AtomicU32::new(0)).collect(),
            generation: AtomicU64::new(0),
        }
    }

    /// Re-samples every curve through `to_plain` (the controller's `normalizedParamToPlain`, so UI
    /// thread only), then bumps the generation.
    pub fn refresh(&self, to_plain: impl Fn(u32, f64) -> Option<f64>) {
        for (t, &id) in self.ids.iter().enumerate() {
            let mut ok = true;
            for k in 0..CURVE_POINTS {
                let n = k as f64 / (CURVE_POINTS - 1) as f64;
                let v = to_plain(id, n).map(|v| v as f32).filter(|v| v.is_finite());
                ok &= v.is_some();
                if let Some(cell) = self.plain.get(t * CURVE_POINTS + k) {
                    cell.store(v.unwrap_or(0.0).to_bits(), Ordering::Relaxed);
                }
            }
            if let Some(f) = self.valid.get(t) {
                f.store(ok, Ordering::Relaxed);
            }
        }
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Changes each time [`ParamCurves::refresh`] completes.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The samples of parameter `i`'s curve, when it has a valid one.
    fn table(&self, i: usize) -> Option<&[AtomicU32]> {
        let t = (*self.slot.get(i)?)?;
        if !self.valid.get(t)?.load(Ordering::Relaxed) {
            return None;
        }
        self.plain.get(t * CURVE_POINTS..(t + 1) * CURVE_POINTS)
    }

    /// Normalized → plain for parameter `i` (`None` without a valid curve).
    pub fn to_plain(&self, i: usize, n: f64) -> Option<f64> {
        let table = self.table(i)?;
        let at = |k: usize| table.get(k).map_or(0.0, |c| f64::from(f32::from_bits(c.load(Ordering::Relaxed))));
        let x = n.clamp(0.0, 1.0) * (CURVE_POINTS - 1) as f64;
        let k = (x.floor() as usize).min(CURVE_POINTS - 2);
        let frac = x - k as f64;
        Some(at(k) + frac * (at(k + 1) - at(k)))
    }

    /// Plain → normalized for parameter `i`, clamped to 0..1 (`None` without a valid curve). The
    /// curve may rise or fall; a curve that does neither gives some in-range value.
    pub fn to_normalized(&self, i: usize, plain: f64) -> Option<f64> {
        let table = self.table(i)?;
        if !plain.is_finite() {
            return None;
        }
        let at = |k: usize| table.get(k).map_or(0.0, |c| f64::from(f32::from_bits(c.load(Ordering::Relaxed))));
        let last = CURVE_POINTS - 1;
        // Work on a rising curve: flip the sign of a falling one.
        let sign = if at(last) >= at(0) { 1.0 } else { -1.0 };
        let key = |k: usize| sign * at(k);
        let v = sign * plain;
        if v <= key(0) {
            return Some(0.0);
        }
        if v >= key(last) {
            return Some(1.0);
        }
        // Largest k with key(k) <= v, in 0..last.
        let (mut lo, mut hi) = (0usize, last);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if key(mid) <= v {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let (a, b) = (key(lo), key(lo + 1));
        let frac = if b > a { ((v - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        let n = (lo as f64 + frac) / last as f64;
        n.is_finite().then(|| n.clamp(0.0, 1.0))
    }
}

/// Normalized values waiting for the next `process`: one slot per parameter, so the latest value
/// of each parameter wins and nothing is ever dropped, and no call allocates (all storage is
/// sized at creation). Parameters leave in the order they first changed; a block takes at most
/// as many as the `IParameterChanges` pool holds and the rest go in the next block.
pub(crate) struct PendingParams {
    value: Vec<f64>,
    queued: Vec<bool>,
    /// Indices of queued parameters, oldest first; never longer than the parameter count.
    order: Vec<usize>,
}

impl PendingParams {
    pub fn new(params: usize) -> PendingParams {
        PendingParams { value: vec![0.0; params], queued: vec![false; params], order: Vec::with_capacity(params) }
    }

    /// Queues normalized `n` for parameter `i`, replacing a value still waiting.
    pub fn set(&mut self, i: usize, n: f64) {
        let (Some(v), Some(q)) = (self.value.get_mut(i), self.queued.get_mut(i)) else { return };
        *v = n;
        if !*q && self.order.len() < self.order.capacity() {
            *q = true;
            self.order.push(i);
        }
    }

    pub fn clear(&mut self) {
        for &i in &self.order {
            if let Some(q) = self.queued.get_mut(i) {
                *q = false;
            }
        }
        self.order.clear();
    }

    /// Moves up to `max` waiting values into `out` as (VST3 id, normalized), oldest first, never
    /// past `out`'s capacity.
    pub fn take_into(&mut self, ids: &[u32], out: &mut Vec<(u32, f64)>, max: usize) {
        let k = self.order.len().min(max).min(out.capacity().saturating_sub(out.len()));
        for &i in self.order.get(..k).unwrap_or(&[]) {
            if let (Some(&id), Some(&v)) = (ids.get(i), self.value.get(i)) {
                out.push((id, v));
            }
            if let Some(q) = self.queued.get_mut(i) {
                *q = false;
            }
        }
        self.order.drain(..k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exp_curve(_id: u32, n: f64) -> Option<f64> {
        Some(20.0 * 1000f64.powf(n))
    }

    #[test]
    fn curves_match_the_controller_within_1e_4() {
        let c = ParamCurves::new(&[5, 12, 7], &[false, true, true]);
        assert_eq!(c.to_normalized(1, 100.0), None, "empty until refreshed");
        let g = c.generation();
        c.refresh(|id, n| match id {
            12 => exp_curve(id, n),
            7 => Some(2.0 - 2.0 * n),
            _ => None,
        });
        assert_ne!(c.generation(), g);
        assert_eq!(c.to_normalized(0, 1.0), None, "not plain-mapped");
        for v in [20.0, 25.0, 100.0, 440.0, 1000.0, 5_000.0, 19_999.0, 20_000.0] {
            let want = (v / 20.0f64).ln() / 1000f64.ln();
            let got = c.to_normalized(1, v).unwrap();
            assert!((got - want).abs() < 1e-4, "{v}: {got} vs {want}");
            let back = c.to_plain(1, got).unwrap();
            assert!((back - v).abs() <= 1e-4 * v, "round trip {v}: {back}");
        }
        assert_eq!(c.to_normalized(1, 1.0), Some(0.0), "below the range clamps");
        assert_eq!(c.to_normalized(1, 1e9), Some(1.0), "above the range clamps");
        // A falling curve.
        for v in [0.0, 0.3, 1.0, 1.7, 2.0] {
            let got = c.to_normalized(2, v).unwrap();
            assert!((got - (2.0 - v) / 2.0).abs() < 1e-6, "{v}: {got}");
        }
        assert_eq!(c.to_normalized(1, f64::NAN), None);
    }

    #[test]
    fn a_curve_with_holes_falls_back() {
        let c = ParamCurves::new(&[1], &[true]);
        c.refresh(|_, n| (n < 0.5).then_some(n));
        assert_eq!(c.to_plain(0, 0.25), None);
        c.refresh(|_, n| Some(n));
        assert!((c.to_plain(0, 0.25).unwrap() - 0.25).abs() < 1e-6, "a later refresh restores it");
    }

    #[test]
    fn pending_keeps_the_latest_value_per_parameter_and_never_drops_or_grows() {
        let ids: Vec<u32> = (100..110).collect();
        let mut p = PendingParams::new(ids.len());
        let cap = p.order.capacity();
        let ptr = p.order.as_ptr();
        // Far more changes than parameters: every parameter changes many times.
        for round in 0..50 {
            for i in 0..ids.len() {
                p.set(i, f64::from(round) + i as f64 / 100.0);
            }
        }
        p.set(99, 1.0); // out of range: ignored
        assert_eq!(p.order.len(), ids.len(), "one slot per parameter");
        assert_eq!((p.order.capacity(), p.order.as_ptr()), (cap, ptr), "no reallocation");
        // A pool smaller than the backlog: the rest carries over, in first-changed order.
        let mut out = Vec::with_capacity(4);
        p.take_into(&ids, &mut out, usize::MAX);
        assert_eq!(out, vec![(100, 49.0), (101, 49.01), (102, 49.02), (103, 49.03)]);
        assert_eq!(p.order.len(), 6);
        p.set(0, 7.0);
        out.clear();
        p.take_into(&ids, &mut out, 3);
        assert_eq!(out, vec![(104, 49.04), (105, 49.05), (106, 49.06)]);
        out.clear();
        p.take_into(&ids, &mut out, usize::MAX);
        assert_eq!(out, vec![(107, 49.07), (108, 49.08), (109, 49.09), (100, 7.0)]);
        assert_eq!(p.order.len(), 0);
        p.set(3, 0.5);
        p.clear();
        out.clear();
        p.take_into(&ids, &mut out, usize::MAX);
        assert!(out.is_empty());
        p.set(3, 0.25);
        p.take_into(&ids, &mut out, usize::MAX);
        assert_eq!(out, vec![(103, 0.25)], "cleared slots queue again");
    }
}
