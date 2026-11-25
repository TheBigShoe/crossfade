import CrossfadeModels
import SwiftUI

@main
@MainActor
struct CrossfadeApp: App {
    @State private var app: AppModel

    init() {
        _app = State(initialValue: AppBootstrap.makeAppModel())
    }

    var body: some Scene {
        WindowGroup {
            RootView(app: app)
                .task {
                    await app.start()
                }
                .onOpenURL { url in
                    // A document sent from Files or another app, opened in place when it can be.
                    Task {
                        await app.open([url])
                    }
                }
        }
    }
}
