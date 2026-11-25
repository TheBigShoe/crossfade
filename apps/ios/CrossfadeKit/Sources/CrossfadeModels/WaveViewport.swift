import Foundation

/// Which stretch of a file the waveform shows: the phone's own zoom and pan (the desktop's
/// gesture model is not exposed). Frames are the source's; fractions are of the view's width.
public struct WaveViewport: Equatable, Sendable {
    public private(set) var totalFrames: Double
    /// The first frame shown.
    public private(set) var start: Double
    /// How many frames the width shows.
    public private(set) var span: Double

    /// The closest zoom: this many frames across the whole width.
    public static let minimumSpan: Double = 256

    public init(totalFrames: UInt64) {
        let total = Double(max(totalFrames, 1))
        self.totalFrames = total
        start = 0
        span = total
    }

    public var isZoomed: Bool { span < totalFrames }

    /// Frames per column when the width holds `columns` columns, at least one.
    public func framesPerColumn(columns: Int) -> UInt64 {
        guard columns > 0 else { return 1 }
        return UInt64(max(1, (span / Double(columns)).rounded(.up)))
    }

    public var startFrame: UInt64 { UInt64(start.rounded(.down)) }

    /// The frame under a point `fraction` of the way across.
    public func frame(atFraction fraction: Double) -> UInt64 {
        let clamped = min(max(fraction, 0), 1)
        return UInt64(min(start + clamped * span, totalFrames).rounded(.down))
    }

    /// Where a frame falls across the width, or nil when it is out of view.
    public func fraction(ofFrame frame: UInt64) -> Double? {
        let at = (Double(frame) - start) / span
        return at >= 0 && at <= 1 ? at : nil
    }

    /// Zooms by `factor` (above 1 closer) keeping the frame under `anchor` where it is.
    public mutating func zoom(by factor: Double, anchor: Double) {
        guard factor > 0, factor.isFinite else { return }
        let anchorFraction = min(max(anchor, 0), 1)
        let anchorFrame = start + anchorFraction * span
        let lower = min(Self.minimumSpan, totalFrames)
        span = min(max(span / factor, lower), totalFrames)
        start = anchorFrame - anchorFraction * span
        clamp()
    }

    /// Moves the view by a fraction of its width; a drag to the right shows earlier audio.
    public mutating func pan(byFraction fraction: Double) {
        guard fraction.isFinite else { return }
        start -= fraction * span
        clamp()
    }

    /// Brings `frame` into view, turning a page if it has left the visible stretch.
    public mutating func follow(frame: UInt64) {
        let at = Double(frame)
        if at < start || at > start + span {
            start = at
            clamp()
        }
    }

    public mutating func fit() {
        start = 0
        span = totalFrames
    }

    private mutating func clamp() {
        start = min(max(start, 0), max(totalFrames - span, 0))
    }
}
