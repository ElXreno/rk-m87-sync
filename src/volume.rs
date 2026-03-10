use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use libpulse_binding as pulse;
use pulse::context::{Context, FlagSet as CtxFlags, State as CtxState};
use pulse::mainloop::standard::{IterateResult, Mainloop};
use pulse::proplist::Proplist;
use pulse::time::MicroSeconds;

use crate::error::{Error, Result};

/// Events sent from background threads to the main event loop.
pub enum DaemonEvent {
    VolumeChanged(u8),
    PulseDisconnected,
    Shutdown,
}

/// Spawn a PA monitoring thread. Sends VolumeChanged on sink events,
/// PulseDisconnected on PA error, and exits when shutdown flag is set.
pub fn spawn_pulse_thread(
    tx: Sender<DaemonEvent>,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("pa-monitor".into())
        .spawn(move || {
            if let Err(e) = run_pulse_monitor(&tx, &shutdown) {
                log::warn!("PA monitor thread exited: {e}");
                let _ = tx.send(DaemonEvent::PulseDisconnected);
            }
        })
        .expect("failed to spawn PA thread")
}

/// Internal PA monitoring loop. Runs until error or shutdown.
fn run_pulse_monitor(tx: &Sender<DaemonEvent>, shutdown: &AtomicBool) -> Result<()> {
    let (mut event_ml, mut event_ctx) = pa_connect("rk-m87-events")?;
    let (mut query_ml, mut query_ctx) = pa_connect("rk-m87-query")?;

    // Subscribe to sink events
    event_ctx.subscribe(
        pulse::context::subscribe::InterestMaskSet::SINK,
        |_| {},
    );

    let got_event = Arc::new(AtomicBool::new(false));
    let got_event_cb = Arc::clone(&got_event);
    event_ctx.set_subscribe_callback(Some(Box::new(move |_facility, _operation, _index| {
        got_event_cb.store(true, Ordering::Relaxed);
    })));

    let mut last_vol: Option<u8> = None;

    // Send initial volume
    if let Ok(vol) = query_volume(&mut query_ml, &query_ctx) {
        last_vol = Some(vol);
        let _ = tx.send(DaemonEvent::VolumeChanged(vol));
    }

    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Check PA connection health
        if event_ctx.get_state() != CtxState::Ready {
            let _ = tx.send(DaemonEvent::PulseDisconnected);
            break;
        }

        // prepare/poll/dispatch with short timeout
        let timeout_us = POLL_INTERVAL.as_micros() as u64;
        event_ml
            .prepare(Some(MicroSeconds(timeout_us)))
            .map_err(|_| Error::PulseDisconnected)?;

        // poll() may return Err on signal — treat as non-fatal, just continue
        let _ = event_ml.poll();

        event_ml
            .dispatch()
            .map_err(|_| Error::PulseDisconnected)?;

        if got_event.swap(false, Ordering::Relaxed) {
            // Sink event fired — query volume
            match query_volume(&mut query_ml, &query_ctx) {
                Ok(vol) => {
                    if last_vol != Some(vol) {
                        last_vol = Some(vol);
                        if tx.send(DaemonEvent::VolumeChanged(vol)).is_err() {
                            break; // main loop gone
                        }
                    }
                }
                Err(_) => {
                    let _ = tx.send(DaemonEvent::PulseDisconnected);
                    break;
                }
            }
        }
    }

    // Clean disconnect
    // Safety: we re-borrow mutably since the subscribe callback Arc is separate
    event_ctx.disconnect();
    query_ctx.disconnect();
    let _ = event_ml.iterate(false);
    let _ = query_ml.iterate(false);

    Ok(())
}

/// Iterate the mainloop until the operation completes (or iteration limit).
fn await_op<F: ?Sized>(ml: &mut Mainloop, op: &pulse::operation::Operation<F>) -> Result<()> {
    const MAX_ITERATIONS: usize = 200;
    for _ in 0..MAX_ITERATIONS {
        match ml.iterate(true) {
            IterateResult::Quit(_) | IterateResult::Err(_) => {
                return Err(Error::PulseDisconnected);
            }
            IterateResult::Success(_) => {}
        }
        if op.get_state() != pulse::operation::State::Running {
            break;
        }
    }
    Ok(())
}

/// Query current default sink volume (0-100). Iteration-limited to prevent hangs.
fn query_volume(query_ml: &mut Mainloop, query_ctx: &Context) -> Result<u8> {
    if query_ctx.get_state() != CtxState::Ready {
        return Err(Error::PulseDisconnected);
    }

    // Get server info for default sink name
    let default_sink = Arc::new(std::sync::Mutex::new(String::new()));
    let op = query_ctx.introspect().get_server_info({
        let default_sink = Arc::clone(&default_sink);
        move |info| {
            if let Some(name) = &info.default_sink_name {
                *default_sink.lock().unwrap() = name.to_string();
            }
        }
    });
    await_op(query_ml, &op)?;

    let sink_name = default_sink.lock().unwrap().clone();
    if sink_name.is_empty() {
        return Ok(0);
    }

    // Query sink by name
    let volume = Arc::new(AtomicU8::new(255));
    let op = query_ctx.introspect().get_sink_info_by_name(&sink_name, {
        let volume = Arc::clone(&volume);
        move |result| {
            use pulse::callbacks::ListResult;
            if let ListResult::Item(info) = result {
                if info.mute {
                    volume.store(0, Ordering::Relaxed);
                } else {
                    let avg = info.volume.avg();
                    let pct = (avg.0 as f64 / pulse::volume::Volume::NORMAL.0 as f64 * 100.0)
                        .round()
                        .min(100.0) as u8;
                    volume.store(pct, Ordering::Relaxed);
                }
            }
        }
    });
    await_op(query_ml, &op)?;

    let v = volume.load(Ordering::Relaxed);
    Ok(if v == 255 { 0 } else { v })
}

/// Connect a PulseAudio context and block until ready.
fn pa_connect(app_name: &str) -> Result<(Mainloop, Context)> {
    let mut proplist = Proplist::new().ok_or_else(|| Error::PulseConnect("proplist".into()))?;
    let _ = proplist.set_str(
        pulse::proplist::properties::APPLICATION_NAME,
        app_name,
    );

    let mut ml =
        Mainloop::new().ok_or_else(|| Error::PulseConnect("mainloop creation failed".into()))?;
    let mut ctx = Context::new_with_proplist(&ml, app_name, &proplist)
        .ok_or_else(|| Error::PulseConnect("context creation failed".into()))?;

    ctx.connect(None, CtxFlags::NOFLAGS, None)
        .map_err(|_| Error::PulseConnect("connect failed".into()))?;

    const MAX_CONNECT_ITERATIONS: usize = 500;
    for _ in 0..MAX_CONNECT_ITERATIONS {
        match ml.iterate(true) {
            IterateResult::Quit(_) | IterateResult::Err(_) => {
                return Err(Error::PulseConnect("mainloop iterate failed".into()));
            }
            IterateResult::Success(_) => {}
        }
        match ctx.get_state() {
            CtxState::Ready => return Ok((ml, ctx)),
            CtxState::Failed | CtxState::Terminated => {
                return Err(Error::PulseConnect("context failed".into()));
            }
            _ => {}
        }
    }

    Err(Error::PulseConnect("connect timeout".into()))
}

/// Get current volume via libpulse. For one-shot mode.
pub fn get_volume() -> Result<u8> {
    let (mut ml, mut ctx) = pa_connect("rk-m87-query")?;
    let vol = query_volume(&mut ml, &ctx)?;
    ctx.disconnect();
    let _ = ml.iterate(false);
    Ok(vol)
}
