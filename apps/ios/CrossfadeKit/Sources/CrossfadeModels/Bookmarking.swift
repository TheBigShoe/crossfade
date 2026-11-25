import Foundation

/// Makes and resolves bookmarks, so the library can reopen files between launches.
public protocol Bookmarking {
    /// A bookmark for a URL whose scope the caller holds.
    func bookmark(for url: URL) throws -> Data
    /// The URL a bookmark points at now, and whether it should be made again.
    func resolve(_ bookmark: Data) throws -> (url: URL, isStale: Bool)
}
