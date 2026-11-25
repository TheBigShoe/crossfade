import CrossfadeKit
import CrossfadeModels
import Foundation

/// Counts what the queue asked of the app's activity assertion.
@MainActor
final class FakeActivity: ActivityAssertion {
    private(set) var calls: [Bool] = []
    var active: Bool { calls.last ?? false }

    func setActive(_ active: Bool) {
        calls.append(active)
    }
}
