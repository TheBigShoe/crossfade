import CrossfadeKit
import CrossfadeModels
import SwiftUI

/// Level, trim and fade. Open and closed as the sheet holds it; the line under it is the
/// sheet's processing note: what will be written, or why the controls are off.
struct AdjustSection: View {
    let adjust: AdjustForm
    let processingNote: String?
    let send: (SheetChange) -> Void

    var body: some View {
        Section {
            DisclosureGroup(isExpanded: Binding(get: { adjust.open }, set: { send(adjust.openChange($0)) })) {
                Picker("Level", selection: levelMode) {
                    ForEach(adjust.levelModes.indices, id: \.self) { index in
                        Text(adjust.levelModes[index].label).tag(index)
                    }
                }
                .pickerStyle(.segmented)
                if let level = adjust.level {
                    TextRowView(row: level, send: send)
                }
                TextRowView(row: adjust.trimStart, send: send)
                TextRowView(row: adjust.trimEnd, send: send)
                TextRowView(row: adjust.fadeIn, send: send)
                TextRowView(row: adjust.fadeOut, send: send)
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Adjust")
                    if let summary = adjust.summary, !adjust.open {
                        Text(summary)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .disabled(!adjust.enabled)
        } footer: {
            if let note = processingNote {
                Text(note)
            }
        }
    }

    private var levelMode: Binding<Int> {
        Binding(
            get: { adjust.levelModeIndex ?? 0 },
            set: { index in
                if adjust.levelModes.indices.contains(index), index != adjust.levelModeIndex {
                    send(adjust.levelModes[index].change)
                }
            })
    }
}
