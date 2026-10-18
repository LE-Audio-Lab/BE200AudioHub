#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryClass {
    RestartMedia,
    RebuildStream,
    RestartController,
    RestartWorker,
}
