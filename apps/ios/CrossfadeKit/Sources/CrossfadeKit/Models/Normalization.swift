import Foundation

/// A normalising job's measurement, after its measuring pass.
public struct Normalization: Codable, Equatable, Sendable {
    public var measuredLufs: Double?
    public var targetLufs: Double
    public var reachedLufs: Double?
    public var gainDb: Double
    public var limitedByCeiling: Bool
    public var ceilingDbtp: Double

    public init(
        measuredLufs: Double?,
        targetLufs: Double,
        reachedLufs: Double?,
        gainDb: Double,
        limitedByCeiling: Bool,
        ceilingDbtp: Double
    ) {
        self.measuredLufs = measuredLufs
        self.targetLufs = targetLufs
        self.reachedLufs = reachedLufs
        self.gainDb = gainDb
        self.limitedByCeiling = limitedByCeiling
        self.ceilingDbtp = ceilingDbtp
    }
}
