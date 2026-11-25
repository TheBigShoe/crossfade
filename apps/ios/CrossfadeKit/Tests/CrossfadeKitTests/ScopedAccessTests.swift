import CrossfadeKit
import XCTest

/// Every start that succeeded is stopped exactly once, however the holder lets go.
final class ScopedAccessTests: XCTestCase {
    func testEndStopsOnce() {
        let file = CountingResource()
        let access = ScopedAccess(file)
        XCTAssertTrue(access.isActive)
        access.end()
        access.end()
        XCTAssertEqual(file.starts, 1)
        XCTAssertEqual(file.stops, 1)
        XCTAssertFalse(access.isActive)
    }

    func testDeinitStops() {
        let file = CountingResource()
        do {
            let access = ScopedAccess(file)
            XCTAssertEqual(file.open, 1)
            _ = access
        }
        XCTAssertEqual(file.open, 0)
    }

    func testAFailedStartIsNeverStopped() {
        let file = CountingResource(grants: false)
        let access = ScopedAccess(file)
        access.end()
        XCTAssertEqual(file.starts, 1)
        XCTAssertEqual(file.stops, 0)
    }

    func testDuringStopsWhenTheBodyThrows() {
        struct Boom: Error {}
        let file = CountingResource()
        XCTAssertThrowsError(try ScopedAccess.during(file) { () throws -> Int in throw Boom() })
        XCTAssertEqual(file.open, 0)
        let value = ScopedAccess.during(file) { 42 }
        XCTAssertEqual(value, 42)
        XCTAssertEqual(file.starts, 2)
        XCTAssertEqual(file.stops, 2)
    }

    func testLedgerBalances() {
        let a = CountingResource(), b = CountingResource(), c = CountingResource()
        let ledger = ScopeLedger<Int>()
        ledger.hold(1, [a])
        ledger.hold(2, [b])
        ledger.hold(1, [c])  // holding a key again lets go of what it held
        XCTAssertEqual(a.open, 0)
        XCTAssertEqual(c.open, 1)
        ledger.release(allBut: [2])
        XCTAssertEqual(c.open, 0)
        XCTAssertEqual(b.open, 1)
        ledger.release(2)
        ledger.release(2)
        XCTAssertEqual(b.open, 0)
        XCTAssertEqual(b.stops, 1)
        ledger.hold(3, [a, b])
        ledger.releaseAll()
        XCTAssertEqual([a.open, b.open, c.open], [0, 0, 0])
        XCTAssertTrue(ledger.keys.isEmpty)
    }
}
