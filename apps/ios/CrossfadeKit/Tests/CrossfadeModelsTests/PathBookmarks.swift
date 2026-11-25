import CrossfadeModels
import Foundation

/// Bookmarks that are a path's bytes; a path listed in `gone` no longer resolves.
struct PathBookmarks: Bookmarking {
    var gone: Set<String> = []

    func bookmark(for url: URL) throws -> Data {
        Data(url.path(percentEncoded: false).utf8)
    }

    func resolve(_ bookmark: Data) throws -> (url: URL, isStale: Bool) {
        let path = String(decoding: bookmark, as: UTF8.self)
        if gone.contains(path) { throw CocoaError(.fileNoSuchFile) }
        return (URL(fileURLWithPath: path), false)
    }
}
