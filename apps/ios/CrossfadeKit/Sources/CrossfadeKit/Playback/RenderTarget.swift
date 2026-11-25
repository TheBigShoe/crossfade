import CrossfadeFFI
import Foundation

/// What the render block captures, all fixed before the engine starts: the feed, a plane
/// table allocated once, and the channel count. Plain values, so the block retains nothing.
struct RenderTarget: @unchecked Sendable {
    let feed: OpaquePointer
    let planes: UnsafeMutablePointer<UnsafeMutablePointer<Float>?>
    let channels: Int

    static func make(feed: OpaquePointer, channels: Int) -> RenderTarget {
        let planes = UnsafeMutablePointer<UnsafeMutablePointer<Float>?>.allocate(capacity: channels)
        planes.initialize(repeating: nil, count: channels)
        return RenderTarget(feed: feed, planes: planes, channels: channels)
    }

    /// Frees the feed and the plane table. Only once the engine can no longer call the block.
    func free() {
        cf_feed_destroy(feed)
        planes.deinitialize(count: channels)
        planes.deallocate()
    }
}
