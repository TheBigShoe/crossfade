import Foundation

/// The device format the player was made for.
public struct PlayerOutput: Codable, Equatable, Sendable {
    public var sampleRate: Int
    public var channels: Int

    public init(
        sampleRate: Int,
        channels: Int
    ) {
        self.sampleRate = sampleRate
        self.channels = channels
    }
}
