use std::time::Duration;

pub const VOL_DEBOUNCE: Duration = Duration::from_millis(500);
pub const TIME_SYNC_INTERVAL: Duration = Duration::from_secs(30 * 60);
pub const MAX_POLL_TIMEOUT: Duration = Duration::from_secs(2);
pub const RECONNECT_DELAY: Duration = Duration::from_secs(2);

pub struct SyncConfig {
    pub debounce: Duration,
    pub time_sync_interval: Duration,
    pub max_poll_timeout: Duration,
    pub reconnect_delay: Duration,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            debounce: VOL_DEBOUNCE,
            time_sync_interval: TIME_SYNC_INTERVAL,
            max_poll_timeout: MAX_POLL_TIMEOUT,
            reconnect_delay: RECONNECT_DELAY,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum SyncAction {
    SendSysparam { vol: u8 },
    ConnectDevice,
    SpawnPulseMonitor,
}

pub struct SyncState {
    config: SyncConfig,
    last_vol: Option<u8>,
    vol_dirty: bool,
    vol_last_change: Duration,
    next_time_sync: Duration,
    device_reconnect_at: Option<Duration>,
    pulse_reconnect_at: Option<Duration>,
    /// Whether an initial time sync has been requested but not yet sent
    initial_sync_pending: bool,
}

impl SyncState {
    pub fn new(config: SyncConfig, now: Duration) -> Self {
        Self {
            last_vol: None,
            vol_dirty: false,
            vol_last_change: now,
            next_time_sync: now, // sync immediately
            device_reconnect_at: None,
            pulse_reconnect_at: None,
            initial_sync_pending: true,
            config,
        }
    }

    /// Volume changed event from PA thread.
    pub fn on_volume_changed(&mut self, vol: u8, now: Duration) {
        if self.last_vol == Some(vol) {
            return; // dedup
        }
        self.last_vol = Some(vol);
        self.vol_dirty = true;
        self.vol_last_change = now;
    }

    /// Check what needs to happen right now. Returns action if any.
    /// Call in a loop until it returns None.
    pub fn poll(&mut self, now: Duration) -> Option<SyncAction> {
        // Device reconnect takes priority
        if let Some(at) = self.device_reconnect_at {
            if now >= at {
                self.device_reconnect_at = None;
                return Some(SyncAction::ConnectDevice);
            }
            // Device is down — don't try to send anything
            return None;
        }

        // PA reconnect
        if let Some(at) = self.pulse_reconnect_at {
            if now >= at {
                self.pulse_reconnect_at = None;
                return Some(SyncAction::SpawnPulseMonitor);
            }
        }

        // Initial time sync
        if self.initial_sync_pending && now >= self.next_time_sync {
            self.initial_sync_pending = false;
            let vol = self.last_vol.unwrap_or(0);
            return Some(SyncAction::SendSysparam { vol });
        }

        // Volume debounce expired
        if self.vol_dirty && now >= self.vol_last_change + self.config.debounce {
            self.vol_dirty = false;
            let vol = self.last_vol.unwrap_or(0);
            return Some(SyncAction::SendSysparam { vol });
        }

        // Periodic time sync
        if !self.initial_sync_pending && now >= self.next_time_sync {
            let vol = self.last_vol.unwrap_or(0);
            return Some(SyncAction::SendSysparam { vol });
        }

        None
    }

    /// How long until next action. Used as recv_timeout.
    pub fn next_deadline(&self, now: Duration) -> Duration {
        let mut earliest = now + self.config.max_poll_timeout;

        if let Some(at) = self.device_reconnect_at {
            earliest = earliest.min(at);
        }
        if let Some(at) = self.pulse_reconnect_at {
            earliest = earliest.min(at);
        }
        if self.device_reconnect_at.is_none() {
            if self.vol_dirty {
                earliest = earliest.min(self.vol_last_change + self.config.debounce);
            }
            earliest = earliest.min(self.next_time_sync);
        }

        earliest.saturating_sub(now)
    }

    /// Called after send_sysparam succeeds.
    /// Clears vol_dirty because every sysparam includes the current volume.
    pub fn on_send_ok(&mut self, now: Duration) {
        self.next_time_sync = now + self.config.time_sync_interval;
        self.vol_dirty = false;
    }

    /// Called when HID send fails (device gone).
    pub fn on_device_lost(&mut self, now: Duration) {
        self.device_reconnect_at = Some(now + self.config.reconnect_delay);
    }

    /// Called when PA thread reports disconnect.
    pub fn on_pulse_lost(&mut self, now: Duration) {
        self.pulse_reconnect_at = Some(now + self.config.reconnect_delay);
    }

    /// Called when device reconnected successfully.
    pub fn on_device_connected(&mut self, now: Duration) {
        self.device_reconnect_at = None;
        // Sync immediately after reconnect
        self.next_time_sync = now;
        self.initial_sync_pending = true;
    }

    /// Called when PA thread respawned successfully.
    pub fn on_pulse_connected(&mut self) {
        self.pulse_reconnect_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn fast_config() -> SyncConfig {
        SyncConfig {
            debounce: Duration::from_millis(500),
            time_sync_interval: Duration::from_secs(60),
            max_poll_timeout: Duration::from_secs(2),
            reconnect_delay: Duration::from_secs(2),
        }
    }

    #[test]
    fn test_initial_time_sync() {
        let mut s = SyncState::new(fast_config(), ms(0));
        let action = s.poll(ms(0));
        assert_eq!(action, Some(SyncAction::SendSysparam { vol: 0 }));
    }

    #[test]
    fn test_volume_debounce() {
        let mut s = SyncState::new(fast_config(), ms(0));
        s.on_send_ok(ms(0)); // clear initial time sync
        s.initial_sync_pending = false;

        s.on_volume_changed(50, ms(100));
        assert_eq!(s.poll(ms(100)), None); // too soon
        assert_eq!(s.poll(ms(500)), None); // 400ms since change, still too soon

        let action = s.poll(ms(600));
        assert_eq!(action, Some(SyncAction::SendSysparam { vol: 50 }));
    }

    #[test]
    fn test_volume_rapid_changes() {
        let mut s = SyncState::new(fast_config(), ms(0));
        s.on_send_ok(ms(0));
        s.initial_sync_pending = false;

        s.on_volume_changed(50, ms(0));
        assert_eq!(s.poll(ms(0)), None);

        s.on_volume_changed(55, ms(100));
        assert_eq!(s.poll(ms(100)), None);

        s.on_volume_changed(60, ms(200));
        assert_eq!(s.poll(ms(600)), None); // 400ms since last change

        let action = s.poll(ms(700));
        assert_eq!(action, Some(SyncAction::SendSysparam { vol: 60 }));
    }

    #[test]
    fn test_time_sync_interval() {
        let cfg = fast_config();
        let interval = cfg.time_sync_interval;
        let mut s = SyncState::new(cfg, ms(0));

        // Initial sync
        let action = s.poll(ms(0));
        assert_eq!(action, Some(SyncAction::SendSysparam { vol: 0 }));
        s.on_send_ok(ms(0));

        // Nothing until interval elapses
        assert_eq!(s.poll(ms(1000)), None);

        // After interval
        let action = s.poll(interval);
        assert_eq!(action, Some(SyncAction::SendSysparam { vol: 0 }));
    }

    #[test]
    fn test_next_deadline_during_debounce() {
        let mut s = SyncState::new(fast_config(), ms(0));
        s.on_send_ok(ms(0));
        s.initial_sync_pending = false;

        s.on_volume_changed(50, ms(100));
        let deadline = s.next_deadline(ms(100));
        assert_eq!(deadline, ms(500)); // debounce = 500ms from change
    }

    #[test]
    fn test_next_deadline_idle() {
        let mut s = SyncState::new(fast_config(), ms(0));
        s.on_send_ok(ms(0));
        s.initial_sync_pending = false;

        // Next sync is far away, but capped at max_poll_timeout (2s)
        let deadline = s.next_deadline(ms(0));
        assert_eq!(deadline, ms(2000));
    }

    #[test]
    fn test_device_lost_schedules_reconnect() {
        let mut s = SyncState::new(fast_config(), ms(0));
        s.on_send_ok(ms(0));
        s.initial_sync_pending = false;

        s.on_device_lost(ms(1000));

        // Should not try to send while device is down
        s.on_volume_changed(50, ms(1000));
        assert_eq!(s.poll(ms(1500)), None); // debounce would be ready but device is down

        // Reconnect fires after delay
        let action = s.poll(ms(3000));
        assert_eq!(action, Some(SyncAction::ConnectDevice));
    }

    #[test]
    fn test_pulse_lost_schedules_reconnect() {
        let mut s = SyncState::new(fast_config(), ms(0));
        s.on_send_ok(ms(0));
        s.initial_sync_pending = false;

        s.on_pulse_lost(ms(1000));

        assert_eq!(s.poll(ms(2000)), None); // not yet

        let action = s.poll(ms(3000));
        assert_eq!(action, Some(SyncAction::SpawnPulseMonitor));
    }

    #[test]
    fn test_device_reconnect_resets() {
        let mut s = SyncState::new(fast_config(), ms(0));
        s.on_send_ok(ms(0));
        s.initial_sync_pending = false;

        s.on_device_lost(ms(1000));
        s.on_device_connected(ms(3000));

        // Should immediately want to sync after reconnect
        let action = s.poll(ms(3000));
        assert_eq!(action, Some(SyncAction::SendSysparam { vol: 0 }));
    }

    #[test]
    fn test_reconnect_no_double_send() {
        // Regression: vol_dirty persisted across reconnect, causing a second
        // SendSysparam immediately after the initial sync — the dongle can't
        // handle two rapid sends and returns Ok(false), triggering an infinite
        // reconnect cycle.
        let mut s = SyncState::new(fast_config(), ms(0));
        s.on_send_ok(ms(0));
        s.initial_sync_pending = false;

        // Volume changes, then device is lost before debounce fires
        s.on_volume_changed(50, ms(1000));
        s.on_device_lost(ms(1200));

        // Reconnect after delay
        assert_eq!(s.poll(ms(3200)), Some(SyncAction::ConnectDevice));
        s.on_device_connected(ms(3200));

        // Initial sync after reconnect (includes current vol=50)
        let action = s.poll(ms(3200));
        assert_eq!(action, Some(SyncAction::SendSysparam { vol: 50 }));
        s.on_send_ok(ms(3200));

        // Must NOT fire a second send for the stale vol_dirty
        assert_eq!(s.poll(ms(3200)), None);
        assert_eq!(s.poll(ms(4000)), None); // even after debounce would have expired
    }

    #[test]
    fn test_next_deadline_no_busywait_when_device_down() {
        // Regression: when device is not connected and initial_sync_pending is
        // true, next_deadline returned Duration::ZERO because next_time_sync
        // was in the past. This caused a busy-wait loop since poll() returns
        // None while device is down.
        let mut s = SyncState::new(fast_config(), ms(0));

        // Device is down from the start
        s.on_device_lost(ms(0));

        // next_deadline must NOT return zero — it should wait for reconnect
        let deadline = s.next_deadline(ms(100));
        assert!(
            deadline >= ms(1000),
            "deadline should wait for reconnect delay, got {deadline:?}"
        );
    }

    #[test]
    fn test_next_deadline_no_busywait_vol_dirty_device_down() {
        // Regression: when vol_dirty debounce expired and device was down,
        // next_deadline returned Duration::ZERO because vol_last_change +
        // debounce was in the past. This caused a busy-wait loop since
        // poll() returns None while device is down.
        let mut s = SyncState::new(fast_config(), ms(0));
        s.on_send_ok(ms(0));
        s.initial_sync_pending = false;

        // Volume changes, then device goes down before debounce fires
        s.on_volume_changed(50, ms(1000));
        s.on_device_lost(ms(1200));

        // At ms(1600), debounce has expired (1000+500=1500) but device is down
        let deadline = s.next_deadline(ms(1600));
        assert!(
            deadline >= ms(500),
            "deadline should wait for reconnect, got {deadline:?}"
        );
    }

    #[test]
    fn test_send_ok_advances_time_sync() {
        let cfg = fast_config();
        let interval = cfg.time_sync_interval;
        let mut s = SyncState::new(cfg, ms(0));

        s.poll(ms(0)); // initial sync
        s.on_send_ok(ms(100));

        // Next sync should be at 100 + interval
        assert_eq!(s.poll(interval + ms(50)), None);

        let action = s.poll(interval + ms(100));
        assert_eq!(action, Some(SyncAction::SendSysparam { vol: 0 }));
    }
}
