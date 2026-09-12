use super::{
    process::{owned_command, probe_output, ManagedChild},
    util::external_error,
    Error, Result,
};
use std::{
    env, fs,
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use termtrace_evidence::artifact::{secure_file, write_secure};
use termtrace_model::artifact::visual::CaptureOptions;

pub(super) fn require_tools() -> Result<()> {
    if env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err(external_error(
            "Wayland capture",
            "WAYLAND_DISPLAY is not set",
        ));
    }

    probe_output(
        "Wayland capture dependencies",
        Command::new("/usr/bin/python3").args([
            "-c",
            "import gi; gi.require_version('Gio', '2.0'); gi.require_version('Gst', '1.0'); gi.require_version('GstVideo', '1.0'); from gi.repository import Gst; Gst.init(None); assert all(Gst.ElementFactory.find(n) for n in ['pipewiresrc', 'videoconvert', 'videoscale', 'x264enc', 'matroskamux']), 'missing GStreamer capture plugin'",
        ]),
        Duration::from_secs(10),
    )?;

    return Ok(());
}

pub(super) fn spawn(
    helper: &Path,
    options: &CaptureOptions,
    staging: &Path,
) -> Result<ManagedChild> {
    let script = staging.join("portal.py");
    write_secure(&script, include_bytes!("portal.py"))?;
    let log = secure_file(&staging.join("portal.log"))?;

    return ManagedChild::spawn_logged(
        "Wayland portal capture",
        owned_command(helper, "/usr/bin/python3")?
            .arg(&script)
            .arg(staging)
            .arg(options.width.to_string())
            .arg(options.height.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log)),
    );
}

pub(super) fn wait_for_acquisition(
    capture: &mut ManagedChild,
    ghostty: &mut ManagedChild,
    staging: &Path,
    cancelled: &AtomicBool,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(130);

    while !staging.join("acquisition-ready.json").is_file() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(external_error("Wayland capture", "capture cancelled"));
        }

        capture.check_running()?;
        ghostty.check_running()?;

        if Instant::now() >= deadline {
            return Err(external_error(
                "Wayland capture",
                "timed out waiting for portal selection and first frame",
            ));
        }

        thread::sleep(Duration::from_millis(20));
    }

    return Ok(());
}

#[derive(serde::Deserialize)]
struct AcquisitionCompletion {
    frames: u64,
    first_received_unix_ns: u128,
}

pub(super) fn validate_completion(staging: &Path, packets: u64) -> Result<u128> {
    let path = staging.join("acquisition-completion.json");
    let bytes = fs::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;

    let completion: AcquisitionCompletion = serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path,
            json_path: "$".to_owned(),
            source,
        };
    })?;

    if packets == 0 || packets != completion.frames {
        return Err(external_error(
            "Wayland capture",
            "acquired frame count differs from encoded packet count",
        ));
    }
    return Ok(completion.first_received_unix_ns);
}
