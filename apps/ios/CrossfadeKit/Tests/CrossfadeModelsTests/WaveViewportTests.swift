import CrossfadeModels
import XCTest

/// The phone's zoom and pan over a file's frames.
final class WaveViewportTests: XCTestCase {
    func testAWholeViewAsksForCeilFramesOverWidth() {
        let view = WaveViewport(totalFrames: 1_000)
        XCTAssertFalse(view.isZoomed)
        XCTAssertEqual(view.framesPerColumn(columns: 300), 4)
        XCTAssertEqual(view.startFrame, 0)
    }

    func testZoomKeepsTheAnchorFrameInPlace() {
        var view = WaveViewport(totalFrames: 100_000)
        let before = view.frame(atFraction: 0.25)
        view.zoom(by: 4, anchor: 0.25)
        XCTAssertTrue(view.isZoomed)
        XCTAssertEqual(view.span, 25_000)
        XCTAssertEqual(Double(view.frame(atFraction: 0.25)), Double(before), accuracy: 1)
    }

    func testZoomStopsAtTheLimitsAndPanStaysInTheFile() {
        var view = WaveViewport(totalFrames: 100_000)
        view.zoom(by: 1_000_000, anchor: 0.5)
        XCTAssertEqual(view.span, WaveViewport.minimumSpan)
        view.pan(byFraction: 1_000)
        XCTAssertEqual(view.start, 0)
        view.pan(byFraction: -1_000_000)
        XCTAssertEqual(view.start + view.span, 100_000)
        view.zoom(by: 0.000_001, anchor: 0.5)
        XCTAssertEqual(view.span, 100_000)
        XCTAssertEqual(view.start, 0)
    }

    func testFollowTurnsThePage() {
        var view = WaveViewport(totalFrames: 100_000)
        view.zoom(by: 10, anchor: 0)
        XCTAssertNotNil(view.fraction(ofFrame: 5_000))
        XCTAssertNil(view.fraction(ofFrame: 50_000))
        view.follow(frame: 50_000)
        XCTAssertEqual(view.fraction(ofFrame: 50_000), 0)
        view.fit()
        XCTAssertFalse(view.isZoomed)
    }
}
