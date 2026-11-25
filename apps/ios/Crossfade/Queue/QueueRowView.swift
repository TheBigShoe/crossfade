import CrossfadeModels
import SwiftUI
import UIKit

/// One job: its output's name, its state in words, progress while it runs, the engine's whole
/// message if it failed, and the actions its state allows.
struct QueueRowView: View {
    let row: QueueRow
    let app: AppModel
    @Environment(\.openURL) private var openURL

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Image(systemName: symbol)
                    .foregroundStyle(tint)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.title)
                        .font(.body)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Text("from \(row.source)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 8)
                if let url = row.outputURL {
                    ShareLink(item: url) {
                        Label("Share", systemImage: "square.and.arrow.up")
                            .labelStyle(.iconOnly)
                    }
                    .buttonStyle(.borderless)
                }
            }
            if row.showsProgress {
                if let progress = row.progress {
                    ProgressView(value: progress)
                } else {
                    ProgressView()
                        .progressViewStyle(.linear)
                }
            }
            Text(row.status)
                .font(.footnote.monospacedDigit())
                .foregroundStyle(.secondary)
            if let failure = row.failure {
                Text(failure)
                    .font(.footnote)
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .swipeActions(edge: .trailing, allowsFullSwipe: false) {
            if row.canRemove {
                Button(role: .destructive) {
                    app.queue.remove(row.id)
                } label: {
                    Label("Remove", systemImage: "trash")
                }
            }
            if row.canCancel {
                Button {
                    app.queue.cancel(row.id)
                } label: {
                    Label("Cancel", systemImage: "xmark.circle")
                }
                .tint(.orange)
            }
            if row.canRetry {
                Button {
                    app.queue.retry(row.id)
                } label: {
                    Label("Retry", systemImage: "arrow.clockwise")
                }
                .tint(.accentColor)
            }
        }
        .contextMenu { menu }
    }

    @ViewBuilder
    private var menu: some View {
        if let url = row.outputURL {
            ShareLink(item: url) {
                Label("Share", systemImage: "square.and.arrow.up")
            }
            Button {
                if let folder = FilesLink.folder(of: url) { openURL(folder) }
            } label: {
                Label("Show in Files", systemImage: "folder")
            }
            Button {
                Task { await app.open([url]) }
            } label: {
                Label("Add to Library", systemImage: "plus.rectangle.on.folder")
            }
        }
        if row.canCancel {
            Button { app.queue.cancel(row.id) } label: { Label("Cancel", systemImage: "xmark.circle") }
        }
        if row.canRetry {
            Button { app.queue.retry(row.id) } label: { Label("Retry", systemImage: "arrow.clockwise") }
        }
        if let line = row.commandLine {
            Button {
                UIPasteboard.general.string = line
            } label: {
                Label("Copy Command", systemImage: "doc.on.doc")
            }
        }
        if row.canRemove {
            Button(role: .destructive) { app.queue.remove(row.id) } label: { Label("Remove", systemImage: "trash") }
        }
    }

    private var symbol: String {
        switch row.phase {
        case .pending: return "clock"
        case .running: return "arrow.triangle.2.circlepath"
        case .finished: return "checkmark.circle.fill"
        case .failed: return "exclamationmark.triangle.fill"
        case .cancelled: return "xmark.circle"
        case nil: return "questionmark.circle"
        }
    }

    private var tint: Color {
        switch row.phase {
        case .finished: return .green
        case .failed: return .red
        case .running: return .accentColor
        default: return .secondary
        }
    }
}
