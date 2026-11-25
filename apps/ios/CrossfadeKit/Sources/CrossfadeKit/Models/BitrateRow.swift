import Foundation

/// The Opus bitrate row, in bits per second; `null` is the default.
public struct BitrateRow: Codable, Equatable, Sendable {
    public var value: Int?
    public var label: String
    public var visible: Bool
    public var enabled: Bool
    public var options: [LabelledOption<Int>]

    public init(
        value: Int?,
        label: String,
        visible: Bool,
        enabled: Bool,
        options: [LabelledOption<Int>]
    ) {
        self.value = value
        self.label = label
        self.visible = visible
        self.enabled = enabled
        self.options = options
    }
}
