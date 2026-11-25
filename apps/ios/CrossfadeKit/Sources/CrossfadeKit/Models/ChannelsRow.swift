import Foundation

/// The channels row: `null` is the source's layout, else a layout name.
public struct ChannelsRow: Codable, Equatable, Sendable {
    public var value: String?
    public var label: String
    public var enabled: Bool
    public var options: [LabelledOption<String>]

    public init(
        value: String?,
        label: String,
        enabled: Bool,
        options: [LabelledOption<String>]
    ) {
        self.value = value
        self.label = label
        self.enabled = enabled
        self.options = options
    }
}
