import CrossfadeModels
import SwiftUI

/// The conversions: progress, cancel, retry, remove, clear finished; a finished file can be
/// shared, shown in Files or added to the library.
struct QueueView: View {
    let app: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                ForEach(app.queue.rows) { row in
                    QueueRowView(row: row, app: app)
                }
                if let message = app.queue.message {
                    Label(message, systemImage: "exclamationmark.circle")
                        .foregroundStyle(.red)
                }
            }
            .overlay {
                if app.queue.rows.isEmpty {
                    ContentUnavailableView(
                        "No Conversions",
                        systemImage: "tray",
                        description: Text("Converted files are saved to the Crossfade folder in Files."))
                }
            }
            .navigationTitle("Queue")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Clear Finished") { app.queue.clearFinished() }
                        .disabled(!app.queue.hasFinished)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .safeAreaInset(edge: .bottom) {
                if let warning = app.queue.quitWarning {
                    Text(warning)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .padding()
                        .frame(maxWidth: .infinity)
                        .background(.bar)
                }
            }
        }
    }
}
