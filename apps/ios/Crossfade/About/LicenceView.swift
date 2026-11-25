import CrossfadeModels
import SwiftUI

/// One licence's full text, as the app bundle carries it.
@MainActor
struct LicenceView: View {
    let licence: Licence

    var body: some View {
        ScrollView {
            Text(licence.text(in: .main))
                .font(.system(.footnote, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
        }
        .navigationTitle(licence.title)
        .navigationBarTitleDisplayMode(.inline)
    }
}
