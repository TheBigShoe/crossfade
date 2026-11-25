import Foundation

/// The sample rate row, in Hz; `null` is the source's rate.
public struct SampleRateRow: Codable, Equatable, Sendable {
    public var value: Int?
    public var label: String
    public var enabled: Bool
    public var options: [LabelledOption<Int>]

    public init(
        value: Int?,
        label: String,
        enabled: Bool,
        options: [LabelledOption<Int>]
    ) {
        self.value = value
        self.label = label
        self.enabled = enabled
        self.options = options
    }
}
