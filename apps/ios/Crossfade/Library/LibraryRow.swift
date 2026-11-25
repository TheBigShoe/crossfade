import CrossfadeModels
import SwiftUI

/// One file: its name, and the inspector's summary or why it cannot be read.
@MainActor
struct LibraryRow: View {
    let item: LibraryItem

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: icon)
                .font(.title3)
                .foregroundStyle(item.isReadable || item.isInspecting ? Color.accentColor : Color.orange)
                .frame(width: 28)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title)
                    .font(.body)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(item.subtitle)
                    .font(.footnote.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            Spacer(minLength: 0)
            if item.isInspecting {
                ProgressView()
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityHint(item.isReadable ? "Shows the waveform and details" : "")
    }

    private var icon: String {
        if item.isInspecting { return "doc" }
        return item.isReadable ? "waveform" : "exclamationmark.triangle"
    }
}
