import Foundation

/// The loudness line under the waveform, whole and in pieces.
public struct Readout: Codable, Equatable, Sendable {
    public var text: String
    public var warn: Bool
    public var segments: [ReadoutSegment]

    public init(
        text: String,
        warn: Bool,
        segments: [ReadoutSegment]
    ) {
        self.text = text
        self.warn = warn
        self.segments = segments
    }
}
