import Foundation

/// Keeps the app awake while conversions run: the app's implementation keeps the screen on and
/// holds a background-task assertion; the tests count the calls.
@MainActor
public protocol ActivityAssertion: AnyObject {
    /// True while a job is pending or running; called on every change.
    func setActive(_ active: Bool)
}
