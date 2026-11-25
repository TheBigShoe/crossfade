import CrossfadeKit
import Foundation

/// A sheet that records what it was sent and behaves just enough like the library's: until a
/// path is given, a container renames the output by the default rule (the source is a WAV, so
/// another container drops "-copy"); after one, only the extension follows the container.
final class FakeSheet: SheetDriving {
    var current: SheetState
    private(set) var applied: [SheetChange] = []
    private(set) var pathGiven = false
    var refusal: CrossfadeError?

    init(_ state: SheetState) {
        current = state
    }

    func state() throws -> SheetState { current }

    func apply(_ changes: [SheetChange]) throws {
        if let refusal = refusal { throw refusal }
        for change in changes {
            applied.append(change)
            react(change)
        }
    }

    private func react(_ change: SheetChange) {
        switch change {
        case .container(let id):
            current.container.value = id
            var stem = (current.output as NSString).deletingPathExtension
            if !pathGiven, id != "wav", stem.hasSuffix("-copy") {
                stem = String(stem.dropLast(5))
            }
            current.output = stem + "." + id
        case .output(let path):
            pathGiven = true
            current.output = path
        case .overwrite(let on):
            current.overwrite = on
        default:
            break
        }
    }
}
