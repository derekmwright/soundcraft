//! Gate G3 (ADR 0012, audit F02): parameter changes reach a VST3 processor without any
//! edit-controller call on the render thread, already normalized, and follow a curve the plug-in
//! changes through `restartComponent`.
//!
//! The fixture's probe mode is per process (`VST3_FIXTURE_PROBES`), so `main` reruns this binary
//! with it set (`harness = false`, which also makes `main` the process main thread, where editor
//! calls are allowed). In probe mode the fixture ends the process with status 3 as soon as its
//! edit controller is called on a thread that has run `process`, and writes the last Freq value
//! its processor received to its second output channel.

mod common;

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const PROBES: &str = "VST3_FIXTURE_PROBES";
const BLOCK: usize = 64;

/// Freq's normalized value for `plain`, as the fixture's controller computes it (`20 · 1000ⁿ`).
fn freq_log(plain: f64) -> f64 {
    (plain / 20.0).ln() / 1000f64.ln()
}

/// The same after the fixture's Mode C reshape (`20 + 19980 n²`).
fn freq_square(plain: f64) -> f64 {
    ((plain - 20.0) / 19_980.0).sqrt()
}

fn gain_id() -> String {
    let found = soundcraft_vst3_host::scan_paths(&[common::fixture_dir().to_path_buf()]);
    found.into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id
}

/// Runs one block of 0.5 in, returns (channel 0, channel 1) at the first frame.
fn block(p: &mut Box<dyn soundcraft_dsp::Plugin>, io: &mut [Vec<f32>]) -> (f32, f32) {
    for ch in io.iter_mut() {
        ch.fill(0.5);
    }
    p.process(io, BLOCK);
    (io[0][0], io[1][0])
}

/// (1) and (2): automation changes Gain and Freq every block on the render thread while the main
/// thread services the editor handle, pushes editor-side values and saves state. No controller
/// call may happen on the render thread, and each block's Freq point must match the controller's
/// own conversion within 1e-4 (the last of several changes in a block wins).
fn automation_never_calls_the_controller_on_the_render_thread() {
    let id = gain_id();
    let mut p = soundcraft_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("editor handle");
    let mut reader = p.state_reader().expect("state reader");
    let done = Arc::new(AtomicBool::new(false));
    let render_done = Arc::clone(&done);
    let render = std::thread::spawn(move || {
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        // The first block marks this thread as the render thread in the fixture.
        block(&mut p, &mut io);
        let mut worst = 0.0f64;
        for k in 0..2_000u32 {
            let plain = 20.0 * 1000f32.powf(k as f32 / 1_999.0);
            let gain = (k % 200) as f32 / 100.0;
            assert!(p.set_param("12", 1_000.0));
            assert!(p.set_param("12", plain), "latest value in a block wins");
            assert!(p.set_param("7", gain));
            let (out, freq) = block(&mut p, &mut io);
            worst = worst.max((f64::from(freq) - freq_log(f64::from(plain))).abs());
            assert!((out - 0.5 * gain).abs() < 1e-4, "block {k}: gain {gain} gave {out}");
        }
        render_done.store(true, Ordering::SeqCst);
        (p, worst)
    });
    let mut saves = 0;
    while !done.load(Ordering::SeqCst) && !render.is_finished() {
        let _ = ed.idle();
        ed.set_param("7", 1.0);
        assert!(reader.save_state().is_some());
        saves += 1;
    }
    let (p, worst) = render.join().expect("render thread");
    assert!(worst < 1e-4, "Freq points off the controller's curve by {worst}");
    println!("  render thread: 2000 automated blocks, {saves} state saves alongside, worst Freq error {worst:.2e}");
    drop(ed);
    drop(p);
}

/// The plug-in reshapes Freq (Mode C) and signals `kParamTitlesChanged` from the UI thread; the
/// editor handle's idle re-samples the curve there. The value the host set before the change is
/// re-sent on the new curve without another `set_param`, and new values follow the new curve.
fn a_restart_re_samples_the_curve_on_the_ui_thread() {
    let id = gain_id();
    let mut p = soundcraft_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("editor handle");
    let mut p = std::thread::spawn(move || {
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        block(&mut p, &mut io);
        assert!(p.set_param("12", 5_000.0));
        let (_, freq) = block(&mut p, &mut io);
        assert!((f64::from(freq) - freq_log(5_000.0)).abs() < 1e-4, "before the restart: {freq}");
        p
    })
    .join()
    .expect("render thread");
    // Main (UI) thread: the mode change makes the fixture call restartComponent; idle re-samples.
    ed.set_param("3", 2.0);
    let _ = ed.idle();
    let p = std::thread::spawn(move || {
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        let (_, freq) = block(&mut p, &mut io);
        assert!((f64::from(freq) - freq_square(5_000.0)).abs() < 1e-4, "re-sent on the new curve: {freq}");
        for plain in [200.0f32, 1_000.0, 12_345.0, 20_000.0] {
            assert!(p.set_param("12", plain));
            let (_, freq) = block(&mut p, &mut io);
            let want = freq_square(f64::from(plain));
            assert!((f64::from(freq) - want).abs() < 1e-4, "{plain}: {freq} vs {want}");
        }
        p
    })
    .join()
    .expect("render thread");
    ed.set_param("3", 0.0);
    let _ = ed.idle();
    drop(ed);
    drop(p);
}

fn main() {
    if std::env::var_os(PROBES).is_none() {
        let exe = std::env::current_exe().expect("test binary path");
        let out = Command::new(exe).env(PROBES, "1").output().expect("rerun with fixture probes");
        print!("{}", String::from_utf8_lossy(&out.stdout));
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "probe run failed: {:?}", out.status);
        return;
    }
    automation_never_calls_the_controller_on_the_render_thread();
    println!("automation_never_calls_the_controller_on_the_render_thread: ok");
    a_restart_re_samples_the_curve_on_the_ui_thread();
    println!("a_restart_re_samples_the_curve_on_the_ui_thread: ok");
}
