import CrossfadeKit
import CrossfadeModels
import SwiftUI

/// A switch row, shown and enabled as the sheet says.
@MainActor
struct ToggleRowView: View {
    let row: ToggleRow
    let send: (SheetChange) -> Void

    var body: some View {
        if row.visible {
            Toggle(row.title, isOn: Binding(get: { row.isOn }, set: { send(row.change($0)) }))
                .disabled(!row.enabled)
        }
    }
}
