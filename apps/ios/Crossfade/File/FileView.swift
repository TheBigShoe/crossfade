import CrossfadeModels
import SwiftUI

/// One file: the waveform as the hero, the transport, the loudness line, and the inspector's
/// sections as a grouped list.
@MainActor
struct FileView: View {
    let app: AppModel
    let item: LibraryItem
    let convert: (LibraryItem) -> Void

    var body: some View {
        List {
            if item.isReadable {
                Section {
                    WaveformView(analysis: app.analysis, player: app.player)
                        .frame(height: 190)
                        .listRowInsets(EdgeInsets(top: 12, leading: 12, bottom: 12, trailing: 12))
                    if app.player.isAvailable {
                        TransportBar(player: app.player)
                    }
                    LoudnessReadout(analysis: app.analysis)
                }
            } else if let problem = item.file?.problem ?? item.failure {
                Section {
                    Label(problem, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                        .textSelection(.enabled)
                }
            }
            if let file = item.file {
                InspectorSections(sections: file.sections)
            }
            Section {
                Text(item.path)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            } header: {
                Text("Location")
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle(item.title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    convert(item)
                } label: {
                    Label("Convert", systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(!item.isReadable)
            }
        }
        .onAppear {
            // The shown file is the selection, also when reached by a link rather than the list.
            app.select(item.id)
        }
    }
}
