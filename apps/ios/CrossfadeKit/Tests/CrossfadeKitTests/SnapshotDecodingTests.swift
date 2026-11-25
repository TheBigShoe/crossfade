import CrossfadeKit
import XCTest

/// Every model decodes the Rust side's own pinned JSON, with the keys and nulls it really has.
final class SnapshotDecodingTests: XCTestCase {
    func testAbout() throws {
        let about = try CrossfadeJSON.decode(About.self, from: Fixture.data("about_shape"))
        XCTAssertEqual(about.abiVersion, 1)
        XCTAssertEqual(about.lines.count, 3)
    }

    func testReadableFile() throws {
        let file = try CrossfadeJSON.decode(InspectedFile.self, from: Fixture.data("inspect_readable"))
        XCTAssertTrue(file.readable)
        XCTAssertNil(file.problem)
        XCTAssertEqual(file.summary, "FLAC · 44.1 kHz · stereo · 16-bit · 0:00.113")
        XCTAssertEqual(file.sections.map { $0.title }, ["File", "Stream 0", "Tags"])
        XCTAssertEqual(file.sections[2].rows[0].id, "tags-0")
        XCTAssertEqual(file.sections[1].rows.first { $0.field == "sample_format" }?.value, "16-bit")
    }

    func testUnreadableFile() throws {
        let file = try CrossfadeJSON.decode(InspectedFile.self, from: Fixture.data("inspect_unreadable"))
        XCTAssertFalse(file.readable)
        XCTAssertEqual(file.problem, "unrecognised format")
        XCTAssertNil(file.summary)
        XCTAssertTrue(file.sections.isEmpty)
    }

    func testDefaultSheet() throws {
        let sheet = try CrossfadeJSON.decode(SheetState.self, from: Fixture.data("sheet_default"))
        XCTAssertEqual(sheet.container.value, "wav")
        XCTAssertEqual(sheet.container.options.map { $0.fileExtension }, ["wav", "flac", "aiff", "oga", "m4a", "opus"])
        XCTAssertEqual(sheet.preset.value, "same")
        XCTAssertNil(sheet.compressionLevel.value)
        XCTAssertEqual(sheet.compressionLevel.options.first, .some(nil))
        XCTAssertEqual(sheet.compressionLevel.options.last, .some(8))
        XCTAssertFalse(sheet.bitrate.visible)
        XCTAssertNil(sheet.bitrate.options[0].value)
        XCTAssertEqual(sheet.bitrate.options[1].value, 32000)
        XCTAssertEqual(sheet.channels.options[1].value, "mono")
        XCTAssertNil(sheet.sampleRate.value)
        XCTAssertTrue(sheet.copy.available)
        XCTAssertEqual(sheet.adjust.levelMode, "off")
        XCTAssertNil(sheet.adjust.problems.level)
        XCTAssertTrue(sheet.canQueue)
        XCTAssertNotNil(sheet.commandLine)
        XCTAssertTrue(sheet.problems.isEmpty)
        XCTAssertEqual(sheet.request.overwrite, false)
        XCTAssertNotNil(sheet.request.job["outputs"])
    }

    func testOpusSheet() throws {
        let sheet = try CrossfadeJSON.decode(SheetState.self, from: Fixture.data("sheet_opus"))
        XCTAssertEqual(sheet.container.value, "opus")
        XCTAssertEqual(sheet.bitrate.value, 96000)
        XCTAssertTrue(sheet.bitrate.visible)
        XCTAssertFalse(sheet.format.visible)
        XCTAssertEqual(sheet.sampleRate.value, 48000)
        XCTAssertFalse(sheet.copy.available)
        XCTAssertTrue(sheet.adjust.open)
        XCTAssertEqual(sheet.adjust.summary, "Normalize −16 LUFS · Fade out 0.5 s")
        XCTAssertEqual(sheet.notes, ["Opus is always 48 kHz: resampling from 44100 Hz"])
        XCTAssertEqual(sheet.processingNote, "48 kHz · stereo · 0:01.000")
    }

    func testSheetWithAProblem() throws {
        let sheet = try CrossfadeJSON.decode(SheetState.self, from: Fixture.data("sheet_problems"))
        XCTAssertFalse(sheet.canQueue)
        XCTAssertNil(sheet.commandLine)
        XCTAssertEqual(sheet.problems.count, 1)
        XCTAssertEqual(sheet.problems[0].kind, "extension")
        XCTAssertNil(sheet.problems[0].row)
        XCTAssertTrue(sheet.problems[0].message.hasPrefix("Crossfade can save"))
    }

    func testQueue() throws {
        let queue = try CrossfadeJSON.decode(QueueState.self, from: Fixture.data("queue_shape"))
        XCTAssertEqual(queue.revision, 7)
        XCTAssertTrue(queue.idle)
        XCTAssertNil(queue.quitWarning)
        XCTAssertEqual(queue.summary.finished, 1)
        let job = try XCTUnwrap(queue.jobs.first)
        XCTAssertEqual(job.phase, .finished)
        XCTAssertEqual(job.frames, 4800)
        XCTAssertEqual(job.fraction, 1.0)
        XCTAssertNil(job.framesDone)
        XCTAssertNil(job.normalization)
        XCTAssertEqual(job.clipped, 0)
    }

    func testAnalysis() throws {
        let analysis = try CrossfadeJSON.decode(AnalysisState.self, from: Fixture.data("analysis_shape"))
        XCTAssertEqual(analysis.state, "done")
        XCTAssertEqual(analysis.lanes, 2)
        XCTAssertEqual(analysis.frames, 192000)
        XCTAssertEqual(analysis.sampleRate, 48000)
        XCTAssertNotNil(analysis.loudness.integrated)
        XCTAssertEqual(analysis.readout.segments.count, 5)
        XCTAssertFalse(analysis.readout.warn)
    }

    func testPlayer() throws {
        let player = try CrossfadeJSON.decode(PlayerState.self, from: Fixture.data("player_shape"))
        XCTAssertEqual(player.state, "ready")
        XCTAssertEqual(player.duration, 88200)
        XCTAssertEqual(player.transport, "0:00.0 / 0:02.0")
        XCTAssertEqual(player.output.sampleRate, 48000)
        XCTAssertFalse(player.deviceRunning)
    }

    /// States the snapshots do not pin, from a live run of the library: nulls where the Rust
    /// types are optional.
    func testNullsTheSnapshotsDoNotShow() throws {
        let failed = """
        {"revision": 1, "state": "failed", "failure": "i/o error", "frames_done": 0, "frames": null,
         "sample_rate": null, "channels": null, "lanes": 0,
         "loudness": {"integrated": null, "range": null, "true_peak": null, "sample_peak": 0.0},
         "readout": {"text": "Could not analyse", "warn": false, "segments": []}}
        """
        let analysis = try CrossfadeJSON.decode(AnalysisState.self, from: failed)
        XCTAssertNil(analysis.frames)
        XCTAssertNil(analysis.loudness.truePeak)
        let empty = """
        {"revision": 0, "state": "empty", "failure": null, "position": 0, "duration": null,
         "sample_rate": null, "channels": null, "transport": null, "scrubbing": false,
         "device_running": false, "underruns": 0, "pulled": 0,
         "output": {"sample_rate": 48000, "channels": 2}, "a_key_from_the_future": true}
        """
        let player = try CrossfadeJSON.decode(PlayerState.self, from: empty)
        XCTAssertNil(player.transport)
        XCTAssertNil(player.duration)
    }

    func testRunningJobWithNormalization() throws {
        let json = """
        {"id": 3, "input": "/a.wav", "output": "/a.opus", "command_line": null, "state": "running",
         "frames_done": 10, "frames_total": null, "fraction": null, "cancelling": false,
         "frames": null, "message": null, "measuring": false, "clipped": 0,
         "normalization": {"measured_lufs": -6.9, "target_lufs": -16.0, "reached_lufs": -16.0,
           "gain_db": -9.1, "limited_by_ceiling": false, "ceiling_dbtp": -1.0}}
        """
        let job = try CrossfadeJSON.decode(QueueJob.self, from: json)
        XCTAssertEqual(job.normalization?.gainDb, -9.1)
        XCTAssertEqual(job.normalization?.ceilingDbtp, -1.0)
        XCTAssertNil(job.fraction)
    }

    func testABadDocumentIsADecodingError() {
        XCTAssertThrowsError(try CrossfadeJSON.decode(About.self, from: "{\"lines\": 3}")) { error in
            XCTAssertEqual((error as? CrossfadeError)?.kind, .decoding)
        }
    }
}
