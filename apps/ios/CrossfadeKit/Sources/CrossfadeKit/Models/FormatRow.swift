import Foundation

/// The sample format row.
public struct FormatRow: Codable, Equatable, Sendable {
    public var value: String
    public var label: String
    public var visible: Bool
    public var enabled: Bool
    public var hint: String?
    public var options: [IdOption]

    public init(
        value: String,
        label: String,
        visible: Bool,
        enabled: Bool,
        hint: String?,
        options: [IdOption]
    ) {
        self.value = value
        self.label = label
        self.visible = visible
        self.enabled = enabled
        self.hint = hint
        self.options = options
    }
}
