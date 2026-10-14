using System.Runtime.Versioning;
using NAudio.CoreAudioApi;
using NAudio.Wave;

namespace LEAudioRelay.Routing;

internal delegate void RouteCapturePacketHandler(
    ReadOnlySpan<byte> buffer,
    AudioClientBufferFlags flags,
    long devicePosition,
    long qpcPosition);

[SupportedOSPlatform("windows")]
internal sealed class ProcessLoopbackSource :
    IDisposable
{
    private readonly WasapiRecorder _recorder;

    private bool _started;

    private ProcessLoopbackSource(
        WasapiRecorder recorder)
    {
        _recorder = recorder;

        _recorder.DataAvailable +=
            OnDataAvailable;

        _recorder.RecordingStopped +=
            OnRecordingStopped;
    }

    public event RouteCapturePacketHandler? Packet;

    public event Action<Exception?>? Stopped;

    public static async Task<ProcessLoopbackSource>
        CreateExcludeCurrentProcessTreeAsync(
            WaveFormat format,
            int bufferMilliseconds)
    {
#pragma warning disable CA1416
        WasapiRecorder recorder =
            await new WasapiRecorderBuilder()
                .WithProcessLoopback(
                    (uint)Environment.ProcessId,
                    ProcessLoopbackMode.ExcludeTargetProcessTree)
                .WithFormat(format)
                .WithSharedMode()
                .WithEventSync()
                .WithBufferLength(
                    bufferMilliseconds)
                .WithMmcssThreadPriority(
                    "Pro Audio")
                .BuildAsync();
#pragma warning restore CA1416

        return new ProcessLoopbackSource(
            recorder);
    }

    public void Start()
    {
        if (_started)
        {
            return;
        }

        _recorder.StartRecording();
        _started = true;
    }

    public void Stop()
    {
        if (!_started)
        {
            return;
        }

        _recorder.StopRecording();
        _started = false;
    }

    public void Dispose()
    {
        _recorder.DataAvailable -=
            OnDataAvailable;

        _recorder.RecordingStopped -=
            OnRecordingStopped;

        try
        {
            Stop();
        }
        catch
        {
        }

        _recorder.Dispose();
    }

    private void OnDataAvailable(
        ReadOnlySpan<byte> buffer,
        AudioClientBufferFlags flags,
        long devicePosition,
        long qpcPosition) =>
        Packet?.Invoke(
            buffer,
            flags,
            devicePosition,
            qpcPosition);

    private void OnRecordingStopped(
        object? sender,
        StoppedEventArgs e)
    {
        _started = false;

        Stopped?.Invoke(
            e.Exception);
    }
}
