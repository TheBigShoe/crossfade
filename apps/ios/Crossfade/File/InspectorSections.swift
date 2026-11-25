import CrossfadeKit
import SwiftUI

/// The inspector's sections as they come, each row its label and friendly value.
@MainActor
struct InspectorSections: View {
    let sections: [InspectorSection]

    var body: some View {
        ForEach(sections) { section in
            Section {
                ForEach(section.rows) { row in
                    LabeledContent(row.label) {
                        Text(row.value)
                            .multilineTextAlignment(.trailing)
                            .textSelection(.enabled)
                    }
                }
            } header: {
                Text(section.title)
            }
        }
    }
}
