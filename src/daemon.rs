use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use log::{info, warn};

use crate::error::{Error, Result};
use crate::hid::{self, HidDevice};
#[cfg(test)]
use crate::hid::KeyboardSink;
use crate::protocol::build_sysparam_payload;
use crate::state::{SyncAction, SyncConfig, SyncState};
use crate::volume::{self, DaemonEvent};

pub fn watch_loop(device_path: Option<&Path>, no_ping: bool) -> Result<()> {
    let (tx, rx) = mpsc::channel::<DaemonEvent>();
    let shutdown = Arc::new(AtomicBool::new(false));

    // Signal thread: SIGINT/SIGTERM → send Shutdown
    spawn_signal_thread(tx.clone(), Arc::clone(&shutdown));

    let epoch = Instant::now();
    let mut state = SyncState::new(SyncConfig::default(), epoch.elapsed());
    let mut device: Option<HidDevice> = None;
    let mut pa_handle: Option<JoinHandle<()>> = None;
    let mut logged_waiting = false;

    // Initial connections
    try_connect_device(&mut device, &mut state, device_path, no_ping, &epoch, &mut logged_waiting);
    try_spawn_pulse(&mut pa_handle, &mut state, &tx, &shutdown);

    loop {
        let now = epoch.elapsed();
        let timeout = state.next_deadline(now);

        match rx.recv_timeout(timeout) {
            Ok(DaemonEvent::VolumeChanged(vol)) => {
                state.on_volume_changed(vol, epoch.elapsed());
            }
            Ok(DaemonEvent::PulseDisconnected) => {
                warn!("PulseAudio disconnected");
                state.on_pulse_lost(epoch.elapsed());
                pa_handle = None;
            }
            Ok(DaemonEvent::Shutdown) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // Process state machine actions
        let now = epoch.elapsed();
        while let Some(action) = state.poll(now) {
            match action {
                SyncAction::SendSysparam { vol } => {
                    if let Some(dev) = &device {
                        let wall = chrono::Local::now();
                        let payload = build_sysparam_payload(vol, &wall);
                        match dev.send_sysparam(&payload) {
                            Ok(true) => {
                                info!("Synced: vol={vol}% time={}", wall.format("%H:%M:%S"));
                                state.on_send_ok(now);
                            }
                            other => {
                                if let Err(e) = other {
                                    warn!("Send error: {e}, reconnecting...");
                                } else {
                                    warn!("No response from keyboard, reconnecting...");
                                }
                                state.on_device_lost(now);
                                device = None;
                            }
                        }
                    }
                }
                SyncAction::ConnectDevice => {
                    try_connect_device(&mut device, &mut state, device_path, no_ping, &epoch, &mut logged_waiting);
                }
                SyncAction::SpawnPulseMonitor => {
                    try_spawn_pulse(&mut pa_handle, &mut state, &tx, &shutdown);
                }
            }
        }
    }

    // Clean shutdown
    info!("Shutdown.");
    shutdown.store(true, Ordering::Relaxed);
    // PA thread checks shutdown flag and exits on its own
    if let Some(h) = pa_handle {
        let _ = h.join();
    }
    Ok(())
}

fn try_connect_device(
    device: &mut Option<HidDevice>,
    state: &mut SyncState,
    device_path: Option<&Path>,
    no_ping: bool,
    epoch: &Instant,
    logged_waiting: &mut bool,
) {
    match hid::connect(device_path, no_ping) {
        Ok(dev) => {
            info!("Connected: {} ({})", dev.path.display(), dev.protocol.label());
            state.on_device_connected(epoch.elapsed());
            *device = Some(dev);
            *logged_waiting = false;
        }
        Err(Error::PermissionDenied { path }) => {
            if !*logged_waiting {
                warn!("Permission denied on {}", path.display());
                *logged_waiting = true;
            }
            state.on_device_lost(epoch.elapsed());
        }
        Err(e) => {
            if !*logged_waiting {
                info!("Waiting for keyboard ({e})...");
                *logged_waiting = true;
            }
            state.on_device_lost(epoch.elapsed());
        }
    }
}

fn try_spawn_pulse(
    pa_handle: &mut Option<JoinHandle<()>>,
    state: &mut SyncState,
    tx: &Sender<DaemonEvent>,
    shutdown: &Arc<AtomicBool>,
) {
    info!("Connecting to PulseAudio...");
    // PA thread handles its own connection internally
    let handle = volume::spawn_pulse_thread(tx.clone(), Arc::clone(shutdown));
    *pa_handle = Some(handle);
    state.on_pulse_connected();
}

fn spawn_signal_thread(tx: Sender<DaemonEvent>, shutdown: Arc<AtomicBool>) {
    use signal_hook::iterator::Signals;

    let mut signals =
        Signals::new([signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM])
            .expect("failed to register signals");

    std::thread::Builder::new()
        .name("signal".into())
        .spawn(move || {
            if let Some(_sig) = signals.forever().next() {
                shutdown.store(true, Ordering::Relaxed);
                let _ = tx.send(DaemonEvent::Shutdown);
            }
        })
        .expect("failed to spawn signal thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    struct MockKeyboard {
        sent: RefCell<Vec<[u8; 14]>>,
        fail_next: Cell<bool>,
    }

    impl MockKeyboard {
        fn new() -> Self {
            Self {
                sent: RefCell::new(Vec::new()),
                fail_next: Cell::new(false),
            }
        }
    }

    impl KeyboardSink for MockKeyboard {
        fn send_sysparam(&self, payload: &[u8; 14]) -> Result<bool> {
            if self.fail_next.replace(false) {
                return Err(Error::DeviceDisconnected {
                    path: PathBuf::from("mock"),
                });
            }
            self.sent.borrow_mut().push(*payload);
            Ok(true)
        }
    }

    /// Run a mini event loop with a mock keyboard for testing.
    /// Runs for up to `max_duration` from epoch, processing channel events
    /// and state machine timeouts.
    fn run_test_loop(
        rx: &mpsc::Receiver<DaemonEvent>,
        state: &mut SyncState,
        kbd: &MockKeyboard,
        epoch: &Instant,
        max_duration: Duration,
    ) {
        let deadline = max_duration;
        let mut channel_closed = false;

        loop {
            let now = epoch.elapsed();
            if now >= deadline {
                break;
            }

            let timeout = state.next_deadline(now).min(deadline.saturating_sub(now));

            if !channel_closed {
                match rx.recv_timeout(timeout) {
                    Ok(DaemonEvent::VolumeChanged(vol)) => {
                        state.on_volume_changed(vol, epoch.elapsed());
                    }
                    Ok(DaemonEvent::PulseDisconnected) => {
                        state.on_pulse_lost(epoch.elapsed());
                    }
                    Ok(DaemonEvent::Shutdown) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        channel_closed = true;
                    }
                }
            } else {
                // Channel closed — just sleep for the timeout to let state machine fire
                std::thread::sleep(timeout);
            }

            let now = epoch.elapsed();
            while let Some(action) = state.poll(now) {
                match action {
                    SyncAction::SendSysparam { vol } => {
                        let wall = chrono::Local::now();
                        let payload = build_sysparam_payload(vol, &wall);
                        match kbd.send_sysparam(&payload) {
                            Ok(_) => state.on_send_ok(now),
                            Err(_) => state.on_device_lost(now),
                        }
                    }
                    SyncAction::ConnectDevice => {
                        state.on_device_connected(now);
                    }
                    SyncAction::SpawnPulseMonitor => {
                        state.on_pulse_connected();
                    }
                }
            }

            // If channel closed and no pending short-term actions, exit
            if channel_closed {
                let remaining = state.next_deadline(epoch.elapsed());
                if remaining >= Duration::from_millis(500) {
                    break; // only long-term timers left
                }
            }
        }
    }

    fn test_config() -> SyncConfig {
        SyncConfig {
            debounce: Duration::from_millis(50),
            time_sync_interval: Duration::from_secs(600),
            max_poll_timeout: Duration::from_millis(100),
            reconnect_delay: Duration::from_millis(100),
        }
    }

    #[test]
    fn test_volume_event_debounce_send() {
        let (tx, rx) = mpsc::channel();
        let epoch = Instant::now();
        let mut state = SyncState::new(test_config(), epoch.elapsed());
        let kbd = MockKeyboard::new();

        // Handle initial time sync
        if let Some(SyncAction::SendSysparam { .. }) = state.poll(epoch.elapsed()) {
            state.on_send_ok(epoch.elapsed());
        }

        // Send volume event
        tx.send(DaemonEvent::VolumeChanged(75)).unwrap();
        // Drop sender so loop exits after processing
        drop(tx);

        // Run loop — it will process the volume event, wait for debounce, then send
        run_test_loop(&rx, &mut state, &kbd, &epoch, Duration::from_millis(200));

        let sent = kbd.sent.borrow();
        assert!(!sent.is_empty(), "should have sent at least one sysparam");
        assert_eq!(sent.last().unwrap()[0], 75); // volume byte
    }

    #[test]
    fn test_shutdown_exits_loop() {
        let (tx, rx) = mpsc::channel();
        let epoch = Instant::now();
        let mut state = SyncState::new(test_config(), epoch.elapsed());
        let kbd = MockKeyboard::new();

        tx.send(DaemonEvent::Shutdown).unwrap();
        run_test_loop(&rx, &mut state, &kbd, &epoch, Duration::from_millis(100));

        // If we got here, the loop exited — test passes
    }

    #[test]
    fn test_pulse_disconnect_reconnect() {
        let (tx, rx) = mpsc::channel();
        let epoch = Instant::now();
        let mut state = SyncState::new(test_config(), epoch.elapsed());
        let kbd = MockKeyboard::new();

        // Handle initial time sync
        if let Some(SyncAction::SendSysparam { .. }) = state.poll(epoch.elapsed()) {
            state.on_send_ok(epoch.elapsed());
        }

        tx.send(DaemonEvent::PulseDisconnected).unwrap();
        drop(tx);

        run_test_loop(&rx, &mut state, &kbd, &epoch, Duration::from_millis(200));

        // State should have scheduled and processed a PA reconnect
        // (run_test_loop calls on_pulse_connected for SpawnPulseMonitor)
        assert!(state.poll(epoch.elapsed()).is_none());
    }

    #[test]
    fn test_device_send_failure_reconnect() {
        let (tx, rx) = mpsc::channel();
        let epoch = Instant::now();
        let cfg = SyncConfig {
            debounce: Duration::from_millis(10),
            ..test_config()
        };
        let mut state = SyncState::new(cfg, epoch.elapsed());
        let kbd = MockKeyboard::new();

        // Handle initial time sync
        if let Some(SyncAction::SendSysparam { .. }) = state.poll(epoch.elapsed()) {
            state.on_send_ok(epoch.elapsed());
        }

        // Set mock to fail next send
        kbd.fail_next.set(true);

        tx.send(DaemonEvent::VolumeChanged(50)).unwrap();
        drop(tx);

        run_test_loop(&rx, &mut state, &kbd, &epoch, Duration::from_millis(200));

        // The failed send should have triggered device_lost → reconnect
        // The test loop handles ConnectDevice by calling on_device_connected
        // After reconnect, a new initial sync should fire
        let sent = kbd.sent.borrow();
        // Should have at least the post-reconnect sync
        assert!(!sent.is_empty(), "should have sent after reconnect");
    }

    #[test]
    fn test_time_sync_fires() {
        let epoch = Instant::now();
        let cfg = SyncConfig {
            time_sync_interval: Duration::from_millis(50),
            max_poll_timeout: Duration::from_millis(30),
            ..test_config()
        };
        let mut state = SyncState::new(cfg, epoch.elapsed());
        let kbd = MockKeyboard::new();

        // Initial sync
        if let Some(SyncAction::SendSysparam { .. }) = state.poll(epoch.elapsed()) {
            let payload = build_sysparam_payload(0, &chrono::Local::now());
            kbd.send_sysparam(&payload).unwrap();
            state.on_send_ok(epoch.elapsed());
        }

        // Wait for interval to elapse
        std::thread::sleep(Duration::from_millis(60));

        let now = epoch.elapsed();
        let action = state.poll(now);
        assert!(
            matches!(action, Some(SyncAction::SendSysparam { .. })),
            "time sync should fire after interval"
        );
    }
}
