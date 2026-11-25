import CrossfadeKit
import Foundation
import Observation

/// The convert screen: a sheet for one file, its state rendered as a `ConvertForm`, edits sent
/// back as the sheet's own changes.
///
/// Output goes to the app's Documents folder. The sheet names the output beside its input,
/// and stops renaming once a path is set, so a second sheet that is never given a path keeps
/// the library's own default name (`song.flac`, `song-s16.wav`, `song-copy.wav`) as the
/// choices change; that name is placed in the output folder. Typing a name stops following it.
@MainActor
@Observable
public final class ConvertModel {
    public let inputURL: URL
    public private(set) var state: SheetState?
    /// The last refusal or failure, in the library's words.
    public private(set) var message: String?
    public private(set) var isOpening = true
    public private(set) var followsDefaultName = true

    @ObservationIgnored private var sheet: (any SheetDriving)?
    @ObservationIgnored private var namingSheet: (any SheetDriving)?
    @ObservationIgnored private let folder: OutputFolder

    public init(inputURL: URL, folder: OutputFolder) {
        self.inputURL = inputURL
        self.folder = folder
    }

    public var form: ConvertForm? { state.map { ConvertForm(state: $0) } }

    public var title: String { inputURL.lastPathComponent }

    /// Opens the sheets off the main actor (they read the file's headers) inside its scope.
    public func open(engine: any CrossfadeEngine) async {
        if sheet != nil { return }
        let url = inputURL
        let path = url.path(percentEncoded: false)
        let maker = Handoff(engine)
        let made = await Task.detached(priority: .userInitiated) {
            Handoff(Result<[any SheetDriving], Error>(catching: {
                try ScopedAccess.during(url) {
                    [try maker.value.makeSheet(path: path), try maker.value.makeSheet(path: path)]
                }
            }))
        }.value
        isOpening = false
        switch made.value {
        case .success(let sheets):
            attach(sheet: sheets[0], namingSheet: sheets[1])
        case .failure(let error):
            message = error.localizedDescription
        }
    }

    /// Uses sheets made elsewhere; `open` calls it, and so do the tests.
    public func attach(sheet: any SheetDriving, namingSheet: (any SheetDriving)?) {
        self.sheet = sheet
        self.namingSheet = namingSheet
        isOpening = false
        perform {
            try placeOutput()
        }
    }

    /// Sends one change, as choosing an option or typing in a field does.
    public func send(_ change: SheetChange) {
        perform {
            try sheet?.apply([change])
            if Self.affectsName(change) {
                try namingSheet?.apply([change])
                try placeOutput()
            }
        }
    }

    /// The person typed a file name: write that, in the output folder, from now on.
    public func rename(to name: String) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        followsDefaultName = false
        perform {
            try sheet?.apply([.output(trimmed.isEmpty ? "" : folder.path(forName: trimmed))])
        }
    }

    /// Queues the job if the sheet says it can be; returns its id.
    @discardableResult
    public func addToQueue(_ queue: QueueModel) -> UInt64? {
        guard let sheet = sheet, state?.canQueue == true else { return nil }
        do {
            let id = try queue.add(sheet, inputURL: inputURL)
            message = nil
            return id
        } catch {
            message = error.localizedDescription
            refresh()
            return nil
        }
    }

    // The path, the overwrite switch and Adjust say nothing about the default name, and
    // Adjust's fields change with every keystroke.
    private static func affectsName(_ change: SheetChange) -> Bool {
        switch change {
        case .output, .overwrite, .adjustOpen, .levelMode, .levelValue, .trimStart, .trimEnd,
             .fadeIn, .fadeOut:
            return false
        case .preset, .container, .copy, .format, .compressionLevel, .bitrate, .sampleRate,
             .channels:
            return true
        }
    }

    private func placeOutput() throws {
        guard followsDefaultName, let sheet = sheet, let naming = namingSheet else { return }
        let suggested: String = try naming.state().output
        let wanted = folder.path(forName: (suggested as NSString).lastPathComponent)
        let current: String = try sheet.state().output
        if current != wanted {
            try sheet.apply([.output(wanted)])
        }
    }

    private func perform(_ body: () throws -> Void) {
        do {
            try body()
            message = nil
        } catch {
            message = error.localizedDescription
        }
        refresh()
    }

    private func refresh() {
        guard let sheet = sheet else { return }
        do {
            state = try sheet.state()
        } catch {
            message = error.localizedDescription
        }
    }
}
