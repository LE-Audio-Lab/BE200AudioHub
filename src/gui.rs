#![cfg(windows)]

use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread::{self, JoinHandle},
    time::Duration,
};

use eframe::egui;

use crate::{
    plane::l1_control::{
        engine::machine::TeardownPolicy,
        planning::iso::StreamLatencyProfile,
    },
    review::round8::live_relay::{
        execute_live_relay_until_stop, LiveRelayRuntimeControl,
        LiveRelaySessionConfig,
    },
};

const DEFAULT_CONNECT_SECONDS: u64 = 12;
const DEFAULT_VOLUME_PERCENT: f32 = 100.0;

#[derive(Clone, Debug)]
pub struct GuiConfig {
    pub session: LiveRelaySessionConfig,
}

pub fn parse_gui_args(args: &[String]) -> Result<GuiConfig, String> {
    let mut sfi = None;
    let mut ddc = None;
    let mut bond_root = None;
    let mut target_name = None;
    let mut connect_timeout =
        Duration::from_secs(DEFAULT_CONNECT_SECONDS);
    let mut ungraceful_teardown = false;
    let mut extreme_low_latency = false;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--sfi" | "--ddc" | "--bond-root" | "--name"
            | "--connect-seconds" => {
                let flag = args[index].clone();
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| format!("missing value for {flag}"))?;

                match flag.as_str() {
                    "--sfi" => sfi = Some(PathBuf::from(value)),
                    "--ddc" => ddc = Some(PathBuf::from(value)),
                    "--bond-root" => {
                        bond_root = Some(PathBuf::from(value))
                    }
                    "--name" => {
                        if value.trim().is_empty() {
                            return Err("--name must not be empty".into());
                        }
                        target_name = Some(value.clone());
                    }
                    "--connect-seconds" => {
                        let seconds: u64 = value.parse().map_err(|_| {
                            "--connect-seconds must be an integer".to_string()
                        })?;
                        if !(3..=30).contains(&seconds) {
                            return Err(
                                "--connect-seconds must be in 3..=30".into(),
                            );
                        }
                        connect_timeout = Duration::from_secs(seconds);
                    }
                    _ => unreachable!(),
                }
                index += 2;
            }
            "--ungraceful-teardown" => {
                if ungraceful_teardown {
                    return Err(
                        "--ungraceful-teardown specified more than once"
                            .into(),
                    );
                }
                ungraceful_teardown = true;
                index += 1;
            }
            "--extreme-low-latency" => {
                if extreme_low_latency {
                    return Err(
                        "--extreme-low-latency specified more than once"
                            .into(),
                    );
                }
                extreme_low_latency = true;
                index += 1;
            }
            unknown => {
                return Err(format!("unknown GUI argument: {unknown}"));
            }
        }
    }

    Ok(GuiConfig {
        session: LiveRelaySessionConfig {
            sfi: sfi.ok_or_else(|| "--sfi is required".to_string())?,
            ddc: ddc.ok_or_else(|| "--ddc is required".to_string())?,
            bond_root: bond_root
                .ok_or_else(|| "--bond-root is required".to_string())?,
            target_name: target_name
                .ok_or_else(|| "--name is required".to_string())?,
            connect_timeout,
            teardown_policy: if ungraceful_teardown {
                TeardownPolicy::ControllerCleanUngraceful
            } else {
                TeardownPolicy::Graceful
            },
            latency_profile: if extreme_low_latency {
                StreamLatencyProfile::ExtremeLowLatency
            } else {
                StreamLatencyProfile::Daily
            },
        },
    })
}

struct SessionWorker {
    control: LiveRelayRuntimeControl,
    result_rx: Receiver<Result<(), String>>,
    join: Option<JoinHandle<()>>,
}

impl SessionWorker {
    fn start(
        config: LiveRelaySessionConfig,
        volume_percent: f32,
    ) -> Self {
        let control =
            LiveRelayRuntimeControl::new(volume_percent / 100.0);
        let thread_control = control.clone();
        let (result_tx, result_rx) = mpsc::channel();

        let join = thread::Builder::new()
            .name("be200-gui-session".into())
            .spawn(move || {
                let result =
                    execute_live_relay_until_stop(config, thread_control);
                let _ = result_tx.send(result);
            })
            .expect("failed to spawn BE200 GUI session worker");

        Self {
            control,
            result_rx,
            join: Some(join),
        }
    }

    fn set_volume_percent(&self, volume_percent: f32) {
        self.control
            .gain()
            .set_linear((volume_percent / 100.0).clamp(0.0, 1.0));
    }

    fn request_stop(&self) {
        self.control.request_stop();
    }

    fn stop_requested(&self) -> bool {
        self.control.stop_requested()
    }

    fn try_result(&self) -> Option<Result<(), String>> {
        match self.result_rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(
                "GUI relay worker exited without a result".into(),
            )),
        }
    }

    fn join(&mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct RelayGuiApp {
    config: LiveRelaySessionConfig,
    volume_percent: f32,
    session: Option<SessionWorker>,
}

impl RelayGuiApp {
    fn new(config: LiveRelaySessionConfig) -> Self {
        Self {
            config,
            volume_percent: DEFAULT_VOLUME_PERCENT,
            session: None,
        }
    }

    fn poll_session(&mut self) {
        let Some(result) = self
            .session
            .as_ref()
            .and_then(SessionWorker::try_result)
        else {
            return;
        };

        let mut session = self
            .session
            .take()
            .expect("completed GUI session must still exist");
        session.join();

        if let Err(error) = result {
            eprintln!("BE200AudioHub GUI session failed: {error}");
        }
    }

    fn start_session(&mut self) {
        if self.session.is_none() {
            self.session = Some(SessionWorker::start(
                self.config.clone(),
                self.volume_percent,
            ));
        }
    }

    fn stop_session(&mut self) {
        if let Some(session) = &self.session {
            session.request_stop();
        }
    }
}

impl Drop for RelayGuiApp {
    fn drop(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.request_stop();
            session.join();
        }
    }
}

impl eframe::App for RelayGuiApp {
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _frame: &mut eframe::Frame,
    ) {
        self.poll_session();

        ui.add_space(18.0);
        ui.vertical_centered(|ui| {
            let slider = ui.add(
                egui::Slider::new(
                    &mut self.volume_percent,
                    0.0..=100.0,
                )
                .text("Volume")
                .suffix("%"),
            );

            if slider.changed() {
                if let Some(session) = &self.session {
                    session.set_volume_percent(self.volume_percent);
                }
            }

            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let running = self.session.is_some();
                if ui
                    .add_enabled(!running, egui::Button::new("Start"))
                    .clicked()
                {
                    self.start_session();
                }

                let can_stop = self
                    .session
                    .as_ref()
                    .map(|session| !session.stop_requested())
                    .unwrap_or(false);
                if ui
                    .add_enabled(can_stop, egui::Button::new("Stop"))
                    .clicked()
                {
                    self.stop_session();
                }
            });
        });

        ui.ctx()
            .request_repaint_after(Duration::from_millis(50));
    }
}

pub fn run_gui(config: GuiConfig) -> Result<(), String> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([360.0, 150.0])
            .with_min_inner_size([320.0, 140.0])
            .with_resizable(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    let session = config.session;
    eframe::run_native(
        "BE200AudioHub",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(RelayGuiApp::new(session.clone())))
        }),
    )
    .map_err(|error| format!("egui startup failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Vec<String> {
        vec![
            "--sfi".into(),
            "a.sfi".into(),
            "--ddc".into(),
            "a.ddc".into(),
            "--bond-root".into(),
            "bonding".into(),
            "--name".into(),
            "Galaxy Buds3 Pro (2F23)".into(),
        ]
    }

    #[test]
    fn parser_defaults_to_graceful_teardown() {
        let config = parse_gui_args(&args()).unwrap();
        assert_eq!(
            config.session.teardown_policy,
            TeardownPolicy::Graceful
        );
        assert_eq!(
            config.session.connect_timeout,
            Duration::from_secs(DEFAULT_CONNECT_SECONDS)
        );
        assert_eq!(
            config.session.latency_profile,
            StreamLatencyProfile::Daily
        );
    }

    #[test]
    fn parser_accepts_extreme_low_latency_profile() {
        let mut value = args();
        value.push("--extreme-low-latency".into());
        let config = parse_gui_args(&value).unwrap();
        assert_eq!(
            config.session.latency_profile,
            StreamLatencyProfile::ExtremeLowLatency
        );
    }

    #[test]
    fn parser_accepts_explicit_be200_ungraceful_workaround() {
        let mut value = args();
        value.push("--ungraceful-teardown".into());
        let config = parse_gui_args(&value).unwrap();
        assert_eq!(
            config.session.teardown_policy,
            TeardownPolicy::ControllerCleanUngraceful
        );
    }
}
