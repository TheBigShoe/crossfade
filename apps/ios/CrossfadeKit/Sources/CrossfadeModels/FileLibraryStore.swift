import Foundation

/// The bookmarks as a property list in Application Support (not Documents, which the person
/// sees in Files; not UserDefaults, which would need another privacy-manifest reason).
public struct FileLibraryStore: LibraryStore {
    public let url: URL

    public init(url: URL) {
        self.url = url
    }

    public static func standard(fileManager: FileManager = .default) -> FileLibraryStore {
        let folder = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return FileLibraryStore(url: folder.appendingPathComponent("Library.plist"))
    }

    public func load() -> [Data] {
        guard let data = try? Data(contentsOf: url) else { return [] }
        return (try? PropertyListDecoder().decode([Data].self, from: data)) ?? []
    }

    public func save(_ bookmarks: [Data]) {
        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            let data = try PropertyListEncoder().encode(bookmarks)
            try data.write(to: url, options: [.atomic])
        } catch {
            // Losing the list costs the person re-adding files, not their work; carry on.
        }
    }
}
