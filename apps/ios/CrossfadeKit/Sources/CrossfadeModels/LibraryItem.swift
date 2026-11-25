import CrossfadeKit
import Foundation

/// A file the person brought in, with what the inspector said about it.
public struct LibraryItem: Identifiable, Equatable {
    public let id: UUID
    public var url: URL
    /// Remembers the file between launches; nil if the system would not make one.
    public var bookmark: Data?
    /// The inspector's document; nil while it is being read.
    public var file: InspectedFile?
    /// Why the file could not be inspected at all (it moved, or access was refused).
    public var failure: String?

    public init(id: UUID = UUID(), url: URL, bookmark: Data?, file: InspectedFile? = nil, failure: String? = nil) {
        self.id = id
        self.url = url
        self.bookmark = bookmark
        self.file = file
        self.failure = failure
    }

    public var path: String { url.path(percentEncoded: false) }

    public var title: String { file?.name ?? url.lastPathComponent }

    public var isInspecting: Bool { file == nil && failure == nil }

    public var isReadable: Bool { file?.readable == true }

    /// The files list's line: the inspector's summary, or why the file cannot be read.
    public var subtitle: String {
        if let failure = failure { return failure }
        guard let file = file else { return "Reading…" }
        if file.readable { return file.summary ?? "" }
        return file.problem ?? "Unreadable"
    }
}
