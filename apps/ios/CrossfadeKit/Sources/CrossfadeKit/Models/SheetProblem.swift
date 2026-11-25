import Foundation

/// Something that stops the sheet queueing. Kinds: `no_output output_is_input output_exists extension source adjust`; `row` (`level trim fade`) for `adjust`.
public struct SheetProblem: Codable, Equatable, Sendable {
    public var kind: String
    public var row: String?
    public var message: String

    public init(
        kind: String,
        row: String?,
        message: String
    ) {
        self.kind = kind
        self.row = row
        self.message = message
    }
}
