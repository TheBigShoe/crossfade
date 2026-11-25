import CrossfadeModels
import SwiftUI

/// The toolbar's way to the queue, with how many jobs are still to do.
@MainActor
struct QueueButton: View {
    let queue: QueueModel
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label("Queue", systemImage: queue.isBusy ? "arrow.triangle.2.circlepath" : "tray.full")
                .overlay(alignment: .topTrailing) {
                    if queue.activeCount > 0 {
                        Text("\(queue.activeCount)")
                            .font(.caption2.weight(.semibold).monospacedDigit())
                            .foregroundStyle(.white)
                            .padding(.horizontal, 5)
                            .background(Capsule().fill(Color.accentColor))
                            .offset(x: 8, y: -6)
                    }
                }
        }
        .accessibilityLabel("Queue")
        .accessibilityValue(queue.activeCount > 0 ? "\(queue.activeCount) to do" : "Nothing to do")
    }
}
