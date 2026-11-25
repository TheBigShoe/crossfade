import CrossfadeKit
import Foundation
import Observation

/// The files list: what the person brought in, each inspected off the main actor inside its
/// security scope, remembered between launches as bookmarks.
@MainActor
@Observable
public final class LibraryModel {
    public private(set) var items: [LibraryItem] = []

    @ObservationIgnored private let engine: any CrossfadeEngine
    @ObservationIgnored private let store: any LibraryStore
    @ObservationIgnored private let bookmarks: any Bookmarking

    public init(engine: any CrossfadeEngine, store: any LibraryStore, bookmarks: any Bookmarking) {
        self.engine = engine
        self.store = store
        self.bookmarks = bookmarks
    }

    public func item(_ id: LibraryItem.ID?) -> LibraryItem? {
        guard let id = id else { return nil }
        return items.first { $0.id == id }
    }

    /// Reopens the files of the last launch. One that cannot be found any more is listed with
    /// why, so the person sees it went missing rather than it silently vanishing.
    public func restore() async {
        var restored: [LibraryItem] = []
        for data in store.load() {
            do {
                let found = try bookmarks.resolve(data)
                var bookmark = data
                if found.isStale, let fresh = try? ScopedAccess.during(found.url, { try bookmarks.bookmark(for: found.url) }) {
                    bookmark = fresh
                }
                restored.append(LibraryItem(url: found.url, bookmark: bookmark))
            } catch {
                // A bookmark that no longer resolves cannot even name its file; drop it.
            }
        }
        items = restored
        persist()
        await inspectPending()
    }

    /// Adds files from the importer, an "Open in" or a drop. A file already listed is not
    /// added twice. Returns the ids of the items, new or existing, in the order given.
    @discardableResult
    public func add(urls: [URL]) async -> [LibraryItem.ID] {
        var ids: [LibraryItem.ID] = []
        for url in urls {
            let path = url.standardizedFileURL.path(percentEncoded: false)
            if let existing = items.first(where: { $0.url.standardizedFileURL.path(percentEncoded: false) == path }) {
                ids.append(existing.id)
                continue
            }
            let bookmark = try? ScopedAccess.during(url) { try bookmarks.bookmark(for: url) }
            let item = LibraryItem(url: url, bookmark: bookmark)
            items.append(item)
            ids.append(item.id)
        }
        persist()
        await inspectPending()
        return ids
    }

    public func remove(ids: Set<LibraryItem.ID>) {
        items.removeAll { ids.contains($0.id) }
        persist()
    }

    public func remove(atOffsets offsets: IndexSet) {
        let ids = Set(offsets.compactMap { $0 < items.count ? items[$0].id : nil })
        remove(ids: ids)
    }

    /// Inspects every item not yet inspected, one at a time, off the main actor.
    private func inspectPending() async {
        for item in items where item.isInspecting {
            let url = item.url
            let inspector = Handoff(engine)
            let result: Result<InspectedFile, Error> = await Task.detached(priority: .userInitiated) {
                Result<InspectedFile, Error>(catching: {
                    try ScopedAccess.during(url) {
                        try inspector.value.inspect(path: url.path(percentEncoded: false))
                    }
                })
            }.value
            guard let index = items.firstIndex(where: { $0.id == item.id }) else { continue }
            switch result {
            case .success(let file): items[index].file = file
            case .failure(let error): items[index].failure = error.localizedDescription
            }
        }
    }

    private func persist() {
        store.save(items.compactMap { $0.bookmark })
    }
}
