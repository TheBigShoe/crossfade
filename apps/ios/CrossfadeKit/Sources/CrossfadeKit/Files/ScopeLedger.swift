import Foundation

/// Scopes held per key (a job id, say): `hold` enters one, `release` leaves it. Holding a key
/// again first releases what it held, so starts and stops always balance.
public final class ScopeLedger<Key: Hashable> {
    private var held: [Key: [ScopedAccess]] = [:]

    public init() {}

    /// Enters the scope of each resource for `key`.
    public func hold(_ key: Key, _ resources: [any ScopedResource]) {
        release(key)
        held[key] = resources.map { ScopedAccess($0) }
    }

    public func release(_ key: Key) {
        held.removeValue(forKey: key)?.forEach { $0.end() }
    }

    /// Releases every key not in `keep`.
    public func release(allBut keep: Set<Key>) {
        for key in Array(held.keys) where !keep.contains(key) {
            release(key)
        }
    }

    public func releaseAll() {
        for key in Array(held.keys) {
            release(key)
        }
    }

    public var keys: Set<Key> { Set(held.keys) }
}
