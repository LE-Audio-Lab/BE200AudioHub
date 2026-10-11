namespace LEAudioRelay.Supervision;

internal enum SupervisorState
{
    Idle,
    Suspended,
    WaitingForEndpoint,
    TopologyBlocked,
    Starting,
    Running,
    RestartRequested,
    Restarting,
    RecoveringFault,
    Stopped
}
