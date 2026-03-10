mod daemon;
mod error;
mod hid;
mod protocol;
mod state;
mod volume;

use std::path::PathBuf;
use std::process::ExitCode;

use log::{debug, error, info, warn};

use error::Error;

/// Sync system time and volume to RK M87 keyboard LCD
#[derive(argh::FromArgs)]
struct Args {
    /// hidraw device path (auto-detected if omitted)
    #[argh(option, short = 'd')]
    device: Option<String>,

    /// skip echo ping check (dongle mode only)
    #[argh(switch)]
    no_ping: bool,

    /// daemon mode: continuously sync time and volume
    #[argh(switch)]
    daemon: bool,
}

fn run() -> Result<(), Error> {
    let args: Args = argh::from_env();
    let device = args.device.as_deref().map(PathBuf::from);

    // Show timestamps only when stderr is a terminal — journald/pipes add their own
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stderr());

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(move |buf, record| {
            use std::io::Write;
            let level = record.level();
            let style = buf.default_level_style(level);
            if interactive {
                let now = chrono::Local::now();
                writeln!(
                    buf,
                    "[{} {style}{level:5}{style:#}] {}",
                    now.format("%H:%M:%S"),
                    record.args()
                )
            } else {
                writeln!(buf, "{style}{level:5}{style:#}: {}", record.args())
            }
        })
        .init();

    if args.daemon {
        return daemon::watch_loop(device.as_deref(), args.no_ping);
    }

    // One-shot mode
    let dev = hid::connect(device.as_deref(), args.no_ping)?;

    let now = chrono::Local::now();
    let vol = volume::get_volume()?;
    let payload = protocol::build_sysparam_payload(vol, &now);

    info!("Device: {} ({})", dev.path.display(), dev.protocol.label());
    info!("Time:   {}", now.format("%Y-%m-%d %H:%M:%S"));
    info!("Volume: {vol}%");
    debug!("{}", dev.format_packet(&payload));

    let acked = dev.send_sysparam(&payload)?;
    if acked {
        info!("Synced!");
    } else {
        warn!("No response from keyboard (sent anyway)");
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::NoDevice) => {
            error!("No responding keyboard found");
            ExitCode::from(1)
        }
        Err(Error::PermissionDenied { path }) => {
            error!("Permission denied on {}", path.display());
            error!("Fix: add udev rule or run as root");
            ExitCode::from(1)
        }
        Err(e) => {
            error!("{e}");
            ExitCode::from(3)
        }
    }
}
