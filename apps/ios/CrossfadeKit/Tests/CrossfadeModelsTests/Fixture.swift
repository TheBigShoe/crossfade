import CrossfadeKit
import Foundation
import XCTest

/// The documents crossfade-ffi's snapshot tests pin, copied by apps/ios/scripts/pin_snapshots.py,
/// as the fakes' starting states.
enum Fixture {
    static func decode<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
        let url = Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Snapshots")
        let found = try XCTUnwrap(url, "missing Snapshots/\(name).json; run apps/ios/scripts/pin_snapshots.py")
        return try CrossfadeJSON.decode(type, from: Data(contentsOf: found))
    }

    static func sheet(_ name: String = "sheet_default") throws -> SheetState {
        try decode(SheetState.self, name)
    }

    static func queue() throws -> QueueState {
        try decode(QueueState.self, "queue_shape")
    }

    /// The pinned finished job, turned into another state.
    static func job(_ id: UInt64, _ state: String, output: String = "/out/song.flac") throws -> QueueJob {
        var job = try XCTUnwrap(queue().jobs.first)
        job.id = id
        job.state = state
        job.output = output
        job.frames = state == "finished" ? 4800 : nil
        job.fraction = nil
        return job
    }
}
