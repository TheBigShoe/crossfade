import CrossfadeKit
import CrossfadeModels
import SwiftUI

/// The tag of a row's own label, shown when the sheet holds none of the options.
private let unlistedTag = -1

/// A picker row: hidden, disabled and filled exactly as the sheet says; choosing sends the
/// option's own change.
@MainActor
struct ChoiceRowView: View {
    let row: ChoiceRow
    let send: (SheetChange) -> Void

    var body: some View {
        if row.visible {
            VStack(alignment: .leading, spacing: 4) {
                Picker(row.title, selection: selection) {
                    if row.selected == nil {
                        Text(row.label).tag(unlistedTag)
                    }
                    ForEach(row.choices.indices, id: \.self) { index in
                        Text(row.choices[index].label).tag(index)
                    }
                }
                .disabled(!row.enabled)
                if let hint = row.hint {
                    Text(hint)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private var selection: Binding<Int> {
        Binding(
            get: { row.selected ?? unlistedTag },
            set: { index in
                if row.choices.indices.contains(index), index != row.selected {
                    send(row.choices[index].change)
                }
            })
    }
}
