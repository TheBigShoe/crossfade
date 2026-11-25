import CrossfadeModels
import SwiftUI

/// The app's frame: the library beside the shown file on iPad (a split view), the library then
/// the file on iPhone (the split view collapses into a stack); the queue, convert and About
/// screens as sheets.
struct RootView: View {
    let app: AppModel
    @State private var showingImporter = false
    @State private var showingQueue = false
    @State private var showingAbout = false
    @State private var convertTarget: LibraryItem?
    @State private var columns: NavigationSplitViewVisibility = .all

    var body: some View {
        NavigationSplitView(columnVisibility: $columns) {
            LibraryView(app: app, showingImporter: $showingImporter, convert: { convertTarget = $0 })
                .toolbar { toolbar }
        } detail: {
            if let item = app.selectedItem {
                FileView(app: app, item: item, convert: { convertTarget = $0 })
                    .id(item.id)
            } else {
                ContentUnavailableView(
                    "No File Selected",
                    systemImage: "waveform",
                    description: Text("Choose a file from the library, or add one."))
            }
        }
        .fileImporter(
            isPresented: $showingImporter,
            allowedContentTypes: AudioTypes.importable,
            allowsMultipleSelection: true
        ) { result in
            if case .success(let urls) = result {
                Task { await app.open(urls) }
            }
        }
        .sheet(item: $convertTarget) { item in
            ConvertView(app: app, item: item)
        }
        .sheet(isPresented: $showingQueue) {
            QueueView(app: app)
        }
        .sheet(isPresented: $showingAbout) {
            AboutView(model: app.about)
        }
        .sensoryFeedback(.success, trigger: app.queue.finishedCount)
        .sensoryFeedback(.error, trigger: app.queue.failedCount)
        .overlay(alignment: .bottom) {
            if let problem = app.startupProblem {
                StartupBanner(text: problem)
            }
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            Button {
                showingAbout = true
            } label: {
                Label("About Crossfade", systemImage: "info.circle")
            }
        }
        ToolbarItemGroup(placement: .topBarTrailing) {
            QueueButton(queue: app.queue) { showingQueue = true }
            Button {
                showingImporter = true
            } label: {
                Label("Add Files", systemImage: "plus")
            }
        }
    }
}
