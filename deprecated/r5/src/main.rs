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
       be200hub r4 config-codec --seconds <1..30> --name <DEVICE_NAME> --side <left|right|both> --confirm-ascs-write\n\
       be200hub r5 qos-plan --seconds <1..30> --name <DEVICE_NAME> --confirm-ascs-write\n\
       be200hub r5 config-qos --seconds <1..30> --name <DEVICE_NAME> --confirm-cig-qos-write\n\
       be200hub r5 establish-cis --seconds <1..30> --name <DEVICE_NAME> --confirm-cis-create\n\
       be200hub r5 iso-path-probe --seconds <1..30> --name <DEVICE_NAME> --confirm-iso-path\n\
       be200hub r5 lc3-self-test\n\
       be200hub r5 first-sound --seconds <1..30> --name <DEVICE_NAME> --play-seconds <1..30> --confirm-first-sound\n\
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

#[derive(Debug, PartialEq, Eq)]
enum R4Invocation<'a> {
    CodecPlan { seconds: u16, name: &'a str },
    ConfigCodec {
        seconds: u16,
        name: &'a str,
        scope: bap_client::CodecConfigScope,
    },
}

/// Pure parser used before the hardware bootstrap. Regression-tested so a
/// help-listed command cannot silently fall through to Usage again.
fn parse_r4_invocation(args: &[String]) -> Result<Option<R4Invocation<'_>>> {
    match args {
        [a, b, seconds_flag, seconds, name_flag, name]
            if a == "r4"
                && b == "codec-plan"
                && seconds_flag == "--seconds"
                && name_flag == "--name" =>
        {
            Ok(Some(R4Invocation::CodecPlan {
                seconds: audio_set::parse_seconds(seconds)?,
                name: name.as_str(),
            }))
        }
        [a, b, seconds_flag, seconds, name_flag, name, side_flag, side, confirm]
            if a == "r4"
                && b == "config-codec"
                && seconds_flag == "--seconds"
                && name_flag == "--name"
                && side_flag == "--side"
                && confirm == "--confirm-ascs-write" =>
        {
            Ok(Some(R4Invocation::ConfigCodec {
                seconds: audio_set::parse_seconds(seconds)?,
                name: name.as_str(),
                scope: audio_set::parse_config_scope(side)?,
            }))
        }
        _ => Ok(None),
    }
}

/// R5.1 pure CLI parse: intentionally requires R4.2 Config Codec consent.
/// The R5 path cannot issue ASCS Config QoS or HCI Set CIG Parameters.
fn parse_r5_qos_invocation(args: &[String]) -> Result<Option<(u16, &str)>> {
    match args {
        [r5, command, seconds_flag, seconds, name_flag, name, confirm]
            if r5 == "r5"
                && command == "qos-plan"
                && seconds_flag == "--seconds"
                && name_flag == "--name"
                && confirm == "--confirm-ascs-write" =>
        {
            Ok(Some((audio_set::parse_seconds(seconds)?, name.as_str())))
        }
        _ => Ok(None),
    }
}

/// R5.2-A performs controller and ASCS writes; require a distinct, explicit
/// confirmation token so a R5.1 dry-run cannot accidentally allocate a CIG.
fn parse_r5_config_qos_invocation(args: &[String]) -> Result<Option<(u16, &str)>> {
    match args {
        [r5, command, seconds_flag, seconds, name_flag, name, confirm]
            if r5 == "r5"
                && command == "config-qos"
                && seconds_flag == "--seconds"
                && name_flag == "--name"
                && confirm == "--confirm-cig-qos-write" =>
        {
            Ok(Some((audio_set::parse_seconds(seconds)?, name.as_str())))
        }
        _ => Ok(None),
    }
}

/// R5.2-B intentionally creates two CIS links and must not reuse the
/// R5.2-A consent flag. Exact seven-token parser, pre-bootstrap validation.
fn parse_r5_establish_cis_invocation(args: &[String]) -> Result<Option<(u16, &str)>> {
    match args {
        [r5, command, seconds_flag, seconds, name_flag, name, confirm]
            if r5 == "r5"
                && command == "establish-cis"
                && seconds_flag == "--seconds"
                && name_flag == "--name"
                && confirm == "--confirm-cis-create" =>
        {
            Ok(Some((audio_set::parse_seconds(seconds)?, name.as_str())))
        }
        _ => Ok(None),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum R53Invocation<'a> {
    CodecSelfTest,
    IsoPaths { seconds: u16, name: &'a str },
    FirstSound { seconds: u16, name: &'a str, play_seconds: u16 },
}

/// Distinct consent flags and full argument validation BEFORE any USB bootstrap.
fn parse_r53_invocation(args: &[String]) -> Result<Option<R53Invocation<'_>>> {
    let valid_name = |name: &str| -> Result<()> {
        if name.trim().is_empty() { Err(AppError::Usage("--name requires a nonempty device name".into())) } else { Ok(()) }
    };
    match args {
        [r5, command] if r5 == "r5" && command == "lc3-self-test" => Ok(Some(R53Invocation::CodecSelfTest)),
        [r5, command, seconds_flag, seconds, name_flag, name, confirm]
            if r5 == "r5" && command == "iso-path-probe" && seconds_flag == "--seconds"
                && name_flag == "--name" && confirm == "--confirm-iso-path" => {
            valid_name(name)?;
            Ok(Some(R53Invocation::IsoPaths { seconds: audio_set::parse_seconds(seconds)?, name }))
        }
        [r5, command, seconds_flag, seconds, name_flag, name, play_flag, play, confirm]
            if r5 == "r5" && command == "first-sound" && seconds_flag == "--seconds"
                && name_flag == "--name" && play_flag == "--play-seconds" && confirm == "--confirm-first-sound" => {
            valid_name(name)?;
            let play_seconds: u16 = play.parse().map_err(|_| AppError::Usage("--play-seconds must be an integer (1..=30)".into()))?;
            audio_bridge::first_sound::validate_play_seconds(play_seconds).map_err(AppError::Usage)?;
            Ok(Some(R53Invocation::FirstSound {seconds:audio_set::parse_seconds(seconds)?,name,play_seconds}))
        }
        _ => Ok(None),
    }
}

fn run_codec_self_test() -> Result<()> {
    let report = audio_bridge::first_sound::first_sound_codec_self_test().map_err(AppError::Protocol)?;
    println!("[R5.3-B/LC3] offline encode/decode PASS: frames={} left_RMS={:.1} right_RMS={:.1} tone_power_ratios={:.1}/{:.1}; no USB/bond access",
        report.stereo_frames, report.left_rms, report.right_rms, report.left_440_over_880, report.right_880_over_440);
    Ok(())
}

fn run(args: &[String]) -> Result<()> {
    if let Some(invocation) = parse_r53_invocation(args)? {
        return match invocation {
            R53Invocation::CodecSelfTest => run_codec_self_test(),
            R53Invocation::IsoPaths { seconds, name } => {
                let _guard = bumble_adapter::ProbeGuard::install().map_err(AppError::Protocol)?;
                run_operational(|| audio_set::iso_paths(seconds, name))
            }
            R53Invocation::FirstSound { seconds, name, play_seconds } => {
                let _guard = bumble_adapter::ProbeGuard::install().map_err(AppError::Protocol)?;
                println!("[R5.3-B/PREP] offline LC3 self-test and {}s bounded stereo pre-encode before hardware bootstrap", play_seconds);
                run_codec_self_test()?;
                let tone = audio_bridge::first_sound::prepare_first_sound(play_seconds).map_err(AppError::Protocol)?;
                println!("[R5.3-B/PREP] READY: {} stereo frame pairs, 100B/ear/10ms", tone.frames().len());
                bumble_adapter::ProbeGuard::check_cancelled().map_err(AppError::Protocol)?;
                run_operational(|| audio_set::first_sound(seconds, name, &tone))
            }
        };
    }
    if let Some((seconds, name)) = parse_r5_establish_cis_invocation(args)? {
        return run_operational(|| audio_set::establish_cis(seconds, name));
    }
    if let Some((seconds, name)) = parse_r5_config_qos_invocation(args)? {
        return run_operational(|| audio_set::config_qos(seconds, name));
    }
    if let Some((seconds, name)) = parse_r5_qos_invocation(args)? {
        return run_operational(|| audio_set::qos_plan(seconds, name));
    }
    if let Some(invocation) = parse_r4_invocation(args)? {
        return match invocation {
            R4Invocation::CodecPlan { seconds, name } =>
                run_operational(|| audio_set::codec_plan(seconds, name)),
            R4Invocation::ConfigCodec { seconds, name, scope } =>
                run_operational(|| audio_set::config_codec(seconds, name, scope)),
        };
    }
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

#[cfg(test)]
mod r4_cli_tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn codec_plan_six_tokens_resolve_without_bootstrapping() {
        let args = args(&[
            "r4", "codec-plan", "--seconds", "8",
            "--name", "Galaxy Buds3 Pro (2F23)"
        ]);
        assert_eq!(parse_r4_invocation(&args).unwrap(),
            Some(R4Invocation::CodecPlan {
                seconds: 8, name: "Galaxy Buds3 Pro (2F23)"
            }));
    }

    #[test]
    fn config_codec_nine_tokens_require_explicit_confirmation() {
        let mut args = args(&[
            "r4", "config-codec", "--seconds", "8",
            "--name", "Galaxy Buds3 Pro (2F23)",
            "--side", "both", "--confirm-ascs-write"
        ]);
        assert_eq!(parse_r4_invocation(&args).unwrap(),
            Some(R4Invocation::ConfigCodec {
                seconds: 8, name: "Galaxy Buds3 Pro (2F23)",
                scope: bap_client::CodecConfigScope::Both
            }));
        args.pop();
        assert_eq!(parse_r4_invocation(&args).unwrap(), None);
    }

    #[test]
    fn bad_side_and_bad_seconds_are_rejected_before_hardware_access() {
        let mut args = args(&[
            "r4", "config-codec", "--seconds", "8",
            "--name", "Galaxy Buds3 Pro (2F23)",
            "--side", "none", "--confirm-ascs-write"
        ]);
        assert!(parse_r4_invocation(&args).is_err());
        args[7] = "left".to_owned();
        args[3] = "0".to_owned();
        assert!(parse_r4_invocation(&args).is_err());
    }

    #[test]
    fn r5_cis_create_requires_unique_consent_and_exact_seven_tokens() {
        let mut command = args(&[
            "r5", "establish-cis", "--seconds", "8",
            "--name", "Galaxy Buds3 Pro (2F23)", "--confirm-cis-create"
        ]);
        assert_eq!(parse_r5_establish_cis_invocation(&command).unwrap(),
            Some((8, "Galaxy Buds3 Pro (2F23)")));
        command[6] = "--confirm-cig-qos-write".into();
        assert_eq!(parse_r5_establish_cis_invocation(&command).unwrap(), None);
        command[6] = "--confirm-cis-create".into();
        command.push("extra".into());
        assert_eq!(parse_r5_establish_cis_invocation(&command).unwrap(), None);
        command.pop();
        command[3] = "31".into();
        assert!(parse_r5_establish_cis_invocation(&command).is_err());
    }

    #[test]
    fn r5_config_qos_requires_distinct_confirmation_and_exact_arity() {
        let mut command = args(&[
            "r5", "config-qos", "--seconds", "8",
            "--name", "Galaxy Buds3 Pro (2F23)", "--confirm-cig-qos-write"
        ]);
        assert_eq!(parse_r5_config_qos_invocation(&command).unwrap(),
            Some((8, "Galaxy Buds3 Pro (2F23)")));
        command[6] = "--confirm-ascs-write".into();
        assert_eq!(parse_r5_config_qos_invocation(&command).unwrap(), None);
        command[6] = "--confirm-cig-qos-write".into();
        command.push("extra".into());
        assert_eq!(parse_r5_config_qos_invocation(&command).unwrap(), None);
        command.pop();
        command[3] = "0".into();
        assert!(parse_r5_config_qos_invocation(&command).is_err());
    }

    #[test]
    fn r5_qos_plan_requires_seven_tokens_and_write_confirmation() {
        let mut command = args(&[
            "r5", "qos-plan", "--seconds", "8",
            "--name", "Galaxy Buds3 Pro (2F23)", "--confirm-ascs-write"
        ]);
        assert_eq!(
            parse_r5_qos_invocation(&command).unwrap(),
            Some((8, "Galaxy Buds3 Pro (2F23)"))
        );
        command.pop();
        assert_eq!(parse_r5_qos_invocation(&command).unwrap(), None);
        command.push("--confirm-ascs-write".into());
        command[3] = "31".into();
        assert!(parse_r5_qos_invocation(&command).is_err());
    }
}

#[cfg(test)]
mod r53_cli_tests {
    use super::*;
    fn args(words: &[&str]) -> Vec<String> { words.iter().map(|s|s.to_string()).collect() }
    #[test]
    fn first_sound_requires_its_own_consent_and_valid_duration_before_bootstrap() {
        let mut a=args(&["r5","first-sound","--seconds","8","--name","Buds","--play-seconds","8","--confirm-first-sound"]);
        assert_eq!(parse_r53_invocation(&a).unwrap(),Some(R53Invocation::FirstSound {seconds:8,name:"Buds",play_seconds:8}));
        a[8]="--confirm-cis-create".into(); assert_eq!(parse_r53_invocation(&a).unwrap(),None);
        a[8]="--confirm-first-sound".into();
        for bad in ["0","31","-1","abc"] { a[7]=bad.into(); assert!(parse_r53_invocation(&a).is_err()); }
        a[7]="8".into(); a[5]=" ".into(); assert!(parse_r53_invocation(&a).is_err());
    }
    #[test]
    fn paths_probe_is_separate_and_codec_test_is_hardware_free() {
        assert_eq!(parse_r53_invocation(&args(&["r5","lc3-self-test"])).unwrap(),Some(R53Invocation::CodecSelfTest));
        let mut a=args(&["r5","iso-path-probe","--seconds","8","--name","Buds","--confirm-iso-path"]);
        assert_eq!(parse_r53_invocation(&a).unwrap(),Some(R53Invocation::IsoPaths {seconds:8,name:"Buds"}));
        a[6]="--confirm-first-sound".into(); assert_eq!(parse_r53_invocation(&a).unwrap(),None);
        a.push("extra".into()); assert_eq!(parse_r53_invocation(&a).unwrap(),None);
    }
}
