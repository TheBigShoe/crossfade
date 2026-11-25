import CrossfadeKit
import CrossfadeModels
import SwiftUI

/// A text field of Adjust. Each keystroke goes to the sheet, which keeps the text as typed
/// and says, under the field, why it cannot be used.
@MainActor
struct TextRowView: View {
    let row: TextRow
    let send: (SheetChange) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            LabeledContent(row.title) {
                HStack(spacing: 4) {
                    TextField(row.placeholder, text: Binding(get: { row.text }, set: { send(row.change($0)) }))
                        .multilineTextAlignment(.trailing)
                        .keyboardType(.numbersAndPunctuation)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                    if !row.unit.isEmpty {
                        Text(row.unit)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            if let problem = row.problem {
                Text(problem)
                    .font(.footnote)
                    .foregroundStyle(.red)
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityValue(row.problem ?? "")
    }
}
