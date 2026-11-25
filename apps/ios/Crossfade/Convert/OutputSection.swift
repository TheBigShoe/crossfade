import CrossfadeModels
import SwiftUI

/// Where the file goes: a name in the app's folder (shown in Files as On My iPhone ›
/// Crossfade), and whether an existing file may be replaced.
@MainActor
struct OutputSection: View {
    let form: ConvertForm
    let model: ConvertModel
    let folderName: String
    @State private var draft: String?
    @FocusState private var editing: Bool

    var body: some View {
        Section {
            LabeledContent("Name") {
                TextField("File name", text: Binding(get: { draft ?? form.outputName }, set: { draft = $0 }))
                    .multilineTextAlignment(.trailing)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .focused($editing)
                    .submitLabel(.done)
                    .onSubmit(commit)
            }
            ToggleRowView(row: form.overwrite, send: model.send)
        } header: {
            Text("Save To")
        } footer: {
            Text("The \(folderName) folder in the Files app. "
                + (model.followsDefaultName ? "The name follows your choices until you type one." : ""))
        }
        .onChange(of: editing) { _, isEditing in
            if !isEditing { commit() }
        }
    }

    private func commit() {
        guard let name = draft else { return }
        draft = nil
        if name != form.outputName {
            model.rename(to: name)
        }
    }
}
