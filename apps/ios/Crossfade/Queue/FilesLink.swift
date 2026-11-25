import Foundation

/// A link that opens the Files app at a folder of this app's container (the `shareddocuments`
/// scheme Files answers to).
enum FilesLink {
    static func folder(of file: URL) -> URL? {
        let folder = file.deletingLastPathComponent().path(percentEncoded: true)
        return URL(string: "shareddocuments://" + folder)
    }
}
