import Foundation

/// Loudness in LUFS and LU; the peaks are linear. `null` until measurable.
public struct Loudness: Codable, Equatable, Sendable {
    public var integrated: Double?
    public var range: Double?
    public var truePeak: Double?
    public var samplePeak: Double

    public init(
        integrated: Double?,
        range: Double?,
        truePeak: Double?,
        samplePeak: Double
    ) {
        self.integrated = integrated
        self.range = range
        self.truePeak = truePeak
        self.samplePeak = samplePeak
    }
}
