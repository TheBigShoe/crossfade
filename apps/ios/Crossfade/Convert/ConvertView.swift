import CrossfadeModels
import SwiftUI

/// The convert form, drawn entirely from the sheet's state: each row as the state shows,
/// hides, enables and fills it; problems and notes in the library's words; the equivalent
/// command line; Add to Queue only while the sheet says it can queue.
struct ConvertView: View {
    let app: AppModel
    @State private var model: ConvertModel
    @State private var queued = 0
    @Environment(\.dismiss) private var dismiss

    init(app: AppModel, item: LibraryItem) {
        self.app = app
        _model = State(initialValue: app.makeConvert(for: item))
    }

    var body: some View {
        NavigationStack {
            Group {
                if let form = model.form {
                    ConvertFormView(form: form, model: model, folderName: "Crossfade")
                } else if model.isOpening {
                    ProgressView("Reading \(model.title)…")
                } else {
                    ContentUnavailableView(
                        "Cannot Convert",
                        systemImage: "exclamationmark.triangle",
                        description: Text(model.message ?? "This file cannot be read."))
                }
            }
            .navigationTitle("Convert \(model.title)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add to Queue") {
                        if model.addToQueue(app.queue) != nil {
                            queued += 1
                            dismiss()
                        }
                    }
                    .disabled(model.form?.canQueue != true)
                }
            }
        }
        .sensoryFeedback(.success, trigger: queued)
        .task {
            await model.open(engine: app.engine)
        }
    }
}
