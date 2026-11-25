import SwiftUI
import UIKit

/// The equivalent `crossfade convert` command, selectable and copyable.
@MainActor
struct CommandLineSection: View {
    let line: String
    @State private var copied = 0

    var body: some View {
        Section {
            Text(line)
                .font(.system(.footnote, design: .monospaced))
                .textSelection(.enabled)
            Button {
                UIPasteboard.general.string = line
                copied += 1
            } label: {
                Label("Copy Command", systemImage: "doc.on.doc")
            }
            .sensoryFeedback(.success, trigger: copied)
        } header: {
            Text("Command Line")
        } footer: {
            Text("The same conversion with the crossfade command-line tool.")
        }
    }
}
