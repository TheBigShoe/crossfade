import Foundation

/// The preset row. `value` is null while the choices match no preset ("Custom").
public struct PresetRow: Codable, Equatable, Sendable {
    public var value: String?
    public var options: [IdOption]

    public init(
        value: String?,
        options: [IdOption]
    ) {
        self.value = value
        self.options = options
    }
}
