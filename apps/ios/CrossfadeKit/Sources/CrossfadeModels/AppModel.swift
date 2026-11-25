import CrossfadeKit
import Foundation
import Observation

/// What the whole app shares: the library, the selected file and its analysis, the player,
/// the queue and where output goes. Built once by the app with the live types.
@MainActor
@Observable
public final class AppModel {
    public let library: LibraryModel
    public let queue: QueueModel
    public let player: PlayerModel
    public let about: AboutModel
    public let folder: OutputFolder
    /// Set when the linked library is not the one this app was built for.
    public private(set) var startupProblem: String?

    public private(set) var selection: LibraryItem.ID?
    public private(set) var analysis: AnalysisModel?

    @ObservationIgnored public let engine: any CrossfadeEngine
    @ObservationIgnored private var started = false

    public init(
        engine: any CrossfadeEngine, queue: any QueueDriving, player: (any PlaybackDriving)?,
        store: any LibraryStore, bookmarks: any Bookmarking, activity: (any ActivityAssertion)?,
        folder: OutputFolder, startupProblem: String? = nil
    ) {
        self.engine = engine
        self.folder = folder
        self.startupProblem = startupProblem
        library = LibraryModel(engine: engine, store: store, bookmarks: bookmarks)
        self.queue = QueueModel(queue: queue, activity: activity)
        self.player = PlayerModel(player: player)
        about = AboutModel()
    }

    /// Once per launch: follows the queue, reads About and reopens the library.
    public func start() async {
        if started { return }
        started = true
        queue.start()
        about.load(engine: engine)
        await library.restore()
    }

    public var selectedItem: LibraryItem? { library.item(selection) }

    /// Shows a file: inspector, a new analysis and the player loaded with it.
    /// Selecting the shown file again starts what could not start while it was being read.
    public func select(_ id: LibraryItem.ID?) {
        let pending = analysis == nil && library.item(id)?.isReadable == true
        guard id != selection || pending else { return }
        selection = id
        analysis?.stop()
        analysis = nil
        guard let item = library.item(id), item.isReadable else { return }
        let model = AnalysisModel()
        model.start(engine: engine, url: item.url)
        analysis = model
        player.load(item.url)
    }

    /// Adds files and shows the first of them.
    public func open(_ urls: [URL]) async {
        let ids = await library.add(urls: urls)
        if let first = ids.first {
            selection = nil
            select(first)
        }
    }

    public func remove(ids: Set<LibraryItem.ID>) {
        if let selected = selection, ids.contains(selected) {
            select(nil)
            player.unload()
        }
        library.remove(ids: ids)
    }

    public func remove(atOffsets offsets: IndexSet) {
        let ids = Set(offsets.compactMap { $0 < library.items.count ? library.items[$0].id : nil })
        remove(ids: ids)
    }

    /// A convert screen for a file of the library.
    public func makeConvert(for item: LibraryItem) -> ConvertModel {
        ConvertModel(inputURL: item.url, folder: folder)
    }
}
