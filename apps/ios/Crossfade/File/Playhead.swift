import CrossfadeModels
import SwiftUI

/// The playhead, redrawn every display frame while playing (the position is a few atomic
/// loads in the library) and left still otherwise.
struct Playhead: View {
    let player: PlayerModel
    let viewport: WaveViewport

    var body: some View {
        TimelineView(.animation(minimumInterval: nil, paused: !player.isPlaying)) { _ in
            GeometryReader { geometry in
                if let fraction = viewport.fraction(ofFrame: player.position()) {
                    Rectangle()
                        .fill(Color.primary)
                        .frame(width: 2, height: geometry.size.height)
                        .offset(x: CGFloat(fraction) * geometry.size.width - 1)
                }
            }
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}
