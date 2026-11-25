import CrossfadeKit
import CrossfadeModels
import Foundation

/// Builds the app's model from the live types, once, at launch.
@MainActor
enum AppBootstrap {
    static func makeAppModel() -> AppModel {
        let folder = OutputFolder.documents()
        // Nothing runs yet, so any conversion temp file here was left by a run the system ended.
        folder.sweepStaleTemporaryFiles()

        var problem: String?
        do {
            try CrossfadeLibrary.checkABI()
        } catch {
            problem = error.localizedDescription
        }

        let queue: any QueueDriving
        do {
            queue = try ConversionQueue()
        } catch {
            queue = UnavailableQueue(reason: error.localizedDescription)
            problem = problem ?? error.localizedDescription
        }

        // Preview needs an audio session; without one the transport hides and the rest works.
        let player = PreviewPlayer()
        var playback: (any PlaybackDriving)? = player
        do {
            try player.start()
        } catch {
            playback = nil
        }

        return AppModel(
            engine: LiveEngine(),
            queue: queue,
            player: playback,
            store: FileLibraryStore.standard(),
            bookmarks: SystemBookmarks(),
            activity: KeepAwake(),
            folder: folder,
            startupProblem: problem)
    }
}
