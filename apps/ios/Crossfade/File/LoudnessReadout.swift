import CrossfadeModels
import SwiftUI

/// The loudness line under the waveform, in the library's words, a figure in the warning
/// colour where it asks; analysis progress while it runs.
struct LoudnessReadout: View {
    let analysis: AnalysisModel?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let progress = analysis?.progress {
                ProgressView(value: progress)
                    .accessibilityLabel("Analysing")
            }
            readout
                .font(.subheadline.monospacedDigit())
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityElement(children: .combine)
    }

    private var readout: Text {
        if let failure = analysis?.failure {
            return Text(failure).foregroundStyle(.secondary)
        }
        guard let segments = analysis?.state?.readout.segments, !segments.isEmpty else {
            return Text(analysis?.state?.readout.text ?? "Analysing…").foregroundStyle(.secondary)
        }
        return segments.reduce(Text("")) { line, segment in
            line + Text(segment.text).foregroundStyle(segment.warn ? Color.orange : Color.secondary)
        }
    }
}
