import CrossfadeKit
@testable import CrossfadeModels
import XCTest

/// Queue state to row states, the commands, the inputs' scopes and the awake assertion.
final class QueueModelTests: XCTestCase {
    private func state(_ jobs: [QueueJob]) throws -> QueueState {
        var state = try Fixture.queue()
        state.jobs = jobs
        func count(_ s: String) -> Int { jobs.filter { $0.state == s }.count }
        state.summary = QueueSummary(
            pending: count("pending"), running: count("running"), finished: count("finished"),
            failed: count("failed"), cancelled: count("cancelled"))
        state.idle = count("pending") + count("running") == 0
        return state
    }

    func testRowStates() throws {
        var running = try Fixture.job(1, "running")
        running.framesDone = 52000
        running.framesTotal = 96000
        running.fraction = 0.54
        var measuring = try Fixture.job(2, "running")
        measuring.measuring = true
        var failed = try Fixture.job(3, "failed")
        failed.message = "/out/song.flac already exists"
        let finished = try Fixture.job(4, "finished", output: "/Documents/song.flac")
        let pending = try Fixture.job(5, "pending")

        let rows = [running, measuring, failed, finished, pending].map { QueueRow(job: $0) }
        XCTAssertEqual(rows[0].status, "Converting · 54%")
        XCTAssertEqual(rows[0].progress, 0.54)
        XCTAssertTrue(rows[0].canCancel)
        XCTAssertFalse(rows[0].canRemove)
        XCTAssertFalse(rows[0].canRetry)
        XCTAssertEqual(rows[1].status, "Measuring loudness…")
        XCTAssertNil(rows[1].progress)
        XCTAssertTrue(rows[1].showsProgress)
        XCTAssertEqual(rows[2].status, "Failed")
        XCTAssertEqual(rows[2].failure, "/out/song.flac already exists")
        XCTAssertTrue(rows[2].canRetry)
        XCTAssertTrue(rows[2].canRemove)
        XCTAssertFalse(rows[2].canCancel)
        XCTAssertTrue(rows[3].status.hasPrefix("Done"))
        XCTAssertEqual(rows[3].outputURL, URL(fileURLWithPath: "/Documents/song.flac"))
        XCTAssertEqual(rows[3].title, "song.flac")
        XCTAssertEqual(rows[4].status, "Waiting")
        XCTAssertTrue(rows[4].canCancel)
        XCTAssertNil(rows[4].outputURL)
    }

    func testACancellingJobCannotBeCancelledAgain() throws {
        var job = try Fixture.job(1, "running")
        job.cancelling = true
        let row = QueueRow(job: job)
        XCTAssertFalse(row.canCancel)
        XCTAssertEqual(row.status, "Cancelling…")
    }

    @MainActor
    func testTheInputsScopeIsHeldUntilTheJobStopsReading() throws {
        let queue = FakeQueue(try state([]), template: try Fixture.job(0, "pending"))
        let activity = FakeActivity()
        let model = QueueModel(queue: queue, activity: activity)
        let sheet = FakeSheet(try Fixture.sheet())
        let id2 = try model.add(sheet, inputURL: URL(fileURLWithPath: "/elsewhere/in.wav"))
        // Pending: the job will open its input when it starts.
        XCTAssertTrue(model.holdsScope(for: id2))

        queue.current = try state([Fixture.job(id2, "running")])
        model.apply(queue.current)
        XCTAssertTrue(model.holdsScope(for: id2))
        XCTAssertTrue(model.isBusy)
        XCTAssertEqual(activity.active, true)

        queue.current = try state([Fixture.job(id2, "finished")])
        model.apply(queue.current)
        XCTAssertFalse(model.holdsScope(for: id2))
        XCTAssertFalse(model.isBusy)
        XCTAssertEqual(activity.active, false)
        XCTAssertEqual(model.finishedCount, 1)
    }

    @MainActor
    func testRetryHoldsTheScopeUnderTheNewId() throws {
        let queue = FakeQueue(try state([]), template: try Fixture.job(0, "pending"))
        let model = QueueModel(queue: queue, activity: nil)
        let id = try model.add(FakeSheet(try Fixture.sheet()), inputURL: URL(fileURLWithPath: "/a.wav"))
        queue.current = try state([Fixture.job(id, "failed")])
        model.apply(queue.current)
        XCTAssertFalse(model.holdsScope(for: id))
        XCTAssertEqual(model.failedCount, 1)

        model.retry(id)
        let newId = queue.nextId
        XCTAssertTrue(model.holdsScope(for: newId))
        XCTAssertFalse(model.holdsScope(for: id))
        XCTAssertEqual(model.rows.map { $0.id }, [newId])
    }

    @MainActor
    func testCommandsAndRefusals() throws {
        let queue = FakeQueue(try state([Fixture.job(1, "running"), Fixture.job(2, "finished")]))
        let model = QueueModel(queue: queue, activity: nil)
        model.cancel(1)
        model.remove(2)
        model.clearFinished()
        XCTAssertEqual(queue.cancelled, [1])
        XCTAssertEqual(queue.removed, [2])
        XCTAssertEqual(queue.cleared, 1)
        XCTAssertEqual(model.rows.count, 2)

        queue.refusal = CrossfadeError(code: 4, message: "A running conversion cannot be removed.")
        model.remove(1)
        XCTAssertEqual(model.message, "A running conversion cannot be removed.")
    }
}
