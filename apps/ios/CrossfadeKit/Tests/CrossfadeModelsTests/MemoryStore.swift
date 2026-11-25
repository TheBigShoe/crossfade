import CrossfadeModels
import Foundation

/// A library store in memory.
final class MemoryStore: LibraryStore {
    var saved: [Data] = []

    func load() -> [Data] { saved }

    func save(_ bookmarks: [Data]) {
        saved = bookmarks
    }
}
