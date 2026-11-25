import AVFoundation
import CrossfadeFFI
import Foundation

/// The device side of preview: an `AVAudioEngine` whose source node pulls deinterleaved
/// floats from the library's feed. Made, started, paused and stopped on the main actor; the
/// render block runs on the audio thread and does only what `cf_feed_fill_planar` allows.
final class AudioOutput {
    let sampleRate: Double
    private let engine: AVAudioEngine
    private let node: AVAudioSourceNode
    private let target: RenderTarget
    private var released = false

    /// Takes ownership of `feed`: it is freed by `release()`, after the engine has stopped.
    init(feed: OpaquePointer, sampleRate: Double, channels: Int) throws {
        guard let format = AVAudioFormat(
            standardFormatWithSampleRate: sampleRate,
            channels: AVAudioChannelCount(channels))
        else {
            cf_feed_destroy(feed)
            throw CrossfadeError(
                code: CrossfadeError.missingResultCode,
                message: "This device's audio format (\(Int(sampleRate)) Hz) is not supported.")
        }
        let target = RenderTarget.make(feed: feed, channels: channels)
        let node = AVAudioSourceNode(format: format, renderBlock: AudioOutput.renderBlock(target))
        let engine = AVAudioEngine()
        engine.attach(node)
        engine.connect(node, to: engine.mainMixerNode, format: format)
        engine.prepare()
        self.sampleRate = sampleRate
        self.engine = engine
        self.node = node
        self.target = target
    }

    var isRunning: Bool { engine.isRunning }

    func start() throws {
        if !engine.isRunning {
            try engine.start()
        }
    }

    func pause() {
        if engine.isRunning {
            engine.pause()
        }
    }

    /// Stops the engine for good, then frees the feed: the block can no longer run.
    func release() {
        if released { return }
        released = true
        engine.stop()
        engine.detach(node)
        target.free()
    }

    /// The render block. Real-time: it fills the plane table (allocated once, before the
    /// engine started) from the buffer list and calls the feed; no allocation, lock, Objective-C
    /// message or Swift call that can allocate. Anything unexpected is silence.
    private static func renderBlock(_ target: RenderTarget) -> AVAudioSourceNodeRenderBlock {
        return {
            (_: UnsafeMutablePointer<ObjCBool>, _: UnsafePointer<AudioTimeStamp>,
             frameCount: AVAudioFrameCount, bufferList: UnsafeMutablePointer<AudioBufferList>) -> OSStatus in
            let buffers = UnsafeMutableAudioBufferListPointer(bufferList)
            var ready = buffers.count == target.channels
            var channel = 0
            while ready && channel < target.channels {
                let data = buffers[channel].mData
                if data == nil { ready = false }
                target.planes[channel] = data?.assumingMemoryBound(to: Float.self)
                channel += 1
            }
            if !ready || cf_feed_fill_planar(target.feed, target.planes, target.channels, Int(frameCount)) != CF_OK {
                var index = 0
                while index < buffers.count {
                    if let data = buffers[index].mData {
                        memset(data, 0, Int(buffers[index].mDataByteSize))
                    }
                    index += 1
                }
            }
            return 0
        }
    }
}
