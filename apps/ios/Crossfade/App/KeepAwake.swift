import CrossfadeModels
import UIKit

/// While a conversion is pending or running: the screen stays on, and a background-task
/// assertion asks iOS for time to finish if the app leaves the screen. iOS grants that time
/// briefly (often about 30 seconds); when it runs out the assertion is ended, as iOS requires,
/// and the app is suspended with the job paused, to carry on when the app is opened again.
@MainActor
final class KeepAwake: ActivityAssertion {
    private var task: UIBackgroundTaskIdentifier = .invalid

    func setActive(_ active: Bool) {
        UIApplication.shared.isIdleTimerDisabled = active
        if active {
            guard task == .invalid else { return }
            task = UIApplication.shared.beginBackgroundTask(withName: "Crossfade conversion") { [weak self] in
                // Called on the main thread; the assertion must end before this returns.
                MainActor.assumeIsolated {
                    self?.endTask()
                }
            }
        } else {
            endTask()
        }
    }

    private func endTask() {
        if task != .invalid {
            UIApplication.shared.endBackgroundTask(task)
            task = .invalid
        }
    }
}
