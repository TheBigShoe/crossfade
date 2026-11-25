import Foundation

/// A queued job's state, from the JSON's `state` string.
public enum JobPhase: String, Equatable, Sendable {
    case pending, running, finished, failed, cancelled
}
