#[macro_use]
mod console_ascii;
mod audio_set;
mod ble_scan;
mod boot;
mod controller_bootstrap;
mod driver;
mod error;
mod firmware;
mod hci;
mod hci_capabilities;
mod hci_wire;
mod intel;
mod logging;
mod pairing;
mod r2_attach;
mod usb;

use error::{AppError, Result};
use std::env;

fn usage() -> &'static str {
    "BE200AudioHub v0.1\n\
     Usage:\n\
       be200hub doctor\n\
       be200hub usb list [--target-only]\n\
       be200hub usb inspect [VID:PID]\n\
       be200hub usb probe\n\
       be200hub hci version\n\
       be200hub hci capabilities\n\
       be200hub ble scan --seconds <1..30> [--show-addresses]\n\
       be200hub r2 attach-probe\n\
       be200hub r2 scan --seconds <1..30> [--show-addresses]\n\
       be200hub r2 scan-forensics --seconds <1..30>\n\
       be200hub r2 pair reset --confirm\n\
       be200hub r2 pair onboard --seconds <1..30> --name <DEVICE_NAME>\n\
       be200hub r2 pair verify --seconds <1..30> --name <DEVICE_NAME>\n\
       be200hub r3 audio-set inspect --seconds <1..30> --name <DEVICE_NAME>\n\
       be200hub r4 codec-plan --seconds <1..30> --name <DEVICE_NAME>\n\
       be200hub firmware inspect <SFI_PATH> [DDC_PATH]\n\
       be200hub firmware plan --sfi <PATH> --ddc <PATH>\n\
       be200hub firmware preflight --sfi <PATH> --ddc <PATH>\n\
       be200hub firmware boot --sfi <PATH> --ddc <PATH> --confirm-cold-boot\n\
       be200hub driver status\n\
       be200hub driver verify\n\
       be200hub driver backup [--out DIRECTORY]\n\
       be200hub help\n\
     BE200 default target: 8087:0036\n\
     R2/R3 bootstrap: Operational is reused; Bootloader is initialized once from .\\firmware.\n\
     Pairing route: discover -> FAL ACL -> provisional SMP -> CSIS gate -> atomic commit.\n\
     R3 route: one scan -> dual bonded ACL -> PACS/ASCS/CAS read-only discovery.\n\
     Logging: %LOCALAPPDATA%\\BE200AudioHub\\logs\\be200hub.log; BE200_LOG=debug"
}

fn doctor() -> Result<()> {
    println!("BE200AudioHub 0.1.0 - read-only diagnostics");
    println!("OS: {}", env::consts::OS);
    println!("Architecture: {}", env::consts::ARCH);
    println!("Target: {:04x}:{:04x}", usb::BE200_VID, usb::BE200_PID);
    println!("USB backend: libusb/rusb (vendored)");
    println!("Log directory: {}", logging::default_directory().display());
    println!("BE200 status:");
    usb::list(true)
}

fn run_operational(action: impl FnOnce() -> Result<()>) -> Result<()> {
    controller_bootstrap::ensure_operational()?;
    action()
}

fn run(args: &[String]) -> Result<()> {
    match args {
        [x] if x == "help" || x == "--help" || x == "-h" => {
            println!("{}", usage());
            Ok(())
        }
        [x] if x == "doctor" => doctor(),

        [a, b] if a == "usb" && b == "list" => usb::list(false),
        [a, b, c] if a == "usb" && b == "list" && c == "--target-only" => usb::list(true),
        [a, b] if a == "usb" && b == "inspect" => {
            usb::inspect(usb::BE200_VID, usb::BE200_PID)
        }
        [a, b, value] if a == "usb" && b == "inspect" => {
            let (vid, pid) = usb::parse_vid_pid(value)?;
            usb::inspect(vid, pid)
        }
        [a, b] if a == "usb" && b == "probe" => {
            driver::verify()?;
            usb::probe_winusb()
        }

        [a, b] if a == "hci" && b == "version" => {
            driver::verify()?;
            hci::read_intel_version()
        }
        [a, b] if a == "hci" && b == "capabilities" => hci_capabilities::query(),

        [a, b, c, seconds] if a == "ble" && b == "scan" && c == "--seconds" => {
            ble_scan::scan(ble_scan::parse_seconds(seconds)?, false)
        }
        [a, b, c, seconds, flag]
            if a == "ble"
                && b == "scan"
                && c == "--seconds"
                && flag == "--show-addresses" =>
        {
            ble_scan::scan(ble_scan::parse_seconds(seconds)?, true)
        }

        [a, b] if a == "r2" && b == "attach-probe" => {
            run_operational(r2_attach::probe)
        }
        [a, b, c, seconds] if a == "r2" && b == "scan" && c == "--seconds" => {
            let seconds = r2_attach::parse_scan_seconds(seconds)?;
            run_operational(|| r2_attach::scan(seconds, false))
        }
        [a, b, c, seconds, flag]
            if a == "r2"
                && b == "scan"
                && c == "--seconds"
                && flag == "--show-addresses" =>
        {
            let seconds = r2_attach::parse_scan_seconds(seconds)?;
            run_operational(|| r2_attach::scan(seconds, true))
        }
        [a, b, c, seconds]
            if a == "r2" && b == "scan-forensics" && c == "--seconds" =>
        {
            let seconds = r2_attach::parse_scan_seconds(seconds)?;
            run_operational(|| r2_attach::scan_forensics(seconds))
        }
        [a, b, c, flag]
            if a == "r2" && b == "pair" && c == "reset" && flag == "--confirm" =>
        {
            run_operational(|| pairing::archive_reset(true))
        }
        [a, b, c, d, seconds, e, name]
            if a == "r2"
                && b == "pair"
                && c == "onboard"
                && d == "--seconds"
                && e == "--name" =>
        {
            let seconds = pairing::parse_onboard_seconds(seconds)?;
            run_operational(|| pairing::onboard(seconds, name))
        }
        [a, b, c, d, seconds, e, name]
            if a == "r2"
                && b == "pair"
                && c == "verify"
                && d == "--seconds"
                && e == "--name" =>
        {
            let seconds = pairing::parse_verify_seconds(seconds)?;
            run_operational(|| pairing::verify(seconds, name))
        }

        [a, b, c, d, seconds, e, name]
            if a == "r3"
                && b == "audio-set"
                && c == "inspect"
                && d == "--seconds"
                && e == "--name" =>
        {
            let seconds = audio_set::parse_seconds(seconds)?;
            run_operational(|| audio_set::inspect(seconds, name))
        }

        [a, b, c, d, seconds, e, name]
            if a == "r4"
                && b == "codec-plan"
                && c == "--seconds"
                && d == "--name" =>
        {
            let seconds = audio_set::parse_seconds(seconds)?;
            run_operational(|| audio_set::codec_plan(seconds, name))
        }

        [a, b, c, sfi] if a == "firmware" && b == "inspect" && c == "--sfi" => {
            firmware::inspect(sfi, None)
        }
        [a, b, c, sfi, d, ddc]
            if a == "firmware" && b == "inspect" && c == "--sfi" && d == "--ddc" =>
        {
            firmware::inspect(sfi, Some(ddc))
        }
        [a, b, sfi] if a == "firmware" && b == "inspect" => firmware::inspect(sfi, None),
        [a, b, sfi, ddc] if a == "firmware" && b == "inspect" => {
            firmware::inspect(sfi, Some(ddc))
        }
        [a, b, c, sfi, d, ddc]
            if a == "firmware" && b == "plan" && c == "--sfi" && d == "--ddc" =>
        {
            firmware::dry_run(sfi, ddc)
        }
        [a, b, c, sfi, d, ddc]
            if a == "firmware" && b == "preflight" && c == "--sfi" && d == "--ddc" =>
        {
            boot::preflight(sfi, ddc)
        }
        [a, b, c, sfi, d, ddc, flag]
            if a == "firmware"
                && b == "boot"
                && c == "--sfi"
                && d == "--ddc"
                && flag == "--confirm-cold-boot" =>
        {
            boot::boot_once(sfi, ddc)
        }

        [a, b] if a == "driver" && b == "status" => driver::status(),
        [a, b] if a == "driver" && b == "verify" => driver::verify(),
        [a, b] if a == "driver" && b == "backup" => driver::backup(None),
        [a, b, c, output] if a == "driver" && b == "backup" && c == "--out" => {
            driver::backup(Some(output))
        }

        _ => Err(AppError::Usage(usage().into())),
    }
}

fn main() {
    let logger = match logging::install(&logging::default_directory()) {
        Ok(runtime) => Some(runtime),
        Err(error) => {
            eprintln!("Warning: file logging unavailable: {error}");
            None
        }
    };

    let args: Vec<String> = env::args().skip(1).collect();
    log::info!("BE200AudioHub invoked: {:?}", args);
    match run(&args) {
        Ok(()) => log::info!("command completed successfully"),
        Err(error) => {
            eprintln!("Error: {error}");
            log::error!("{error}");
            if let Some(logger) = logger.as_ref() {
                logger.flush();
            }
            std::process::exit(1);
        }
    }
    if let Some(logger) = logger.as_ref() {
        logger.flush();
        log::debug!("Log file: {}", logger.path().display());
    }
}
