import CrossfadeModels
import SwiftUI

/// About Crossfade in the library's words, and the licences the app carries.
@MainActor
struct AboutView: View {
    let model: AboutModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    VStack(alignment: .leading, spacing: 8) {
                        Image(systemName: "waveform")
                            .font(.system(size: 40))
                            .foregroundStyle(Color.accentColor)
                            .accessibilityHidden(true)
                        ForEach(model.lines, id: \.self) { line in
                            Text(line)
                        }
                    }
                    .padding(.vertical, 8)
                }
                Section {
                    ForEach(model.licences) { licence in
                        NavigationLink {
                            LicenceView(licence: licence)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(licence.title)
                                Text(licence.detail)
                                    .font(.footnote)
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                } header: {
                    Text("Licences")
                }
            }
            .navigationTitle("About")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}
