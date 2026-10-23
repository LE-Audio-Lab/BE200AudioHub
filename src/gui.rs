#![cfg(windows)]

use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use eframe::egui;

use crate::{
    plane::l1_control::{
        engine::machine::TeardownPolicy,
        planning::iso::StreamLatencyProfile,
    },
    review::round8::live_relay::{
        execute_live_relay_until_stop, LiveRelayConditionSample,
        LiveRelayRuntimeControl, LiveRelaySessionConfig,
    },
};

const DEFAULT_CONNECT_SECONDS: u64 = 12;
const DEFAULT_VOLUME_PERCENT: f32 = 100.0;
const GUI_REPAINT_INTERVAL: Duration = Duration::from_millis(50);

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

    fn latest_condition_sample(
        &self,
    ) -> Option<LiveRelayConditionSample> {
        self.control.latest_condition_sample()
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
    last_condition_sample: Option<LiveRelayConditionSample>,
    last_condition_sample_seen_at: Option<Instant>,
    last_error: Option<String>,
}

impl RelayGuiApp {
    fn new(config: LiveRelaySessionConfig) -> Self {
        Self {
            config,
            volume_percent: DEFAULT_VOLUME_PERCENT,
            session: None,
            last_condition_sample: None,
            last_condition_sample_seen_at: None,
            last_error: None,
        }
    }

    fn poll_session(&mut self) {
        if let Some(sample) = self
            .session
            .as_ref()
            .and_then(SessionWorker::latest_condition_sample)
        {
            let is_new = self
                .last_condition_sample
                .map(|previous| previous.sequence != sample.sequence)
                .unwrap_or(true);
            if is_new {
                self.last_condition_sample = Some(sample);
                self.last_condition_sample_seen_at = Some(Instant::now());
            }
        }

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
            self.last_error = Some(error);
        } else {
            self.last_error = None;
        }
    }

    fn start_session(&mut self) {
        if self.session.is_none() {
            self.last_error = None;
            self.last_condition_sample = None;
            self.last_condition_sample_seen_at = None;
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


fn metric_row(
    ui: &mut egui::Ui,
    label: &str,
    value: impl Into<String>,
) {
    ui.label(label);
    ui.monospace(value.into());
    ui.end_row();
}

fn dashboard_card(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.strong(title);
            ui.add_space(6.0);
            add_contents(ui);
        });
}

fn render_condition_dashboard(
    ui: &mut egui::Ui,
    sample: LiveRelayConditionSample,
    seen_at: Option<Instant>,
) {
    let snapshot = sample.snapshot;
    let sample_age_ms = seen_at
        .map(|seen| seen.elapsed().as_millis())
        .unwrap_or(0);

    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Condition · 1 Hz snapshot");
                ui.separator();
                ui.monospace(format!("#{}", sample.sequence));
                ui.label(format!("epoch {}", snapshot.epoch));
                ui.label(format!("gen {}", snapshot.generation.0));
                ui.label(format!(
                    "lifecycle={:?}",
                    snapshot.lifecycle.lifecycle
                ));
                ui.label(format!(
                    "session={:?}",
                    snapshot.lifecycle.session
                ));
                ui.label(format!("age={} ms", sample_age_ms));
            });
        });

    ui.add_space(8.0);
    ui.columns(2, |columns| {
        dashboard_card(&mut columns[0], "Control", |ui| {
            egui::Grid::new("dashboard_control")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    metric_row(
                        ui,
                        "State",
                        format!("{:?}", snapshot.control.state),
                    );
                    metric_row(
                        ui,
                        "Stream",
                        snapshot.control.stream_id
                            .map(|id| id.0.to_string())
                            .unwrap_or_else(|| "-".into()),
                    );
                    if let Some(format) = snapshot.control.format {
                        metric_row(
                            ui,
                            "Format",
                            format!(
                                "{} Hz / {} us / {} B",
                                format.sample_rate_hz,
                                format.frame_duration_us,
                                format.octets_per_sdu,
                            ),
                        );
                    } else {
                        metric_row(ui, "Format", "-");
                    }
                });
        });

        dashboard_card(&mut columns[1], "HCI", |ui| {
            egui::Grid::new("dashboard_hci")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    metric_row(
                        ui,
                        "State",
                        format!("{:?}", snapshot.hci.state.state),
                    );
                    metric_row(
                        ui,
                        "Stream",
                        snapshot.hci.state.stream_id
                            .map(|id| id.0.to_string())
                            .unwrap_or_else(|| "-".into()),
                    );

                    if let Some(t) = snapshot.hci.iso_telemetry {
                        metric_row(
                            ui,
                            "ISO window",
                            t.controller_limit
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".into()),
                        );
                        metric_row(
                            ui,
                            "In flight",
                            format!(
                                "{} / peak {}",
                                t.in_flight_packets,
                                t.peak_in_flight_packets
                            ),
                        );
                        metric_row(
                            ui,
                            "Packets",
                            format!(
                                "tx {}  done {}  retired {}",
                                t.submitted_packets,
                                t.completed_packets,
                                t.retired_packets,
                            ),
                        );
                        metric_row(
                            ui,
                            "Trace",
                            format!(
                                "{}/{}",
                                t.trace.len, t.trace.capacity
                            ),
                        );
                    } else {
                        metric_row(ui, "Telemetry", "waiting");
                    }
                });
        });
    });

    ui.add_space(8.0);
    ui.columns(2, |columns| {
        dashboard_card(&mut columns[0], "Media", |ui| {
            egui::Grid::new("dashboard_media")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    metric_row(
                        ui,
                        "State",
                        format!("{:?}", snapshot.media.state.state),
                    );
                    metric_row(
                        ui,
                        "Clock",
                        format!("{:?}", snapshot.media.state.clock_lock),
                    );

                    if let Some(t) = snapshot.media.telemetry {
                        metric_row(
                            ui,
                            "Frame",
                            t.timing.produced_frame
                                .map(|frame| frame.0.to_string())
                                .unwrap_or_else(|| "-".into()),
                        );
                        metric_row(
                            ui,
                            "PCM fill",
                            format!("{} f", t.timing.pcm_fill_frames),
                        );
                        metric_row(
                            ui,
                            "Intervals",
                            t.timing.intervals_consumed.to_string(),
                        );
                        metric_row(
                            ui,
                            "Underflow",
                            t.timing.pcm_underflow_frames.to_string(),
                        );
                        metric_row(
                            ui,
                            "Source skip",
                            t.timing.source_intervals_skipped.to_string(),
                        );
                        metric_row(
                            ui,
                            "Trim",
                            format!(
                                "startup {}  gradual {}  emergency {}",
                                t.timing.startup_trim_frames,
                                t.timing.gradual_drift_trim_frames,
                                t.timing.emergency_drift_trim_frames,
                            ),
                        );
                        metric_row(
                            ui,
                            "Timing trace",
                            format!(
                                "{}/{}",
                                t.timing.trace.len,
                                t.timing.trace.capacity,
                            ),
                        );
                        metric_row(
                            ui,
                            "Bridge",
                            format!(
                                "tx {}  drop {}  fallback {}",
                                t.bridge.submitted_intervals,
                                t.bridge.dropped_backpressure_intervals,
                                t.bridge.fallback_intervals,
                            ),
                        );
                        metric_row(
                            ui,
                            "Bridge miss",
                            format!(
                                "skip {}  stale {}",
                                t.bridge.skipped_intervals,
                                t.bridge.stale_frames_dropped,
                            ),
                        );
                        metric_row(
                            ui,
                            "Bridge trace",
                            format!(
                                "{}/{}",
                                t.bridge.trace.len,
                                t.bridge.trace.capacity,
                            ),
                        );
                    } else {
                        metric_row(ui, "Telemetry", "waiting");
                    }
                });
        });

        dashboard_card(&mut columns[1], "Transport", |ui| {
            egui::Grid::new("dashboard_transport")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    metric_row(
                        ui,
                        "State",
                        format!("{:?}", snapshot.transport.state.state),
                    );
                    if let Some(t) = snapshot.transport.telemetry {
                        metric_row(
                            ui,
                            "RX packets",
                            t.rx_packets.to_string(),
                        );
                        metric_row(
                            ui,
                            "TX packets",
                            t.tx_packets.to_string(),
                        );
                        metric_row(
                            ui,
                            "Faults",
                            t.faults.to_string(),
                        );
                        metric_row(
                            ui,
                            "Trace",
                            format!(
                                "{}/{}",
                                t.trace.len, t.trace.capacity
                            ),
                        );
                    } else {
                        metric_row(ui, "Telemetry", "waiting");
                    }
                });
        });
    });
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

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
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

                ui.separator();
                ui.label(if running { "Session running" } else { "Session idle" });
            });

            if let Some(error) = &self.last_error {
                ui.add_space(8.0);
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Session error: {error}"),
                );
            }

            ui.add_space(10.0);
            match self.last_condition_sample {
                Some(sample) => render_condition_dashboard(
                    ui,
                    sample,
                    self.last_condition_sample_seen_at,
                ),
                None => {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.strong("Condition · 1 Hz snapshot");
                            ui.label(
                                if running {
                                    "Waiting for the first runtime snapshot…"
                                } else {
                                    "Start a session to populate the dashboard."
                                },
                            );
                        });
                }
            }

            ui.add_space(10.0);
        });

        ui.ctx().request_repaint_after(GUI_REPAINT_INTERVAL);
    }
}

pub fn run_gui(config: GuiConfig) -> Result<(), String> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([820.0, 620.0])
            .with_min_inner_size([640.0, 480.0])
            .with_resizable(true),
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
