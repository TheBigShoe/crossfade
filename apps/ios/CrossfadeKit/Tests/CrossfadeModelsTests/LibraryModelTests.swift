import CrossfadeKit
import CrossfadeModels
import XCTest

/// The files list: inspected rows, unreadable files with their problem, no duplicates, and the
/// list remembered between launches.
final class LibraryModelTests: XCTestCase {
    @MainActor
    func testAddedFilesAreInspectedAndRemembered() async {
        let engine = FakeEngine()
        engine.files["/music/song.flac"] = FakeEngine.readable("/music/song.flac")
        let store = MemoryStore()
        let library = LibraryModel(engine: engine, store: store, bookmarks: PathBookmarks())
        let urls = [URL(fileURLWithPath: "/music/song.flac"), URL(fileURLWithPath: "/music/notes.txt")]
        let ids = await library.add(urls: urls)

        XCTAssertEqual(ids.count, 2)
        XCTAssertEqual(library.items.map { $0.title }, ["song.flac", "notes.txt"])
        XCTAssertEqual(library.items[0].subtitle, "FLAC · 44.1 kHz · stereo · 16-bit · 0:01.000")
        XCTAssertTrue(library.items[0].isReadable)
        XCTAssertEqual(library.items[1].subtitle, "unrecognised format")
        XCTAssertFalse(library.items[1].isReadable)
        XCTAssertEqual(store.saved.count, 2)

        let again = await library.add(urls: [URL(fileURLWithPath: "/music/song.flac")])
        XCTAssertEqual(again, [ids[0]])
        XCTAssertEqual(library.items.count, 2)
    }

    @MainActor
    func testRemoveAndRestore() async {
        let engine = FakeEngine()
        let store = MemoryStore()
        let library = LibraryModel(engine: engine, store: store, bookmarks: PathBookmarks())
        await library.add(urls: [URL(fileURLWithPath: "/a.wav"), URL(fileURLWithPath: "/b.wav"), URL(fileURLWithPath: "/c.wav")])
        library.remove(atOffsets: IndexSet(integer: 1))
        XCTAssertEqual(library.items.map { $0.title }, ["a.wav", "c.wav"])

        let reopened = LibraryModel(engine: engine, store: store, bookmarks: PathBookmarks(gone: ["/c.wav"]))
        await reopened.restore()
        XCTAssertEqual(reopened.items.map { $0.title }, ["a.wav"])
        XCTAssertEqual(store.saved.count, 1)
    }
}
