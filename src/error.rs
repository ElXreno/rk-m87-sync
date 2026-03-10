use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no keyboard found")]
    NoDevice,

    #[error("permission denied on {path} — add udev rule or run as root")]
    PermissionDenied { path: PathBuf },

    #[error("device disconnected: {path}")]
    DeviceDisconnected { path: PathBuf },

    #[error("keyboard reported CRC error")]
    Crc,

    #[error("PulseAudio: {0}")]
    PulseConnect(String),

    #[error("PulseAudio server disconnected")]
    PulseDisconnected,

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
