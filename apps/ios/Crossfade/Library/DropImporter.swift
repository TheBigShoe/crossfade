import Foundation
import UniformTypeIdentifiers

/// Files dropped on the library (iPad). A file the source lends in place is used where it is;
/// otherwise iOS hands over a temporary copy that it deletes, so that copy is kept in the
/// Documents folder's "Imported" folder, where the person can see and delete it.
enum DropImporter {
    static func load(_ providers: [NSItemProvider], into open: @escaping @MainActor @Sendable ([URL]) -> Void) -> Bool {
        var accepted = false
        for provider in providers {
            let types = provider.registeredTypeIdentifiers
            let audio = types.first { identifier in
                AudioTypes.importable.contains { UTType(identifier)?.conforms(to: $0) == true }
            }
            guard let type = audio else { continue }
            accepted = true
            _ = provider.loadInPlaceFileRepresentation(forTypeIdentifier: type) { url, inPlace, _ in
                guard let url = url else { return }
                let kept = inPlace ? url : keep(url)
                guard let found = kept else { return }
                Task { @MainActor in
                    open([found])
                }
            }
        }
        return accepted
    }

    /// Copies a file iOS is about to delete into Documents/Imported under a free name.
    private static func keep(_ url: URL) -> URL? {
        let fileManager = FileManager.default
        let documents = fileManager.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let folder = documents.appendingPathComponent("Imported", isDirectory: true)
        do {
            try fileManager.createDirectory(at: folder, withIntermediateDirectories: true)
            let stem = url.deletingPathExtension().lastPathComponent
            let ext = url.pathExtension
            var target = folder.appendingPathComponent(url.lastPathComponent)
            var n = 2
            while fileManager.fileExists(atPath: target.path(percentEncoded: false)) {
                let name = ext.isEmpty ? "\(stem) \(n)" : "\(stem) \(n).\(ext)"
                target = folder.appendingPathComponent(name)
                n += 1
            }
            try fileManager.copyItem(at: url, to: target)
            return target
        } catch {
            return nil
        }
    }
}
