import CrossfadeKit
import SwiftUI

/// Draws waveform lanes, one column per point: a bar from each column's minimum to its
/// maximum. Columns not analysed yet (NaN) are left empty.
@MainActor
struct WaveformCanvas: View {
    let lanes: [WaveColumns]
    /// The analysis's revision, so a new one redraws even when the lanes compare equal.
    let revision: UInt64

    var body: some View {
        Canvas { context, size in
            guard !lanes.isEmpty else { return }
            let laneHeight = size.height / CGFloat(lanes.count)
            for (index, lane) in lanes.enumerated() {
                let top = CGFloat(index) * laneHeight
                let middle = top + laneHeight / 2
                let half = laneHeight / 2 * 0.9
                var centre = Path()
                centre.move(to: CGPoint(x: 0, y: middle))
                centre.addLine(to: CGPoint(x: size.width, y: middle))
                context.stroke(centre, with: .color(.secondary.opacity(0.25)), lineWidth: 0.5)

                var bars = Path()
                let count = min(lane.mins.count, lane.maxs.count)
                for column in 0..<count {
                    let low = lane.mins[column]
                    let high = lane.maxs[column]
                    if low.isNaN || high.isNaN { continue }
                    let y1 = middle - CGFloat(min(max(high, -1), 1)) * half
                    let y2 = middle - CGFloat(min(max(low, -1), 1)) * half
                    bars.addRect(CGRect(x: CGFloat(column), y: y1, width: 1, height: max(y2 - y1, 0.5)))
                }
                context.fill(bars, with: .color(.accentColor))
            }
        }
        .accessibilityHidden(true)
    }
}
