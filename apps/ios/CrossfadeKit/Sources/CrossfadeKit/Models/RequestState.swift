import Foundation

/// The engine's own request: the job file's shape, shown, never sent back.
public struct RequestState: Codable, Equatable, Sendable {
    public var job: JSONValue
    public var overwrite: Bool

    public init(
        job: JSONValue,
        overwrite: Bool
    ) {
        self.job = job
        self.overwrite = overwrite
    }
}
