import Foundation

/// A `CfPlayer` pointer the waiting task may carry off the main actor: `cf_player_wait` is
/// the one player function any thread may call. Every other call stays on the main actor.
struct PlayerHandle: @unchecked Sendable {
    let raw: OpaquePointer
}
