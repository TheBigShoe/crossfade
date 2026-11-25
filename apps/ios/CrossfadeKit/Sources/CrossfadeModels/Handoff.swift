import Foundation

/// Carries a value into or out of a detached task. The engine and the sheets are safe to use
/// from any thread (the library serialises calls on each handle), which the protocols cannot
/// say; this says it at the one place a value crosses.
struct Handoff<Value>: @unchecked Sendable {
    let value: Value

    init(_ value: Value) {
        self.value = value
    }
}
