import Foundation

/// Keeps the library's bookmarks between launches, in order.
public protocol LibraryStore {
    func load() -> [Data]
    func save(_ bookmarks: [Data])
}
