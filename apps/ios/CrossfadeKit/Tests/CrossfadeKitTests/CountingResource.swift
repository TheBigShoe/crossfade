import CrossfadeKit
import Foundation

/// A scoped resource that counts its starts and stops; `grants` is what a start returns.
final class CountingResource: ScopedResource {
    var grants: Bool
    private(set) var starts = 0
    private(set) var stops = 0

    init(grants: Bool = true) {
        self.grants = grants
    }

    var open: Int { starts - stops }

    func startAccessingSecurityScopedResource() -> Bool {
        starts += 1
        return grants
    }

    func stopAccessingSecurityScopedResource() {
        stops += 1
    }
}
