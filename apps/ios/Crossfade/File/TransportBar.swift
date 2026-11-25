import CrossfadeModels
import SwiftUI

/// Play, pause, stop and ten-second skips, with the desktop's position readout.
struct TransportBar: View {
    let player: PlayerModel

    var body: some View {
        VStack(spacing: 6) {
            HStack(spacing: 28) {
                button("Back 10 seconds", "gobackward.10") { player.skip(seconds: -10) }
                Button {
                    player.toggle()
                } label: {
                    Image(systemName: player.isPlaying ? "pause.circle.fill" : "play.circle.fill")
                        .font(.system(size: 52))
                        .symbolRenderingMode(.hierarchical)
                        .contentTransition(.symbolEffect(.replace))
                        .frame(minWidth: 60, minHeight: 60)
                }
                .accessibilityLabel(player.isPlaying ? "Pause" : "Play")
                .disabled(!player.isLoaded)
                button("Forward 10 seconds", "goforward.10") { player.skip(seconds: 10) }
                button("Stop", "stop.fill") { player.stop() }
            }
            .buttonStyle(.plain)
            .foregroundStyle(Color.accentColor)
            .sensoryFeedback(.impact(weight: .light), trigger: player.isPlaying)

            Text(player.failure ?? player.message ?? player.transportText)
                .font(.footnote.monospacedDigit())
                .foregroundStyle(.secondary)
                .accessibilityLabel("Position")
                .accessibilityValue(player.transportText)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 4)
    }

    private func button(_ label: String, _ symbol: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.title2)
                .frame(minWidth: 44, minHeight: 44)
        }
        .accessibilityLabel(label)
        .disabled(!player.isLoaded)
    }
}
