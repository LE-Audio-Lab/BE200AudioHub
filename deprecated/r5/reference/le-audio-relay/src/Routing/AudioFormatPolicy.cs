using NAudio.CoreAudioApi;
using NAudio.Wave;

namespace LEAudioRelay.Routing;

internal static class AudioFormatPolicy
{
    public const int SampleRate = 48_000;
    public const int Channels = 2;

    private static readonly Guid IeeeFloatSubFormat =
        new(
            "00000003-0000-0010-8000-00AA00389B71");

    public static WaveFormat CreateRouteFormat() =>
        WaveFormat.CreateIeeeFloatWaveFormat(
            SampleRate,
            Channels);

    public static void ValidateDestination(
        MMDevice destination)
    {
        using var client =
            destination.CreateAudioClient();

        WaveFormat mix =
            client.MixFormat;

        bool float32 =
            mix.Encoding ==
                WaveFormatEncoding.IeeeFloat &&
            mix.BitsPerSample == 32;

        if (mix is WaveFormatExtensible extensible)
        {
            float32 =
                extensible.SubFormat ==
                    IeeeFloatSubFormat &&
                mix.BitsPerSample == 32;
        }

        if (mix.SampleRate != SampleRate ||
            mix.Channels != Channels ||
            !float32)
        {
            throw new InvalidOperationException(
                "Destination must expose a 48 kHz Float32 stereo MixFormat. " +
                $"Actual: {mix}");
        }
    }
}
