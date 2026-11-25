import CrossfadeModels
import SwiftUI

/// The form's sections. Every row comes from `form`; every edit goes to `model.send`.
struct ConvertFormView: View {
    let form: ConvertForm
    let model: ConvertModel
    let folderName: String

    var body: some View {
        Form {
            Section {
                ChoiceRowView(row: form.preset, send: model.send)
                ChoiceRowView(row: form.container, send: model.send)
                ChoiceRowView(row: form.format, send: model.send)
                ChoiceRowView(row: form.compression, send: model.send)
                ChoiceRowView(row: form.bitrate, send: model.send)
                ChoiceRowView(row: form.sampleRate, send: model.send)
                ChoiceRowView(row: form.channels, send: model.send)
                ToggleRowView(row: form.copy, send: model.send)
            } header: {
                Text("Format")
            }

            AdjustSection(adjust: form.adjust, processingNote: form.processingNote, send: model.send)

            OutputSection(form: form, model: model, folderName: folderName)

            if !form.problems.isEmpty || model.message != nil {
                Section {
                    ForEach(form.problems, id: \.self) { problem in
                        Label(problem, systemImage: "exclamationmark.circle.fill")
                            .foregroundStyle(.red)
                    }
                    if let message = model.message, !form.problems.contains(message) {
                        Label(message, systemImage: "exclamationmark.circle.fill")
                            .foregroundStyle(.red)
                    }
                } header: {
                    Text("Cannot Queue Yet")
                }
            }

            if !form.notes.isEmpty {
                Section {
                    ForEach(form.notes, id: \.self) { note in
                        Label(note, systemImage: "info.circle")
                            .foregroundStyle(.secondary)
                    }
                } header: {
                    Text("Notes")
                }
            }

            if let line = form.commandLine {
                CommandLineSection(line: line)
            }
        }
    }
}
