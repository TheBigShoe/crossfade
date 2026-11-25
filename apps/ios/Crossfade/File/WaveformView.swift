import CrossfadeKit
import CrossfadeModels
import SwiftUI

/// The waveform: one lane per channel (a combined lane past stereo, as the library plans),
/// pinch to zoom, drag to pan once zoomed, tap to seek, touch and hold then drag to scrub, and
/// the playhead following playback.
struct WaveformView: View {
    let analysis: AnalysisModel?
    let player: PlayerModel
    @State private var viewport = WaveViewport(totalFrames: 1)
    @State private var gestureStart: WaveViewport?
    @State private var scrubbing = false

    private var totalFrames: UInt64 {
        let analysed = analysis?.totalFrames ?? 0
        return analysed > 0 ? analysed : player.duration
    }

    var body: some View {
        GeometryReader { geometry in
            let width = max(geometry.size.width, 1)
            // Read in the body so each revision of the analysis redraws.
            let revision = analysis?.state?.revision ?? 0
            let lanes = analysis?.lanes(viewport: viewport, columns: Int(width)) ?? []
            ZStack(alignment: .topLeading) {
                WaveformCanvas(lanes: lanes, revision: revision)
                Playhead(player: player, viewport: viewport)
            }
            .contentShape(Rectangle())
            .onTapGesture(coordinateSpace: .local) { location in
                player.seek(to: viewport.frame(atFraction: location.x / width))
            }
            .gesture(scrub(width: width))
            .simultaneousGesture(pan(width: width), including: viewport.isZoomed ? .all : .subviews)
            .simultaneousGesture(zoom)
        }
        .background(.fill.quaternary, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(alignment: .topTrailing) {
            if viewport.isZoomed {
                Button {
                    withAnimation(.snappy) { viewport.fit() }
                } label: {
                    Label("Show Whole File", systemImage: "arrow.down.right.and.arrow.up.left")
                        .labelStyle(.iconOnly)
                        .padding(8)
                        .background(.ultraThinMaterial, in: Circle())
                }
                .padding(6)
            }
        }
        .onChange(of: totalFrames, initial: true) { _, total in
            viewport = WaveViewport(totalFrames: total)
        }
        .accessibilityElement()
        .accessibilityLabel("Waveform")
        .accessibilityValue(spokenSummary)
        .accessibilityHint("Swipe up or down to move the playhead by five seconds.")
        .accessibilityAdjustableAction { direction in
            switch direction {
            case .increment: player.skip(seconds: 5)
            case .decrement: player.skip(seconds: -5)
            @unknown default: break
            }
        }
    }

    private var zoom: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                let base = gestureStart ?? viewport
                gestureStart = base
                var next = base
                next.zoom(by: Double(value.magnification), anchor: Double(value.startAnchor.x))
                viewport = next
            }
            .onEnded { _ in gestureStart = nil }
    }

    private func pan(width: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 8)
            .onChanged { value in
                guard !scrubbing else { return }
                let base = gestureStart ?? viewport
                gestureStart = base
                var next = base
                next.pan(byFraction: Double(value.translation.width / width))
                viewport = next
            }
            .onEnded { _ in gestureStart = nil }
    }

    private func scrub(width: CGFloat) -> some Gesture {
        LongPressGesture(minimumDuration: 0.35)
            .sequenced(before: DragGesture(minimumDistance: 0))
            .onChanged { value in
                if case .second(true, let drag) = value, let drag = drag {
                    scrubbing = true
                    player.scrub(to: viewport.frame(atFraction: drag.location.x / width))
                }
            }
            .onEnded { _ in
                if scrubbing {
                    scrubbing = false
                    player.endScrub()
                }
            }
    }

    private var spokenSummary: String {
        var parts: [String] = []
        if let state = analysis?.state {
            if let channels = state.channels {
                parts.append(channels == 1 ? "1 channel" : "\(channels) channels")
            }
            parts.append(state.readout.text)
        } else if let failure = analysis?.failure {
            parts.append(failure)
        }
        if !player.transportText.isEmpty {
            parts.append("Playhead \(player.transportText)")
        }
        return parts.joined(separator: ", ")
    }
}
