import CrossfadeModels
import SwiftUI

/// The files the person brought in, each with the inspector's line or its problem.
struct LibraryView: View {
    let app: AppModel
    @Binding var showingImporter: Bool
    let convert: (LibraryItem) -> Void
    @State private var dropTargeted = false

    var body: some View {
        List(selection: selection) {
            ForEach(app.library.items) { item in
                LibraryRow(item: item)
                    .tag(item.id)
                    .contextMenu {
                        Button {
                            convert(item)
                        } label: {
                            Label("Convert…", systemImage: "arrow.triangle.2.circlepath")
                        }
                        .disabled(!item.isReadable)
                        Button(role: .destructive) {
                            app.remove(ids: [item.id])
                        } label: {
                            Label("Remove from Library", systemImage: "trash")
                        }
                    }
            }
            .onDelete { offsets in
                app.remove(atOffsets: offsets)
            }
        }
        .navigationTitle("Library")
        .overlay {
            if app.library.items.isEmpty {
                ContentUnavailableView {
                    Label("No Files", systemImage: "music.note.list")
                } description: {
                    Text("Add audio from Files, or open it in Crossfade from another app.")
                } actions: {
                    Button("Add Files") { showingImporter = true }
                        .buttonStyle(.borderedProminent)
                }
            }
        }
        .onDrop(of: AudioTypes.importable, isTargeted: $dropTargeted) { providers in
            DropImporter.load(providers) { [app] urls in
                Task { await app.open(urls) }
            }
        }
        .overlay {
            if dropTargeted {
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .strokeBorder(Color.accentColor, style: StrokeStyle(lineWidth: 3, dash: [8, 6]))
                    .padding(8)
                    .allowsHitTesting(false)
            }
        }
    }

    private var selection: Binding<LibraryItem.ID?> {
        Binding(get: { app.selection }, set: { app.select($0) })
    }
}
