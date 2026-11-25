import Foundation

/// The container row: ids `wav flac aiff oga m4a opus`.
public struct ContainerRow: Codable, Equatable, Sendable {
    public var value: String
    public var label: String
    public var options: [ContainerOption]

    public init(
        value: String,
        label: String,
        options: [ContainerOption]
    ) {
        self.value = value
        self.label = label
        self.options = options
    }
}
