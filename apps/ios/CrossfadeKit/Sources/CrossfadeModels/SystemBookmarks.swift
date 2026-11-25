import Foundation

/// Foundation's bookmarks. On iOS a bookmark made while a URL's scope is held carries that
/// scope; the URL it resolves to is entered with `startAccessingSecurityScopedResource`.
public struct SystemBookmarks: Bookmarking {
    public init() {}

    public func bookmark(for url: URL) throws -> Data {
        try url.bookmarkData(options: .minimalBookmark, includingResourceValuesForKeys: nil, relativeTo: nil)
    }

    public func resolve(_ bookmark: Data) throws -> (url: URL, isStale: Bool) {
        var stale = false
        let url = try URL(resolvingBookmarkData: bookmark, options: [], relativeTo: nil, bookmarkDataIsStale: &stale)
        return (url, stale)
    }
}
