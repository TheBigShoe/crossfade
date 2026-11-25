import Foundation

/// The FLAC compression level row: `null` is the default level.
public struct LevelRow: Codable, Equatable, Sendable {
    public var value: Int?
    public var visible: Bool
    public var enabled: Bool
    public var options: [Int?]

    public init(
        value: Int?,
        visible: Bool,
        enabled: Bool,
        options: [Int?]
    ) {
        self.value = value
        self.visible = visible
        self.enabled = enabled
        self.options = options
    }
}
