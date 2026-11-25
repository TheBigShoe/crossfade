import CrossfadeKit
import CrossfadeModels
import XCTest

/// The form shows exactly what the sheet's state says: rows appear, hide, enable and offer
/// options as the state does, and choosing an option sends the sheet's own change.
final class ConvertFormTests: XCTestCase {
    func testDefaultSheet() throws {
        let form = ConvertForm(state: try Fixture.sheet())
        XCTAssertEqual(form.container.label, "WAV")
        XCTAssertEqual(form.container.selected, 0)
        XCTAssertEqual(form.container.choices.count, 6)
        XCTAssertEqual(form.container.choices[1].change, .container("flac"))
        XCTAssertTrue(form.format.visible)
        XCTAssertEqual(form.format.choices.map { $0.label }.first, "Same as source")
        XCTAssertFalse(form.compression.visible)
        XCTAssertFalse(form.bitrate.visible)
        XCTAssertFalse(form.bitrate.enabled)
        XCTAssertTrue(form.copy.visible)
        XCTAssertEqual(form.copy.change(true), .copy(true))
        XCTAssertEqual(form.preset.label, "Same as source")
        XCTAssertEqual(form.sampleRate.selected, 0)
        XCTAssertEqual(form.sampleRate.choices[0].change, .sampleRate(nil))
        XCTAssertEqual(form.channels.choices[1].change, .channels("mono"))
        XCTAssertEqual(form.outputName, "in-copy.wav")
        XCTAssertTrue(form.canQueue)
        XCTAssertNotNil(form.commandLine)
        XCTAssertTrue(form.problems.isEmpty)
        XCTAssertFalse(form.adjust.open)
        XCTAssertNil(form.adjust.level)
        XCTAssertEqual(form.adjust.levelModeIndex, 0)
    }

    func testOpusSheet() throws {
        let form = ConvertForm(state: try Fixture.sheet("sheet_opus"))
        XCTAssertFalse(form.format.visible)
        XCTAssertTrue(form.bitrate.visible)
        XCTAssertTrue(form.bitrate.enabled)
        XCTAssertEqual(form.bitrate.selected, 3)
        XCTAssertEqual(form.bitrate.choices[3].label, "96 kb/s")
        XCTAssertEqual(form.bitrate.choices[0].change, .bitrate(nil))
        XCTAssertEqual(form.sampleRate.choices.count, 1)
        XCTAssertFalse(form.copy.visible)
        XCTAssertEqual(form.notes, ["Opus is always 48 kHz: resampling from 44100 Hz"])
        XCTAssertEqual(form.processingNote, "48 kHz · stereo · 0:01.000")
        XCTAssertTrue(form.adjust.open)
        XCTAssertEqual(form.adjust.level?.unit, "LUFS")
        XCTAssertEqual(form.adjust.level?.text, "-16")
        XCTAssertEqual(form.adjust.level?.change("-14"), .levelValue("-14"))
        XCTAssertEqual(form.adjust.fadeOut.text, "0.5")
    }

    func testAProblemDisablesQueueingAndIsShownInTheSheetsWords() throws {
        let form = ConvertForm(state: try Fixture.sheet("sheet_problems"))
        XCTAssertFalse(form.canQueue)
        XCTAssertNil(form.commandLine)
        XCTAssertEqual(form.problems.count, 1)
        XCTAssertTrue(form.problems[0].hasPrefix("Crossfade can save"))
        XCTAssertEqual(form.outputName, "x.mp3")
    }

    func testAnAdjustProblemStaysOnItsRow() throws {
        var state = try Fixture.sheet()
        state.adjust.levelMode = "gain"
        state.adjust.problems.level = "Enter a gain in dB, such as -3."
        state.problems = [SheetProblem(kind: "adjust", row: "level", message: "Enter a gain in dB, such as -3.")]
        state.canQueue = false
        let form = ConvertForm(state: state)
        XCTAssertEqual(form.adjust.level?.problem, "Enter a gain in dB, such as -3.")
        XCTAssertEqual(form.adjust.level?.unit, "dB")
        XCTAssertTrue(form.problems.isEmpty)
        XCTAssertFalse(form.canQueue)
    }

    func testRowsFollowTheStateNotTheContainer() throws {
        // Whatever the container, visibility and enabling are the state's flags.
        var state = try Fixture.sheet()
        state.format.visible = false
        state.compressionLevel.visible = true
        state.compressionLevel.enabled = false
        state.compressionLevel.value = 8
        state.preset.value = nil
        let form = ConvertForm(state: state)
        XCTAssertFalse(form.format.visible)
        XCTAssertTrue(form.compression.visible)
        XCTAssertFalse(form.compression.enabled)
        XCTAssertEqual(form.compression.selected, 9)
        XCTAssertEqual(form.compression.choices[0].change, .compressionLevel(nil))
        XCTAssertNil(form.preset.selected)
        XCTAssertEqual(form.preset.label, "Custom")
    }
}
