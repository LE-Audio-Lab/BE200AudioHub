using System.Text.Json;

namespace LEAudioRelay.Supervision;

internal static class WorkerProtocol
{
    public const string Hello = "hello";
    public const string Running = "running";
    public const string Heartbeat = "heartbeat";
    public const string Stopping = "stopping";
    public const string Stopped = "stopped";
    public const string Faulted = "faulted";
    public const string Shutdown = "shutdown";

    public static string Serialize(WorkerEnvelope message) =>
        JsonSerializer.Serialize(message);

    public static WorkerEnvelope Deserialize(string line) =>
        JsonSerializer.Deserialize<WorkerEnvelope>(line)
        ?? throw new InvalidDataException("Worker protocol message was empty.");
}

internal sealed record WorkerEnvelope(
    string Type,
    long Generation,
    string? Detail = null);
